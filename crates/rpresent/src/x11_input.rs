use std::collections::HashMap;
use std::error::Error;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;

use rpresent_config::ConfigService;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Allow, ButtonIndex, ChangeWindowAttributesAux, ConnectionExt, EventMask, GrabMode, GrabStatus,
    InputFocus, KeyButMask, ModMask, StackMode, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::input::{InputBindings, RawPointer};
use crate::native_input::{NativeEvent, NativeEventSink, NativeInbox, NativeRequest};
use crate::native_keys::NativeKeys;

const BUTTON_WHEEL_UP: u8 = 4;
const BUTTON_WHEEL_DOWN: u8 = 5;

pub(crate) fn spawn(
    config_service: Arc<ConfigService>,
    inbox: NativeInbox,
    events: NativeEventSink,
) {
    std::thread::spawn(move || {
        let result = run_input_loop(config_service, inbox, Arc::clone(&events));
        match &result {
            Ok(()) => events(NativeEvent::Exited(Ok(()))),
            Err(error) => events(NativeEvent::Exited(Err(error.to_string()))),
        }
    });
}

fn run_input_loop(
    config_service: Arc<ConfigService>,
    mut inbox: NativeInbox,
    events: NativeEventSink,
) -> Result<(), Box<dyn Error>> {
    let (connection, screen_num) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen_num].root;
    let mut keycode_map = build_keycode_map(&connection);
    let mut keys = NativeKeys::new().map_err(std::io::Error::other)?;
    let keys_fd = keys.fd();
    let bindings = InputBindings::from(config_service.shortcuts());
    install_passive_grabs(&connection, root, &keycode_map, &bindings)?;
    connection.flush()?;
    events(NativeEvent::Ready);

    let x11_fd = connection.stream().as_raw_fd();
    let mut owner = None;
    let mut saved_focus = None;
    let mut pending_wheel = None;

    loop {
        inbox.drain_wake()?;
        loop {
            match inbox.try_recv() {
                Ok(NativeRequest::Restack {
                    operation,
                    bottom_to_top,
                }) => {
                    let result =
                        restack(&connection, bottom_to_top).map_err(|error| error.to_string());
                    events(NativeEvent::Applied { operation, result });
                }
                Ok(NativeRequest::SetOwner {
                    operation,
                    window,
                    restore_focus,
                }) => {
                    if let Some(time) = pending_wheel {
                        resolve_wheel(&connection, &mut pending_wheel, time, false)?;
                    }
                    let result = set_owner(
                        &connection,
                        &mut owner,
                        &mut saved_focus,
                        window,
                        restore_focus,
                        operation,
                    )
                    .map_err(|error| error.to_string());
                    events(NativeEvent::Applied { operation, result });
                }
                Ok(NativeRequest::ResolveWheel { time, replay }) => {
                    resolve_wheel(&connection, &mut pending_wheel, time, replay)?;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    if let Some(time) = pending_wheel {
                        resolve_wheel(&connection, &mut pending_wheel, time, false)?;
                    }
                    release_owner(&connection, &mut owner, &mut saved_focus, true);
                    return Ok(());
                }
            }
        }

        loop {
            let Some(event) = (match connection.poll_for_event() {
                Ok(event) => event,
                Err(error) => {
                    if let Some(time) = pending_wheel {
                        let _ = resolve_wheel(&connection, &mut pending_wheel, time, false);
                    }
                    release_owner(&connection, &mut owner, &mut saved_focus, true);
                    return Err(error.into());
                }
            }) else {
                break;
            };
            translate_event(
                &event,
                &connection,
                &mut keycode_map,
                &mut keys,
                &events,
                &mut owner,
                &mut saved_focus,
                &mut pending_wheel,
            );
        }
        if let Err(error) = connection.flush() {
            if let Some(time) = pending_wheel {
                let _ = resolve_wheel(&connection, &mut pending_wheel, time, false);
            }
            release_owner(&connection, &mut owner, &mut saved_focus, true);
            return Err(error.into());
        }

        let mut descriptors = [
            libc::pollfd {
                fd: x11_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: inbox.wake_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: keys_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), 3, -1) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error.into());
        }
        if descriptors[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            release_owner(&connection, &mut owner, &mut saved_focus, true);
            return Err("X11 connection closed".into());
        }
        if descriptors[2].revents & libc::POLLIN != 0 {
            keys.drain_map_notifications();
        }
    }
}

fn install_passive_grabs(
    connection: &RustConnection,
    root: Window,
    keycode_map: &HashMap<u8, &'static str>,
    bindings: &InputBindings,
) -> Result<(), Box<dyn Error>> {
    for (&keycode, digit) in keycode_map {
        let shortcut = match *digit {
            "1" => &bindings.spotlight_mode,
            "2" => &bindings.annotation_mode,
            "3" => &bindings.zoom_mode,
            _ => continue,
        };
        let modifier = shortcut.split_once('+').map_or("", |(name, _)| name);
        for modifiers in modifier_masks(modifier) {
            connection
                .grab_key(
                    false,
                    root,
                    modifiers,
                    keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()?;
        }
    }

    for button in [ButtonIndex::M4, ButtonIndex::M5] {
        for modifiers in modifier_masks(&bindings.wheel_modifier) {
            connection
                .grab_button(
                    false,
                    root,
                    EventMask::BUTTON_PRESS,
                    GrabMode::SYNC,
                    GrabMode::ASYNC,
                    0_u32,
                    0_u32,
                    button,
                    modifiers,
                )?
                .check()?;
        }
    }
    Ok(())
}

fn resolve_wheel(
    connection: &RustConnection,
    pending: &mut Option<u32>,
    time: u32,
    replay: bool,
) -> Result<(), Box<dyn Error>> {
    if *pending != Some(time) {
        return Ok(());
    }
    connection
        .allow_events(
            if replay {
                Allow::REPLAY_POINTER
            } else {
                Allow::ASYNC_POINTER
            },
            time,
        )?
        .check()?;
    connection.flush()?;
    *pending = None;
    Ok(())
}

fn restack(connection: &impl Connection, windows: [Option<u32>; 3]) -> Result<(), Box<dyn Error>> {
    for window in windows.into_iter().flatten() {
        connection
            .configure_window(
                window,
                &x11rb::protocol::xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?
            .check()?;
    }
    connection.flush()?;
    Ok(())
}

fn set_owner(
    connection: &RustConnection,
    owner: &mut Option<Owner>,
    saved_focus: &mut Option<Window>,
    window: Option<u32>,
    restore_focus: bool,
    epoch: u64,
) -> Result<(), Box<dyn Error>> {
    release_owner(connection, owner, saved_focus, false);
    let Some(window) = window else {
        if restore_focus {
            restore_focus_window(connection, saved_focus);
        }
        connection.flush()?;
        return Ok(());
    };

    if saved_focus.is_none() {
        let focus = connection.get_input_focus()?.reply()?.focus;
        if focus != 0 {
            *saved_focus = Some(focus);
        }
    }

    connection
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(
                EventMask::KEY_PRESS
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::STRUCTURE_NOTIFY
                    | EventMask::FOCUS_CHANGE,
            ),
        )?
        .check()?;
    let reply = connection
        .grab_pointer(
            false,
            window,
            EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            0_u32,
            0_u32,
            x11rb::CURRENT_TIME,
        )?
        .reply()?;
    if reply.status != GrabStatus::SUCCESS {
        return Err(format!("pointer grab failed with status {:?}", reply.status).into());
    }
    let finish: Result<(), Box<dyn Error>> = (|| {
        connection
            .set_input_focus(InputFocus::NONE, window, x11rb::CURRENT_TIME)?
            .check()?;
        connection.flush()?;
        Ok(())
    })();
    if let Err(error) = finish {
        let _ = connection.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = connection.flush();
        return Err(error);
    }
    *owner = Some(Owner { window, epoch });
    Ok(())
}

fn release_owner(
    connection: &RustConnection,
    owner: &mut Option<Owner>,
    saved_focus: &mut Option<Window>,
    restore_focus: bool,
) {
    if owner.take().is_some() {
        let _ = connection.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = connection.flush();
    }
    if restore_focus {
        restore_focus_window(connection, saved_focus);
    }
}

fn restore_focus_window(connection: &RustConnection, saved_focus: &mut Option<Window>) {
    let Some(window) = saved_focus.take() else {
        return;
    };
    if let Ok(cookie) = connection.get_geometry(window)
        && cookie.reply().is_ok()
    {
        if let Ok(cookie) =
            connection.set_input_focus(InputFocus::NONE, window, x11rb::CURRENT_TIME)
        {
            let _ = cookie.check();
        }
        let _ = connection.flush();
    }
}

#[derive(Debug, Clone, Copy)]
struct Owner {
    window: Window,
    epoch: u64,
}

#[allow(clippy::too_many_arguments)]
fn translate_event(
    event: &Event,
    connection: &RustConnection,
    keycode_map: &mut HashMap<u8, &'static str>,
    keys: &mut NativeKeys,
    events: &NativeEventSink,
    owner: &mut Option<Owner>,
    saved_focus: &mut Option<Window>,
    pending_wheel: &mut Option<u32>,
) {
    match event {
        Event::ButtonPress(event)
            if event.detail == BUTTON_WHEEL_UP || event.detail == BUTTON_WHEEL_DOWN =>
        {
            let epoch = owner.as_ref().map(|owner| owner.epoch);
            if epoch.is_none() {
                *pending_wheel = Some(event.time);
            }
            events(NativeEvent::Wheel {
                epoch,
                time: event.time,
                modifier: modifier_from_state(event.state).to_owned(),
                delta: if event.detail == BUTTON_WHEEL_UP {
                    1.0
                } else {
                    -1.0
                },
                root: (event.root_x, event.root_y),
            });
        }
        Event::ButtonPress(event) if event.detail == 1 => {
            if let Some(owner) = owner {
                events(NativeEvent::Pointer {
                    epoch: owner.epoch,
                    input: RawPointer::Down {
                        root: (i32::from(event.root_x), i32::from(event.root_y)),
                        time: Duration::from_millis(u64::from(event.time)),
                    },
                });
            }
        }
        Event::ButtonRelease(event) if event.detail == 1 => {
            if let Some(owner) = owner {
                events(NativeEvent::Pointer {
                    epoch: owner.epoch,
                    input: RawPointer::Up {
                        root: (i32::from(event.root_x), i32::from(event.root_y)),
                        time: Duration::from_millis(u64::from(event.time)),
                    },
                });
            }
        }
        Event::MotionNotify(event) => {
            if let Some(owner) = owner {
                events(NativeEvent::Pointer {
                    epoch: owner.epoch,
                    input: RawPointer::Move {
                        root: (i32::from(event.root_x), i32::from(event.root_y)),
                        time: Duration::from_millis(u64::from(event.time)),
                    },
                });
            }
        }
        Event::KeyPress(event) => {
            if let Some(shortcut) = shortcut_from_keypress(event.detail, event.state, keycode_map) {
                events(NativeEvent::Key(shortcut));
                return;
            }
            let Some(owner) = *owner else {
                if keycode_map.get(&event.detail) == Some(&"Escape") {
                    events(NativeEvent::Escape);
                }
                return;
            };
            match keys.translate(event.detail, u16::from(event.state)) {
                Ok(input) => events(NativeEvent::AnnotationKey {
                    epoch: owner.epoch,
                    input,
                }),
                Err(error) => {
                    tracing::debug!(%error, keycode = event.detail, "key translation failed");
                }
            }
        }
        Event::FocusOut(event) if owner.is_some_and(|owner| owner.window == event.event) => {
            let focused = connection
                .get_input_focus()
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| reply.focus);
            if focused == Some(event.event) {
                return;
            }
            let lost = *owner
                .as_ref()
                .expect("the focus event matched the native owner");
            release_owner(connection, owner, saved_focus, true);
            events(NativeEvent::Lost {
                epoch: lost.epoch,
                window: lost.window,
                destroyed: false,
                reason: "owned window lost focus".to_owned(),
            });
        }
        Event::DestroyNotify(event) if owner.is_some_and(|owner| owner.window == event.window) => {
            let lost = *owner
                .as_ref()
                .expect("the destroy event matched the native owner");
            release_owner(connection, owner, saved_focus, true);
            events(NativeEvent::Lost {
                epoch: lost.epoch,
                window: lost.window,
                destroyed: true,
                reason: "owned window was destroyed".to_owned(),
            });
        }
        Event::MappingNotify(_) => {
            *keycode_map = build_keycode_map(connection);
        }
        _ => {}
    }
}

fn build_keycode_map(connection: &impl Connection) -> HashMap<u8, &'static str> {
    let setup = connection.setup();
    let count = setup.max_keycode - setup.min_keycode + 1;
    let mut map = HashMap::new();
    if let Ok(cookie) = connection.get_keyboard_mapping(setup.min_keycode, count)
        && let Ok(reply) = cookie.reply()
    {
        let per_keycode = usize::from(reply.keysyms_per_keycode);
        for (index, chunk) in reply.keysyms.chunks(per_keycode).enumerate() {
            let Some(keysym) = chunk.first().copied() else {
                continue;
            };
            let value = match keysym {
                0x0031 => "1",
                0x0032 => "2",
                0x0033 => "3",
                0xff1b => "Escape",
                _ => continue,
            };
            map.insert(setup.min_keycode + index as u8, value);
        }
    }
    map
}

fn modifier_from_state(state: KeyButMask) -> &'static str {
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

fn modifier_masks(name: &str) -> Vec<ModMask> {
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

fn shortcut_from_keypress(
    keycode: u8,
    state: KeyButMask,
    keycode_map: &HashMap<u8, &'static str>,
) -> Option<String> {
    let digit = keycode_map.get(&keycode)?;
    if *digit == "Escape" {
        return None;
    }
    let modifier = modifier_from_state(state);
    (!modifier.is_empty()).then(|| format!("{modifier}+{digit}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpresent_config::ShortcutConfig;
    use x11rb::protocol::shape::{ConnectionExt as ShapeExt, SK, SO};
    use x11rb::protocol::xproto::{
        BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ClipOrdering, CreateWindowAux, KEY_PRESS_EVENT,
        KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, WindowClass,
    };
    use x11rb::protocol::xtest::ConnectionExt as XTestExt;

    fn connect_test() -> (RustConnection, u32) {
        assert_eq!(
            std::env::var("RPRESENT_NATIVE_TEST").as_deref(),
            Ok("1"),
            "run native input tests through the private xvfb-run command"
        );
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        (connection, root)
    }

    fn window(connection: &RustConnection, root: u32, empty_input: bool) -> u32 {
        let id = connection.generate_id().unwrap();
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id,
                root,
                0,
                0,
                240,
                240,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new().event_mask(
                    EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::POINTER_MOTION
                        | EventMask::FOCUS_CHANGE
                        | EventMask::STRUCTURE_NOTIFY,
                ),
            )
            .unwrap()
            .check()
            .unwrap();
        if empty_input {
            connection
                .shape_rectangles(SO::SET, SK::INPUT, ClipOrdering::UNSORTED, id, 0, 0, &[])
                .unwrap()
                .check()
                .unwrap();
        }
        connection.map_window(id).unwrap().check().unwrap();
        connection.flush().unwrap();
        id
    }

    fn inject(connection: &RustConnection, root: u32, kind: u8, detail: u8, x: i16, y: i16) {
        connection
            .xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, root, x, y, 0)
            .unwrap()
            .check()
            .unwrap();
        connection.flush().unwrap();
    }

    fn drain(connection: &RustConnection) -> Vec<Event> {
        connection.get_input_focus().unwrap().reply().unwrap();
        let mut events = Vec::new();
        while let Some(event) = connection.poll_for_event().unwrap() {
            events.push(event);
        }
        events
    }

    fn wait_for(connection: &RustConnection, predicate: impl Fn(&Event) -> bool) -> Vec<Event> {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut collected = Vec::new();
        loop {
            collected.extend(drain(connection));
            if collected.iter().any(&predicate) {
                return collected;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "native event timed out"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn control_key(connection: &RustConnection) -> u8 {
        let setup = connection.setup();
        let mapping = connection
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .unwrap()
            .reply()
            .unwrap();
        let width = usize::from(mapping.keysyms_per_keycode);
        mapping
            .keysyms
            .chunks(width)
            .enumerate()
            .find(|(_, symbols)| symbols.contains(&0xffe3))
            .map(|(offset, _)| setup.min_keycode + offset as u8)
            .expect("private X server must have Control_L")
    }

    #[test]
    #[ignore = "requires a private Xvfb display"]
    fn native_empty_shape_grab_consumes_pointer_and_releases() {
        let (observer, root) = connect_test();
        let desktop = window(&observer, root, false);
        let (connection, owner_root) = connect_test();
        let spot = window(&connection, owner_root, true);
        observer
            .set_input_focus(InputFocus::NONE, desktop, x11rb::CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();

        let mut owner = None;
        let mut saved = None;
        set_owner(&connection, &mut owner, &mut saved, Some(spot), false, 41).unwrap();
        let competing = observer
            .grab_pointer(
                false,
                desktop,
                EventMask::BUTTON_PRESS,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                0_u32,
                0_u32,
                x11rb::CURRENT_TIME,
            )
            .unwrap()
            .reply()
            .unwrap();
        assert_ne!(competing.status, GrabStatus::SUCCESS);

        drain(&observer);
        drain(&connection);
        inject(&connection, root, MOTION_NOTIFY_EVENT, 0, 50, 50);
        for button in [1, 3, 4, 5, 6, 7] {
            inject(&connection, root, BUTTON_PRESS_EVENT, button, 0, 0);
            inject(&connection, root, BUTTON_RELEASE_EVENT, button, 0, 0);
        }
        let events = wait_for(
            &connection,
            |event| matches!(event, Event::ButtonRelease(event) if event.detail == 7),
        );
        assert!(
            events
                .iter()
                .any(|event| { matches!(event, Event::ButtonPress(event) if event.detail == 1) })
        );
        assert!(
            !drain(&observer)
                .iter()
                .any(|event| { matches!(event, Event::ButtonPress(_) | Event::ButtonRelease(_)) })
        );

        release_owner(&connection, &mut owner, &mut saved, true);
        assert!(owner.is_none());
        assert_eq!(
            observer.get_input_focus().unwrap().reply().unwrap().focus,
            desktop
        );
        let after = observer
            .grab_pointer(
                false,
                desktop,
                EventMask::BUTTON_PRESS,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                0_u32,
                0_u32,
                x11rb::CURRENT_TIME,
            )
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(after.status, GrabStatus::SUCCESS);
        observer
            .ungrab_pointer(x11rb::CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
    }

    #[test]
    #[ignore = "requires a private Xvfb display"]
    fn native_desktop_wheel_replays_once() {
        let (observer, root) = connect_test();
        let desktop = window(&observer, root, false);
        let (connection, _) = connect_test();
        let bindings = InputBindings::from(&ShortcutConfig::default());
        install_passive_grabs(&connection, root, &HashMap::new(), &bindings).unwrap();
        connection.flush().unwrap();
        let ctrl = control_key(&connection);

        inject(&connection, root, MOTION_NOTIFY_EVENT, 0, 50, 50);
        inject(&connection, root, KEY_PRESS_EVENT, ctrl, 0, 0);
        inject(&connection, root, BUTTON_PRESS_EVENT, 4, 0, 0);
        let root_events = wait_for(
            &connection,
            |event| matches!(event, Event::ButtonPress(event) if event.detail == 4),
        );
        let time = root_events
            .iter()
            .find_map(|event| {
                if let Event::ButtonPress(event) = event {
                    (event.detail == 4).then_some(event.time)
                } else {
                    None
                }
            })
            .unwrap();
        let mut pending = Some(time);
        resolve_wheel(&connection, &mut pending, time, true).unwrap();
        assert_eq!(pending, None);

        inject(&connection, root, BUTTON_RELEASE_EVENT, 4, 0, 0);
        inject(&connection, root, KEY_RELEASE_EVENT, ctrl, 0, 0);
        let delivered = wait_for(
            &observer,
            |event| matches!(event, Event::ButtonRelease(event) if event.detail == 4),
        );
        assert_eq!(
            delivered
                .iter()
                .filter(|event| {
                    matches!(event, Event::ButtonPress(event)
                        if event.event == desktop && event.detail == 4)
                })
                .count(),
            1
        );
    }

    #[test]
    #[ignore = "requires a private Xvfb display"]
    fn native_stale_resolution_cannot_thaw_another_wheel() {
        let (connection, _) = connect_test();
        let mut pending = Some(200);
        resolve_wheel(&connection, &mut pending, 199, true).unwrap();
        assert_eq!(pending, Some(200));
    }

    #[test]
    #[ignore = "requires a private Xvfb display"]
    fn native_denied_grab_does_not_publish_an_owner() {
        let (competitor, root) = connect_test();
        let desktop = window(&competitor, root, false);
        let (connection, _) = connect_test();
        let spot = window(&connection, root, true);
        let held = competitor
            .grab_pointer(
                false,
                desktop,
                EventMask::BUTTON_PRESS,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                0_u32,
                0_u32,
                x11rb::CURRENT_TIME,
            )
            .unwrap()
            .reply()
            .unwrap();
        assert_eq!(held.status, GrabStatus::SUCCESS);
        let mut owner = None;
        let mut saved = None;
        assert!(set_owner(&connection, &mut owner, &mut saved, Some(spot), false, 42,).is_err());
        assert!(owner.is_none());
        competitor
            .ungrab_pointer(x11rb::CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        set_owner(&connection, &mut owner, &mut saved, Some(spot), false, 43).unwrap();
        assert_eq!(owner.as_ref().unwrap().epoch, 43);
        release_owner(&connection, &mut owner, &mut saved, true);
    }

    #[test]
    #[ignore = "requires a private Xvfb display"]
    fn native_loss_releases_the_pointer_before_reporting() {
        let (observer, root) = connect_test();
        let (connection, _) = connect_test();
        for destroyed in [false, true] {
            let desktop = window(&observer, root, false);
            let spot = window(&connection, root, true);
            observer
                .set_input_focus(InputFocus::NONE, desktop, x11rb::CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap();
            let mut owner = None;
            let mut saved = None;
            set_owner(&connection, &mut owner, &mut saved, Some(spot), false, 44).unwrap();
            drain(&connection);
            if destroyed {
                connection.destroy_window(spot).unwrap().check().unwrap();
            } else {
                observer
                    .set_input_focus(InputFocus::NONE, desktop, x11rb::CURRENT_TIME)
                    .unwrap()
                    .check()
                    .unwrap();
            }
            let native_events = wait_for(&connection, |event| {
                if destroyed {
                    matches!(event, Event::DestroyNotify(event) if event.window == spot)
                } else {
                    matches!(event, Event::FocusOut(event) if event.event == spot)
                }
            });
            let loss = native_events
                .iter()
                .find(|event| {
                    if destroyed {
                        matches!(event, Event::DestroyNotify(event) if event.window == spot)
                    } else {
                        matches!(event, Event::FocusOut(event) if event.event == spot)
                    }
                })
                .unwrap();
            let reported = Arc::new(std::sync::Mutex::new(Vec::new()));
            let capture = Arc::clone(&reported);
            let sink: NativeEventSink = Arc::new(move |event| {
                capture.lock().unwrap().push(event);
            });
            let mut keycodes = HashMap::new();
            let mut keys = NativeKeys::new().unwrap();
            let mut pending = None;
            translate_event(
                loss,
                &connection,
                &mut keycodes,
                &mut keys,
                &sink,
                &mut owner,
                &mut saved,
                &mut pending,
            );
            assert!(owner.is_none());
            assert!(reported.lock().unwrap().iter().any(|event| {
                matches!(event, NativeEvent::Lost {
                    epoch: 44,
                    window,
                    destroyed: actual,
                    ..
                } if *window == spot && *actual == destroyed)
            }));
            let next = observer
                .grab_pointer(
                    false,
                    desktop,
                    EventMask::BUTTON_PRESS,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    0_u32,
                    0_u32,
                    x11rb::CURRENT_TIME,
                )
                .unwrap()
                .reply()
                .unwrap();
            assert_eq!(next.status, GrabStatus::SUCCESS);
            observer
                .ungrab_pointer(x11rb::CURRENT_TIME)
                .unwrap()
                .check()
                .unwrap();
        }
    }
}
