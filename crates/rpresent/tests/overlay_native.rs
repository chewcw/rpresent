#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rpresent_config::{Config, ConfigService};
use rpresent_spotlight::{
    SpotlightEvent, SpotlightEventSink, SpotlightRuntimeMessage, run_overlay as run_spotlight,
};
use rpresent_zoom::{
    ZoomEvent, ZoomEventSink, ZoomRuntimeRequest, channel as zoom_channel, run_overlay as run_zoom,
};
use x11rb::connection::Connection;
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK};
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, CreateWindowAux, EventMask,
    StackMode, Window,
};

fn config() -> Arc<ConfigService> {
    Arc::new(ConfigService::new(
        PathBuf::from("overlay-native-test.toml"),
        Config::default(),
    ))
}

fn wait_for<T: std::fmt::Debug>(
    rx: &mpsc::Receiver<T>,
    mut predicate: impl FnMut(&T) -> bool,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) if predicate(&event) => return event,
            Ok(event) => seen.push(format!("{event:?}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    panic!("native event was not reported before the deadline; saw {seen:?}");
}

fn start_spotlight() -> (
    mpsc::Sender<SpotlightRuntimeMessage>,
    mpsc::Receiver<SpotlightEvent>,
    JoinHandle<Result<(), rpresent_spotlight::OverlayError>>,
) {
    let (commands, input) = mpsc::channel();
    let (events, output) = mpsc::channel();
    let sink: SpotlightEventSink = Arc::new(move |event| {
        let _ = events.send(event);
    });
    let thread = std::thread::spawn(move || run_spotlight(config(), input, sink));
    (commands, output, thread)
}

fn start_zoom() -> (
    rpresent_zoom::ZoomHandle,
    mpsc::Receiver<ZoomEvent>,
    JoinHandle<Result<(), rpresent_zoom::OverlayError>>,
) {
    let (handle, inbox) = zoom_channel().expect("zoom transport");
    let (events, output) = mpsc::channel();
    let sink: ZoomEventSink = Arc::new(move |event| {
        let _ = events.send(event);
    });
    let thread = std::thread::spawn(move || run_zoom(config(), inbox, sink));
    (handle, output, thread)
}

fn assert_empty_input_region(connection: &impl Connection, window: Window) {
    let reply = connection
        .shape_get_rectangles(window, SK::INPUT)
        .unwrap()
        .reply()
        .unwrap();
    assert!(reply.rectangles.is_empty());
}

fn test_windows(connection: &impl Connection, root: Window) -> [Window; 3] {
    let setup = connection.setup();
    let screen = &setup.roots[0];
    let mut windows = [0; 3];
    for window in &mut windows {
        *window = connection.generate_id().unwrap();
        connection
            .create_window(
                screen.root_depth,
                *window,
                root,
                10,
                10,
                320,
                240,
                0,
                x11rb::protocol::xproto::WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new().event_mask(EventMask::BUTTON_PRESS),
            )
            .unwrap()
            .check()
            .unwrap();
        connection.map_window(*window).unwrap().check().unwrap();
    }
    connection.flush().unwrap();
    windows
}

fn stack(connection: &impl Connection, bottom_to_top: [Window; 3]) {
    for window in bottom_to_top {
        connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .unwrap()
            .check()
            .unwrap();
    }
    connection.flush().unwrap();
}

fn assert_relative_order(connection: &impl Connection, bottom_to_top: [Window; 3]) {
    let children = connection
        .query_tree(connection.setup().roots[0].root)
        .unwrap()
        .reply()
        .unwrap()
        .children;
    let positions = bottom_to_top.map(|window| {
        children
            .iter()
            .position(|child| *child == window)
            .expect("test window is in the root tree")
    });
    assert!(positions[0] < positions[1] && positions[1] < positions[2]);
}

#[test]
#[ignore = "requires a private X11 display with GLX"]
fn spotlight_then_zoom_keeps_both_layers_live() {
    let (spotlight, spotlight_events, spotlight_thread) = start_spotlight();
    let SpotlightEvent::Ready {
        window: spotlight_window,
    } = wait_for(&spotlight_events, |event| {
        matches!(event, SpotlightEvent::Ready { .. })
    })
    else {
        unreachable!();
    };
    spotlight
        .send(SpotlightRuntimeMessage::Present {
            operation: 1,
            visible: true,
        })
        .unwrap();
    wait_for(&spotlight_events, |event| {
        matches!(
            event,
            SpotlightEvent::Presented {
                operation: 1,
                visible: true,
                result: Ok(())
            }
        )
    });

    let (zoom, zoom_events, zoom_thread) = start_zoom();
    wait_for(&zoom_events, |event| matches!(event, ZoomEvent::Ready));
    zoom.send(ZoomRuntimeRequest::SelectMonitor { operation: 1 })
        .unwrap();
    let ZoomEvent::Selected { monitor, .. } = wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Selected { .. })
    }) else {
        unreachable!();
    };
    zoom.send(ZoomRuntimeRequest::Prepare {
        operation: 2,
        monitor,
    })
    .unwrap();
    let ZoomEvent::Prepared {
        window: zoom_window,
        ..
    } = wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Prepared { .. })
    })
    else {
        unreachable!();
    };
    zoom.send(ZoomRuntimeRequest::Commit { operation: 3 })
        .unwrap();
    wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Committed { .. })
    });

    let (connection, _) = x11rb::connect(None).unwrap();
    for window in [zoom_window, spotlight_window] {
        connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .unwrap()
            .check()
            .unwrap();
    }
    connection.flush().unwrap();
    let children = connection
        .query_tree(connection.setup().roots[0].root)
        .unwrap()
        .reply()
        .unwrap()
        .children;
    assert!(
        children
            .iter()
            .position(|window| *window == zoom_window)
            .unwrap()
            < children
                .iter()
                .position(|window| *window == spotlight_window)
                .unwrap()
    );

    zoom.send(ZoomRuntimeRequest::Close { operation: 4 })
        .unwrap();
    wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Closed { .. })
    });
    drop(zoom);
    drop(spotlight);
    zoom_thread.join().unwrap().unwrap();
    spotlight_thread.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires a private X11 display with GLX"]
fn zoom_then_spotlight_keeps_pan_and_wheel() {
    let (zoom, zoom_events, zoom_thread) = start_zoom();
    wait_for(&zoom_events, |event| matches!(event, ZoomEvent::Ready));
    zoom.send(ZoomRuntimeRequest::SelectMonitor { operation: 1 })
        .unwrap();
    let ZoomEvent::Selected { monitor, .. } = wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Selected { .. })
    }) else {
        unreachable!();
    };
    zoom.send(ZoomRuntimeRequest::Prepare {
        operation: 2,
        monitor,
    })
    .unwrap();
    let ZoomEvent::Prepared {
        window: zoom_window,
        ..
    } = wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Prepared { .. })
    })
    else {
        unreachable!();
    };
    zoom.send(ZoomRuntimeRequest::Commit { operation: 3 })
        .unwrap();
    wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Committed { .. })
    });
    zoom.send(ZoomRuntimeRequest::Scroll {
        delta: 1.0,
        root: (20, 20),
    })
    .unwrap();

    let (spotlight, spotlight_events, spotlight_thread) = start_spotlight();
    let SpotlightEvent::Ready {
        window: spotlight_window,
    } = wait_for(&spotlight_events, |event| {
        matches!(event, SpotlightEvent::Ready { .. })
    })
    else {
        unreachable!();
    };
    spotlight
        .send(SpotlightRuntimeMessage::Present {
            operation: 1,
            visible: true,
        })
        .unwrap();
    wait_for(&spotlight_events, |event| {
        matches!(event, SpotlightEvent::Presented { result: Ok(()), .. })
    });

    let (connection, _) = x11rb::connect(None).unwrap();
    for window in [zoom_window, spotlight_window] {
        connection
            .configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .unwrap()
            .check()
            .unwrap();
    }
    connection.flush().unwrap();
    let children = connection
        .query_tree(connection.setup().roots[0].root)
        .unwrap()
        .reply()
        .unwrap()
        .children;
    assert!(
        children
            .iter()
            .position(|window| *window == zoom_window)
            .unwrap()
            < children
                .iter()
                .position(|window| *window == spotlight_window)
                .unwrap()
    );

    zoom.send(ZoomRuntimeRequest::Close { operation: 4 })
        .unwrap();
    wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Closed { .. })
    });
    drop(zoom);
    drop(spotlight);
    zoom_thread.join().unwrap().unwrap();
    spotlight_thread.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires a private X11 display with GLX"]
fn zoom_capture_excludes_spotlight_shading() {
    let (spotlight, spotlight_events, spotlight_thread) = start_spotlight();
    wait_for(&spotlight_events, |event| {
        matches!(event, SpotlightEvent::Ready { .. })
    });
    spotlight
        .send(SpotlightRuntimeMessage::Present {
            operation: 1,
            visible: true,
        })
        .unwrap();
    wait_for(&spotlight_events, |event| {
        matches!(
            event,
            SpotlightEvent::Presented {
                operation: 1,
                visible: true,
                result: Ok(())
            }
        )
    });
    spotlight
        .send(SpotlightRuntimeMessage::Present {
            operation: 2,
            visible: false,
        })
        .unwrap();
    wait_for(&spotlight_events, |event| {
        matches!(
            event,
            SpotlightEvent::Presented {
                operation: 2,
                visible: false,
                result: Ok(())
            }
        )
    });

    let (zoom, zoom_events, zoom_thread) = start_zoom();
    wait_for(&zoom_events, |event| matches!(event, ZoomEvent::Ready));
    zoom.send(ZoomRuntimeRequest::SelectMonitor { operation: 1 })
        .unwrap();
    let ZoomEvent::Selected { monitor, .. } = wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Selected { .. })
    }) else {
        unreachable!()
    };
    zoom.send(ZoomRuntimeRequest::Prepare {
        operation: 2,
        monitor,
    })
    .unwrap();
    wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Prepared { .. })
    });

    zoom.send(ZoomRuntimeRequest::Close { operation: 3 })
        .unwrap();
    wait_for(&zoom_events, |event| {
        matches!(event, ZoomEvent::Closed { .. })
    });
    drop(zoom);
    drop(spotlight);
    zoom_thread.join().unwrap().unwrap();
    spotlight_thread.join().unwrap().unwrap();
}

#[test]
#[ignore = "requires a private X11 display with XTEST"]
fn closing_zoom_restores_desktop_input() {
    let (connection, screen_num) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[screen_num].root;
    let [desktop, zoom, spotlight] = test_windows(&connection, root);
    stack(&connection, [desktop, zoom, spotlight]);
    connection.unmap_window(zoom).unwrap().check().unwrap();
    connection.flush().unwrap();
    let desktop_attributes = connection
        .get_window_attributes(desktop)
        .unwrap()
        .reply()
        .unwrap();
    let zoom_attributes = connection
        .get_window_attributes(zoom)
        .unwrap()
        .reply()
        .unwrap();
    assert_eq!(
        desktop_attributes.map_state,
        x11rb::protocol::xproto::MapState::VIEWABLE
    );
    assert_eq!(
        zoom_attributes.map_state,
        x11rb::protocol::xproto::MapState::UNMAPPED
    );
}

#[test]
#[ignore = "requires a private X11 display"]
fn native_order_accepts_an_annotation_window() {
    let (connection, screen_num) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[screen_num].root;
    let [zoom, spotlight, annotation] = test_windows(&connection, root);
    connection
        .shape_rectangles(
            x11rb::protocol::shape::SO::SET,
            SK::INPUT,
            x11rb::protocol::xproto::ClipOrdering::UNSORTED,
            spotlight,
            0,
            0,
            &[],
        )
        .unwrap()
        .check()
        .unwrap();
    stack(&connection, [zoom, spotlight, annotation]);
    assert_empty_input_region(&connection, spotlight);
    assert_relative_order(&connection, [zoom, spotlight, annotation]);
}
