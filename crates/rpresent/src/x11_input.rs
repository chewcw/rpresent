use std::collections::HashMap;
use std::error::Error;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;

use rpresent_config::ConfigService;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ButtonIndex, ChangeWindowAttributesAux, ConnectionExt, EventMask, GrabMode, GrabStatus,
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
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    release_owner(&connection, &mut owner, &mut saved_focus, true);
                    return Ok(());
                }
            }
        }

        while let Some(event) = connection.poll_for_event()? {
            translate_event(
                &event,
                &connection,
                &mut keycode_map,
                &mut keys,
                &events,
                &mut owner,
                &mut saved_focus,
            );
        }
        connection.flush()?;

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
                    GrabMode::ASYNC,
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
    connection
        .set_input_focus(InputFocus::NONE, window, x11rb::CURRENT_TIME)?
        .check()?;
    connection.flush()?;
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
) {
    match event {
        Event::ButtonPress(event)
            if event.detail == BUTTON_WHEEL_UP || event.detail == BUTTON_WHEEL_DOWN =>
        {
            events(NativeEvent::Wheel {
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
            let epoch = owner.take().map_or(0, |owner| owner.epoch);
            release_owner(connection, owner, saved_focus, true);
            events(NativeEvent::Lost {
                epoch,
                reason: "owned window lost focus".to_owned(),
            });
        }
        Event::DestroyNotify(event) if owner.is_some_and(|owner| owner.window == event.window) => {
            let epoch = owner.take().map_or(0, |owner| owner.epoch);
            release_owner(connection, owner, saved_focus, true);
            events(NativeEvent::Lost {
                epoch,
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
