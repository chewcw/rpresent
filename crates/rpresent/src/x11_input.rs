/// X11 button codes for mouse wheel scroll up/down.
#[cfg(target_os = "linux")]
const BUTTON_WHEEL_UP: u8 = 4;
#[cfg(target_os = "linux")]
const BUTTON_WHEEL_DOWN: u8 = 5;

#[cfg(target_os = "linux")]
pub(crate) fn spawn(
    shortcuts: rpresent_config::ShortcutConfig,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
) {
    std::thread::spawn(move || {
        if let Err(e) = run_input_loop(shortcuts, tx) {
            tracing::error!(error = %e, "X11 input loop failed");
        }
    });
}

#[cfg(target_os = "linux")]
fn run_input_loop(
    shortcuts: rpresent_config::ShortcutConfig,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt as _, EventMask};
    use x11rb::protocol::Event;

    use crate::input::{ActiveMode, InputBindings, InputRouter};

    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    conn.change_window_attributes(
        root,
        &ChangeWindowAttributesAux::new().event_mask(
            EventMask::KEY_PRESS | EventMask::KEY_RELEASE | EventMask::BUTTON_PRESS,
        ),
    )?
    .check()?;
    conn.flush()?;

    let keycode_map: HashMap<u8, &'static str> = build_keycode_map(&conn);
    let bindings = InputBindings::from(&shortcuts);
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::ButtonPress(ev)
                if ev.detail == BUTTON_WHEEL_UP || ev.detail == BUTTON_WHEEL_DOWN =>
            {
                let modifier = modifier_from_state(ev.state);
                let delta = if ev.detail == BUTTON_WHEEL_UP {
                    1.0_f32
                } else {
                    -1.0_f32
                };
                if let Some(action) = router.handle_wheel(modifier, delta) {
                    let _ = tx.send(crate::input::AppEvent::Input(action));
                }
            }
            Event::KeyPress(ev) => {
                if let Some(shortcut) = shortcut_from_keypress(ev.detail, ev.state, &keycode_map) {
                    if let Some(cmd) = router.handle_key(&shortcut) {
                        let _ = tx.send(crate::input::AppEvent::Command(cmd));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
fn build_keycode_map(
    connection: &impl x11rb::connection::Connection,
) -> std::collections::HashMap<u8, &'static str> {
    use x11rb::protocol::xproto::ConnectionExt;
    let setup = connection.setup();
    let min_keycode = setup.min_keycode;
    let max_keycode = setup.max_keycode;
    let count = max_keycode - min_keycode + 1;

    let mut map = std::collections::HashMap::new();
    if let Ok(cookie) = connection.get_keyboard_mapping(min_keycode, count) {
        if let Ok(reply) = cookie.reply() {
            let keysyms_per_keycode = reply.keysyms_per_keycode as usize;
            for (i, chunk) in reply.keysyms.chunks(keysyms_per_keycode).enumerate() {
                let keycode = min_keycode + i as u8;
                // Only index the first (unshifted) keysym
                let keysym = match chunk.first().copied() {
                    Some(k) => k,
                    None => continue,
                };
                let digit = match keysym {
                    0x0031 => "1",
                    0x0032 => "2",
                    0x0033 => "3",
                    _ => continue,
                };
                map.insert(keycode, digit);
            }
        }
    }
    map
}

#[cfg(target_os = "linux")]
fn modifier_from_state(state: x11rb::protocol::xproto::KeyButMask) -> &'static str {
    use x11rb::protocol::xproto::KeyButMask;
    if state.contains(KeyButMask::CONTROL) {
        "Ctrl"
    } else if state.contains(KeyButMask::SHIFT) {
        "Shift"
    } else {
        ""
    }
}

#[cfg(target_os = "linux")]
fn shortcut_from_keypress(
    keycode: u8,
    state: x11rb::protocol::xproto::KeyButMask,
    keycode_map: &std::collections::HashMap<u8, &'static str>,
) -> Option<String> {
    use x11rb::protocol::xproto::KeyButMask;
    let digit = keycode_map.get(&keycode)?;
    if state.contains(KeyButMask::CONTROL) {
        Some(format!("Ctrl+{digit}"))
    } else {
        None
    }
}
