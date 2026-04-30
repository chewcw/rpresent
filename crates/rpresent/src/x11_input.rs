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
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ButtonIndex, ConnectionExt as _, EventMask, GrabMode, ModMask};

    use crate::input::{ActiveMode, InputBindings, InputRouter};

    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    // Build the keycode map first so we can set up passive key grabs.
    let keycode_map: HashMap<u8, &'static str> = build_keycode_map(&conn);

    // Passive grab Ctrl+1/2/3 on the root window for global shortcut capture.
    // ChangeWindowAttributes with KEY_PRESS does NOT capture events over other windows;
    // GrabKey is required for global hotkey delivery.
    // Use common lock-mask variants so Ctrl+<key> still works when NumLock/CapsLock is active.
    let key_grab_modifiers = [
        ModMask::CONTROL,
        ModMask::CONTROL | ModMask::LOCK,
        ModMask::CONTROL | ModMask::M2,
        ModMask::CONTROL | ModMask::M2 | ModMask::LOCK,
        ModMask::CONTROL | ModMask::M5,
        ModMask::CONTROL | ModMask::M5 | ModMask::LOCK,
        ModMask::CONTROL | ModMask::M2 | ModMask::M5,
        ModMask::CONTROL | ModMask::M2 | ModMask::M5 | ModMask::LOCK,
    ];
    for &keycode in keycode_map.keys() {
        for modifiers in key_grab_modifiers {
            match conn.grab_key(
                false,
                root,
                modifiers,
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            ) {
                Ok(cookie) => {
                    if let Err(error) = cookie.check() {
                        tracing::warn!(
                            keycode,
                            ?modifiers,
                            ?error,
                            "failed to grab global key shortcut"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        keycode,
                        ?modifiers,
                        ?error,
                        "failed to request global key shortcut grab"
                    );
                }
            }
        }
    }

    // Passive grab Ctrl+wheel (buttons 4 and 5) globally on the root window.
    // ChangeWindowAttributes with BUTTON_PRESS only fires when the pointer is over the root
    // background; GrabButton is required to intercept wheel events over any window.
    let wheel_grab_modifiers = [
        ModMask::CONTROL,
        ModMask::CONTROL | ModMask::LOCK,
        ModMask::CONTROL | ModMask::M2,
        ModMask::CONTROL | ModMask::M2 | ModMask::LOCK,
        ModMask::CONTROL | ModMask::M5,
        ModMask::CONTROL | ModMask::M5 | ModMask::LOCK,
        ModMask::CONTROL | ModMask::M2 | ModMask::M5,
        ModMask::CONTROL | ModMask::M2 | ModMask::M5 | ModMask::LOCK,
    ];
    for button in [ButtonIndex::M4, ButtonIndex::M5] {
        for modifiers in wheel_grab_modifiers {
            match conn.grab_button(
                false,
                root,
                EventMask::BUTTON_PRESS,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                0u32, // confine_to: NONE
                0u32, // cursor: NONE
                button,
                modifiers,
            ) {
                Ok(cookie) => {
                    if let Err(error) = cookie.check() {
                        tracing::warn!(
                            ?button,
                            ?modifiers,
                            ?error,
                            "failed to grab global wheel shortcut"
                        );
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        ?button,
                        ?modifiers,
                        ?error,
                        "failed to request global wheel shortcut grab"
                    );
                }
            }
        }
    }

    conn.flush()?;
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
                if let Some(shortcut) = shortcut_from_keypress(ev.detail, ev.state, &keycode_map)
                    && let Some(cmd) = router.handle_key(&shortcut)
                {
                    let _ = tx.send(crate::input::AppEvent::Command(cmd));
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
    if let Ok(cookie) = connection.get_keyboard_mapping(min_keycode, count)
        && let Ok(reply) = cookie.reply()
    {
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
