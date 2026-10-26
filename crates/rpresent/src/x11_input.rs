use std::error::Error;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;

use rpresent_config::ConfigService;
use rpresent_config::shortcuts::{ShortcutBackend, ShortcutEventSink};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xkb;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt, EventMask, GrabMode, GrabStatus, InputFocus,
    StackMode, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::input::{InputBindings, RawPointer};
use crate::native_input::{NativeEvent, NativeEventSink, NativeInbox, NativeRequest};
use crate::x11_shortcuts::X11ShortcutBackend;

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
    let connection = Arc::new(connection);
    let bindings = InputBindings::from(config_service.bindings());
    let mut backend = X11ShortcutBackend::new(Arc::clone(&connection), root)?;
    let shortcut_events: ShortcutEventSink = {
        let events = Arc::clone(&events);
        Arc::new(move |event| events(NativeEvent::Shortcut(event)))
    };
    backend.register(&bindings.global, bindings.wheel_modifier, shortcut_events)?;
    connection.flush()?;
    events(NativeEvent::Ready);

    let x11_fd = connection.stream().as_raw_fd();
    let mut owner = None;
    let mut saved_focus = None;

    loop {
        if let Err(error) = inbox.drain_wake() {
            exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
            return Err(error.into());
        }
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
                Ok(NativeRequest::ResolveWheel { token, replay }) => {
                    if let Err(error) = backend.resolve_wheel(token, replay) {
                        exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
                        return Err(error.into());
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
                    return Ok(());
                }
            }
        }

        loop {
            let Some(event) = (match connection.poll_for_event() {
                Ok(event) => event,
                Err(error) => {
                    exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
                    return Err(error.into());
                }
            }) else {
                break;
            };
            if let Err(error) = translate_event(
                &event,
                &connection,
                &mut backend,
                &events,
                &mut owner,
                &mut saved_focus,
            ) {
                exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
                return Err(error);
            }
        }
        if let Err(error) = connection.flush() {
            exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
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
        ];
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, -1) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
            return Err(error.into());
        }
        if descriptors[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            exit_loop(&mut backend, &connection, &mut owner, &mut saved_focus);
            return Err("X11 connection closed".into());
        }
    }
}

/// Release shortcut grabs and native pointer ownership before the loop exits.
fn exit_loop(
    backend: &mut X11ShortcutBackend,
    connection: &RustConnection,
    owner: &mut Option<Owner>,
    saved_focus: &mut Option<Window>,
) {
    let _ = backend.unregister();
    release_owner(connection, owner, saved_focus, true);
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
    backend: &mut X11ShortcutBackend,
    events: &NativeEventSink,
    owner: &mut Option<Owner>,
    saved_focus: &mut Option<Window>,
) -> Result<(), Box<dyn Error>> {
    match event {
        Event::KeyPress(_press) => {
            let epoch = owner.as_ref().map(|owner| owner.epoch);
            backend.handle_event(event, epoch)?;
        }
        Event::ButtonPress(press) => {
            let epoch = owner.as_ref().map(|owner| owner.epoch);
            let consumed = backend.handle_event(event, epoch)?;
            if !consumed
                && press.detail == 1
                && let Some(owner) = owner
            {
                events(NativeEvent::Pointer {
                    epoch: owner.epoch,
                    input: RawPointer::Down {
                        root: (i32::from(press.root_x), i32::from(press.root_y)),
                        time: Duration::from_millis(u64::from(press.time)),
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
        Event::FocusOut(event) if owner.is_some_and(|owner| owner.window == event.event) => {
            let focused = connection
                .get_input_focus()
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .map(|reply| reply.focus);
            if focused == Some(event.event) {
                return Ok(());
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
        Event::MappingNotify(_) | Event::XkbMapNotify(_) | Event::XkbNewKeyboardNotify(_) => {
            backend.refresh()?;
        }
        Event::XkbStateNotify(event) if event.changed.contains(xkb::StatePart::GROUP_STATE) => {
            backend.refresh()?;
        }
        _ => {}
    }
    Ok(())
}
