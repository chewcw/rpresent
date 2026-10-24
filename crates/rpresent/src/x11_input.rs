/// X11 button codes for mouse wheel scroll up/down.
#[cfg(target_os = "linux")]
const BUTTON_WHEEL_UP: u8 = 4;
#[cfg(target_os = "linux")]
const BUTTON_WHEEL_DOWN: u8 = 5;

#[cfg(target_os = "linux")]
pub(crate) fn spawn(
    config_service: std::sync::Arc<rpresent_config::ConfigService>,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
    zoom_handle: rpresent_zoom::ZoomHandle,
) {
    std::thread::spawn(move || {
        if let Err(e) = run_input_loop(config_service, tx, zoom_handle) {
            tracing::error!(error = %e, "X11 input loop failed");
        }
    });
}

#[cfg(target_os = "linux")]
fn run_input_loop(
    config_service: std::sync::Arc<rpresent_config::ConfigService>,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
    zoom_handle: rpresent_zoom::ZoomHandle,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ButtonIndex, ConnectionExt as _, EventMask, GrabMode};

    use crate::input::{ActiveMode, InputBindings, InputRouter};

    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    // Build the keycode map first so we can set up passive key grabs.
    let keycode_map: HashMap<u8, &'static str> = build_keycode_map(&conn);
    let bindings = InputBindings::from(config_service.shortcuts());

    // Passive-grab each configured shortcut on the root window.
    // ChangeWindowAttributes with KEY_PRESS does NOT capture events over other windows;
    // GrabKey is required for global hotkey delivery.
    // Use common lock-mask variants so Ctrl+<key> still works when NumLock/CapsLock is active.
    for (&keycode, digit) in &keycode_map {
        let shortcut = match *digit {
            "1" => &bindings.spotlight_mode,
            "2" => &bindings.annotation_mode,
            "3" => &bindings.zoom_mode,
            _ => continue,
        };
        for modifiers in modifier_masks(
            shortcut
                .split_once('+')
                .map_or("", |(modifier, _)| modifier),
        ) {
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

    // Passive-grab the configured wheel modifier globally on the root window.
    // ChangeWindowAttributes with BUTTON_PRESS only fires when the pointer is over the root
    // background; GrabButton is required to intercept wheel events over any window.
    let wheel_grab_modifiers = modifier_masks(&bindings.wheel_modifier);
    for button in [ButtonIndex::M4, ButtonIndex::M5] {
        for modifiers in wheel_grab_modifiers.iter().copied() {
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
                if let Some(action) = router.handle_wheel_with_zoom(
                    modifier,
                    delta,
                    ev.root_x,
                    ev.root_y,
                    zoom_handle.is_active(),
                ) {
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
    } else if state.contains(KeyButMask::MOD1) {
        "Alt"
    } else if state.contains(KeyButMask::MOD4) {
        "Super"
    } else {
        ""
    }
}

#[cfg(target_os = "linux")]
fn modifier_masks(name: &str) -> Vec<x11rb::protocol::xproto::ModMask> {
    use x11rb::protocol::xproto::ModMask;
    let base = match name {
        "Ctrl" | "Control" => ModMask::CONTROL,
        "Shift" => ModMask::SHIFT,
        "Alt" => ModMask::M1,
        "Super" | "Meta" => ModMask::M4,
        other => {
            tracing::warn!(
                modifier = other,
                "unsupported shortcut modifier; using no modifier"
            );
            ModMask::from(0_u8)
        }
    };
    [
        base,
        base | ModMask::LOCK,
        base | ModMask::M2,
        base | ModMask::M2 | ModMask::LOCK,
        base | ModMask::M5,
        base | ModMask::M5 | ModMask::LOCK,
        base | ModMask::M2 | ModMask::M5,
        base | ModMask::M2 | ModMask::M5 | ModMask::LOCK,
    ]
    .into_iter()
    .collect()
}

#[cfg(target_os = "linux")]
fn shortcut_from_keypress(
    keycode: u8,
    state: x11rb::protocol::xproto::KeyButMask,
    keycode_map: &std::collections::HashMap<u8, &'static str>,
) -> Option<String> {
    let digit = keycode_map.get(&keycode)?;
    let modifier = modifier_from_state(state);
    if modifier.is_empty() {
        None
    } else {
        Some(format!("{modifier}+{digit}"))
    }
}
