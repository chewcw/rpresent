/// X11 button codes for mouse wheel scroll up/down.
#[cfg(target_os = "linux")]
const BUTTON_WHEEL_UP: u8 = 4;
#[cfg(target_os = "linux")]
const BUTTON_WHEEL_DOWN: u8 = 5;
#[cfg(target_os = "linux")]
const BUTTON_LEFT: u8 = 1;

#[cfg(target_os = "linux")]
pub(crate) fn spawn(
    shortcuts: rpresent_config::ShortcutConfig,
    active_mode: crate::input::ActiveModeState,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
) {
    std::thread::spawn(move || {
        if let Err(e) = run_input_loop(shortcuts, active_mode, tx) {
            tracing::error!(error = %e, "X11 input loop failed");
        }
    });
}

#[cfg(target_os = "linux")]
fn run_input_loop(
    shortcuts: rpresent_config::ShortcutConfig,
    active_mode: crate::input::ActiveModeState,
    tx: std::sync::mpsc::Sender<crate::input::AppEvent>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::HashMap;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ButtonIndex, ConnectionExt as _, EventMask, GrabMode, ModMask};

    use crate::input::{InputBindings, InputRouter};

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

    // Passive grab the configured wheel shortcut globally on the root window.
    // ChangeWindowAttributes with BUTTON_PRESS only fires when the pointer is over the root
    // background; GrabButton is required to intercept wheel events over any window.
    let wheel_grab_modifiers = wheel_grab_modifiers_for_config(&shortcuts.wheel_modifier);
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

    // Capture Button1 drag lifecycle so zoom mode can pan via MotionNotify events.
    match conn.grab_button(
        true,
        root,
        EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
        GrabMode::ASYNC,
        GrabMode::ASYNC,
        0u32,
        0u32,
        ButtonIndex::M1,
        ModMask::ANY,
    ) {
        Ok(cookie) => {
            if let Err(error) = cookie.check() {
                tracing::warn!(?error, "failed to grab global Button1 drag events");
            }
        }
        Err(error) => {
            tracing::warn!(?error, "failed to request global Button1 drag grab");
        }
    }

    conn.flush()?;
    let bindings = InputBindings::from(&shortcuts);
    let router_active_mode = active_mode.clone();
    let mut router = InputRouter::new(bindings, router_active_mode);
    let mut drag_active = false;

    let grab_watcher_mode = active_mode.clone();
    std::thread::spawn(move || {
        if let Err(error) = run_zoom_wheel_grab_watcher(grab_watcher_mode) {
            tracing::error!(error = %error, "zoom wheel grab watcher failed");
        }
    });

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
                if let Some(action) = router.handle_wheel(modifier, delta, ev.root_x, ev.root_y) {
                    let _ = tx.send(crate::input::AppEvent::Input(action));
                }
            }
            Event::ButtonPress(ev) if ev.detail == BUTTON_LEFT => {
                if let Some(action) = router.handle_drag_start(ev.root_x, ev.root_y) {
                    drag_active = true;
                    let _ = tx.send(crate::input::AppEvent::Input(action));
                }
            }
            Event::MotionNotify(ev) => {
                if drag_active
                    && let Some(action) = router.handle_drag_move(ev.root_x, ev.root_y)
                {
                    let _ = tx.send(crate::input::AppEvent::Input(action));
                }
            }
            Event::ButtonRelease(ev) if ev.detail == BUTTON_LEFT => {
                if drag_active {
                    drag_active = false;
                    if let Some(action) = router.handle_drag_end() {
                        let _ = tx.send(crate::input::AppEvent::Input(action));
                    }
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
fn run_zoom_wheel_grab_watcher(
    active_mode: crate::input::ActiveModeState,
) -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection as _;

    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    let (mode, mut generation) = active_mode.snapshot();
    let mut zoom_wheel_grabbed = false;
    if sync_zoom_wheel_grab(&conn, root, &mut zoom_wheel_grabbed, mode)? {
        active_mode.mark_applied(generation);
    }
    active_mode.enable_sync_wait();

    loop {
        let (next_mode, next_generation) = active_mode.wait_for_change(generation);
        if sync_zoom_wheel_grab(&conn, root, &mut zoom_wheel_grabbed, next_mode)? {
            active_mode.mark_applied(next_generation);
        }
        generation = next_generation;
    }
}

#[cfg(target_os = "linux")]
fn sync_zoom_wheel_grab(
    connection: &impl x11rb::connection::Connection,
    root: x11rb::protocol::xproto::Window,
    zoom_wheel_grabbed: &mut bool,
    active_mode: crate::input::ActiveMode,
) -> Result<bool, Box<dyn std::error::Error>> {
    use x11rb::protocol::xproto::{ButtonIndex, ConnectionExt as _, EventMask, GrabMode};

    let should_grab = active_mode == crate::input::ActiveMode::Zoom;
    if should_grab == *zoom_wheel_grabbed {
        return Ok(true);
    }

    let mut all_synced = true;

    for button in [ButtonIndex::M4, ButtonIndex::M5] {
        for modifiers in zoom_wheel_grab_modifiers() {
            let result = if should_grab {
                connection.grab_button(
                    false,
                    root,
                    EventMask::BUTTON_PRESS,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    0u32,
                    0u32,
                    button,
                    modifiers,
                )
            } else {
                connection.ungrab_button(button, root, modifiers)
            };

            match result {
                Ok(cookie) => {
                    if let Err(error) = cookie.check() {
                        all_synced = false;
                        tracing::warn!(
                            ?button,
                            ?modifiers,
                            ?error,
                            "failed to sync zoom wheel shortcut"
                        );
                    }
                }
                Err(error) => {
                    all_synced = false;
                    tracing::warn!(
                        ?button,
                        ?modifiers,
                        ?error,
                        "failed to sync zoom wheel shortcut"
                    );
                }
            }
        }
    }

    connection.flush()?;
    *zoom_wheel_grabbed = should_grab;
    Ok(all_synced)
}

#[cfg(target_os = "linux")]
fn zoom_wheel_grab_modifiers() -> [x11rb::protocol::xproto::ModMask; 8] {
    use x11rb::protocol::xproto::ModMask;

    [
        ModMask::from(0u8),
        ModMask::LOCK,
        ModMask::M2,
        ModMask::M2 | ModMask::LOCK,
        ModMask::M5,
        ModMask::M5 | ModMask::LOCK,
        ModMask::M2 | ModMask::M5,
        ModMask::M2 | ModMask::M5 | ModMask::LOCK,
    ]
}

#[cfg(target_os = "linux")]
fn wheel_grab_modifiers_for_config(
    wheel_modifier: &str,
) -> [x11rb::protocol::xproto::ModMask; 8] {
    use x11rb::protocol::xproto::ModMask;

    let base = match normalize_wheel_modifier(wheel_modifier) {
        "Ctrl" => ModMask::CONTROL,
        "Shift" => ModMask::SHIFT,
        "Alt" => ModMask::M1,
        "Meta" | "Super" => ModMask::M4,
        other => {
            tracing::warn!(modifier = other, "unsupported wheel modifier; defaulting to Control");
            ModMask::CONTROL
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
}

#[cfg(target_os = "linux")]
fn normalize_wheel_modifier(raw: &str) -> &str {
    match raw {
        "Control" => "Ctrl",
        other => other,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{modifier_from_state, wheel_grab_modifiers_for_config, zoom_wheel_grab_modifiers};
    use x11rb::protocol::xproto::{KeyButMask, ModMask};

    #[test]
    fn zoom_wheel_grab_modifiers_cover_lock_variants() {
        let modifiers = zoom_wheel_grab_modifiers();

        assert!(modifiers.contains(&ModMask::from(0u8)));
        assert!(modifiers.contains(&ModMask::LOCK));
        assert!(modifiers.contains(&(ModMask::M2 | ModMask::LOCK)));
        assert!(modifiers.contains(&(ModMask::M5 | ModMask::LOCK)));
    }

    #[test]
    fn configured_wheel_grab_modifiers_follow_the_modifier_name() {
        let modifiers = wheel_grab_modifiers_for_config("Alt");

        assert!(modifiers.contains(&ModMask::M1));
        assert!(modifiers.contains(&(ModMask::M1 | ModMask::LOCK)));
    }

    #[test]
    fn modifier_from_state_maps_alt_and_super() {
        assert_eq!(modifier_from_state(KeyButMask::MOD1), "Alt");
        assert_eq!(modifier_from_state(KeyButMask::MOD4), "Super");
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
    } else if state.contains(KeyButMask::MOD1) {
        "Alt"
    } else if state.contains(KeyButMask::MOD4) {
        "Super"
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
