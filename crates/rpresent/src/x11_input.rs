#[cfg(target_os = "linux")]
pub(crate) fn spawn(
    shortcuts: rpresent_config::ShortcutConfig,
    tx: std::sync::mpsc::Sender<super::app::AppEvent>,
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
    tx: std::sync::mpsc::Sender<super::app::AppEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
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

    let bindings = InputBindings::from(&shortcuts);
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    loop {
        let event = conn.wait_for_event()?;
        match event {
            Event::ButtonPress(ev) if ev.detail == 4 || ev.detail == 5 => {
                let modifier = modifier_from_state(ev.state);
                let delta = if ev.detail == 4 { 1.0_f32 } else { -1.0_f32 };
                if let Some(action) = router.handle_wheel(modifier, delta) {
                    let _ = tx.send(super::app::AppEvent::Input(action));
                }
            }
            Event::KeyPress(ev) => {
                if let Some(shortcut) = shortcut_from_keypress(&conn, ev.detail, ev.state) {
                    if let Some(cmd) = router.handle_key(&shortcut) {
                        let _ = tx.send(super::app::AppEvent::Command(cmd));
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
fn modifier_from_state(state: x11rb::protocol::xproto::KeyButMask) -> &'static str {
    use x11rb::protocol::xproto::KeyButMask;
    let mask = state;
    if mask.contains(KeyButMask::CONTROL) {
        "Ctrl"
    } else if mask.contains(KeyButMask::SHIFT) {
        "Shift"
    } else {
        ""
    }
}

#[cfg(target_os = "linux")]
fn shortcut_from_keypress(
    connection: &impl x11rb::connection::Connection,
    keycode: u8,
    state: x11rb::protocol::xproto::KeyButMask,
) -> Option<String> {
    use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask};
    let mapping = connection
        .get_keyboard_mapping(keycode, 1)
        .ok()?
        .reply()
        .ok()?;
    let keysym = *mapping.keysyms.first()?;
    let digit = match keysym {
        0x0031 => "1",
        0x0032 => "2",
        0x0033 => "3",
        _ => return None,
    };
    let mask = state;
    if mask.contains(KeyButMask::CONTROL) {
        Some(format!("Ctrl+{digit}"))
    } else {
        None
    }
}
