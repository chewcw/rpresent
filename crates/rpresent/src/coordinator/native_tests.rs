use super::tests::Rig;
use super::*;
use rpresent_config::{Config, ConfigService, SpotShape};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, CreateWindowAux, ImageFormat,
    WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;

fn service(shape: SpotShape) -> Arc<ConfigService> {
    let mut config = Config::default();
    config.spotlight.shape = shape;
    config.spotlight.radius_px = 60;
    config.spotlight.softness_px = 0;
    config.spotlight.show_border = false;
    config.spotlight.show_center_dot = true;
    config.spotlight.dot_size_px = 8;
    config.spotlight.dot_color = "#FF0000".into();
    config.spotlight.dot_opacity = 1.0;
    config.spotlight.shade_color = "#000000".into();
    config.spotlight.shade_opacity = 0.6;
    Arc::new(ConfigService::new(
        PathBuf::from("native-test.toml"),
        config,
    ))
}

fn drive(rig: &mut Rig, events: &mpsc::Receiver<AppEvent>, mut done: impl FnMut(&Rig) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        while rig.step() {}
        if done(rig) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native coordinator did not settle"
        );
        if let Ok(event) = events.recv_timeout(Duration::from_millis(10)) {
            rig.event(event);
        }
    }
}

fn pixel(bytes: &[u8], width: u16, x: usize, y: usize) -> [u8; 4] {
    let offset = (y * usize::from(width) + x) * 4;
    bytes[offset..offset + 4].try_into().unwrap()
}

fn wait_for_pixel(
    mut snapshot: impl FnMut() -> Vec<u8>,
    width: u16,
    x: usize,
    y: usize,
    expected: [u8; 4],
) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let image = snapshot();
        if pixel(&image, width, x, y) == expected {
            return image;
        }
        assert!(Instant::now() < deadline, "expected pixel did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires RPRESENT_NATIVE_TEST=1 and a private X11 display"]
fn native_stationary_spotlight_during_annotation() {
    assert_eq!(std::env::var("RPRESENT_NATIVE_TEST").as_deref(), Ok("1"));
    for shape in [
        SpotShape::Circle,
        SpotShape::Square {
            corner_radius_pct: 0,
        },
    ] {
        let (connection, screen_number) = x11rb::connect(None).unwrap();
        let screen = &connection.setup().roots[screen_number];
        assert!(screen.width_in_pixels >= 800 && screen.height_in_pixels >= 600);
        assert_eq!(
            connection.setup().image_byte_order,
            x11rb::protocol::xproto::ImageOrder::LSB_FIRST
        );
        let root = screen.root;
        let annotation_window = connection.generate_id().unwrap();
        connection
            .create_window(
                0,
                annotation_window,
                root,
                0,
                0,
                800,
                600,
                0,
                WindowClass::INPUT_ONLY,
                0_u32,
                &CreateWindowAux::new().override_redirect(1),
            )
            .unwrap()
            .check()
            .unwrap();
        connection
            .map_window(annotation_window)
            .unwrap()
            .check()
            .unwrap();
        connection
            .warp_pointer(0_u32, root, 0, 0, 0, 0, 150, 150)
            .unwrap()
            .check()
            .unwrap();
        connection.flush().unwrap();

        let config = service(shape);
        let (events_tx, events_rx) = mpsc::channel();
        let (spotlight_tx, spotlight_rx) = mpsc::channel();
        let spot_events = events_tx.clone();
        let sink: rpresent_spotlight::SpotlightEventSink = Arc::new(move |event| {
            let _ = spot_events.send(AppEvent::SpotlightRuntime(event));
        });
        let spot_config = Arc::clone(&config);
        let spot_thread = std::thread::spawn(move || {
            rpresent_spotlight::run_overlay(spot_config, spotlight_rx, sink)
        });
        let (native, native_inbox) = crate::native_input::channel().unwrap();
        let native_events = events_tx.clone();
        crate::x11_input::spawn(
            Arc::clone(&config),
            native_inbox,
            Arc::new(move |event| {
                let _ = native_events.send(AppEvent::Native(event));
            }),
        );
        let mut rig = Rig::new();
        rig.app.config = config;
        rig.app.outputs.spotlight = spotlight_tx;
        rig.app.outputs.native = native;
        rig.app.spotlight_ready = false;
        rig.app.native_ready = false;
        rig.app.layers.spotlight = LayerState::default();
        rig.app.layers.annotation.window = Some(annotation_window);
        drive(&mut rig, &events_rx, |r| {
            r.app.spotlight_ready && r.app.native_ready
        });
        rig.app
            .handle_command(AppCommand::Spotlight(
                rpresent_spotlight::SpotlightCommand::On,
            ))
            .unwrap();
        drive(&mut rig, &events_rx, |r| {
            r.app.input.confirmed_target() == PointerOwner::Spotlight
                && r.app.lifecycle.next_deadline().is_none()
        });
        let spotlight_window = rig.app.layers.spotlight.window.unwrap();
        let geometry = connection
            .get_geometry(spotlight_window)
            .unwrap()
            .reply()
            .unwrap();
        let mut snapshot = || {
            connection
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    spotlight_window,
                    0,
                    0,
                    geometry.width,
                    geometry.height,
                    u32::MAX,
                )
                .unwrap()
                .reply()
                .unwrap()
                .data
        };
        let initial = wait_for_pixel(&mut snapshot, geometry.width, 150, 150, [0, 0, 255, 255]);

        rig.app
            .annotation_status(AnnotationStatus {
                generation: 1,
                window: Some(annotation_window),
                ready: true,
                editing_requested: true,
                has_content: true,
                text_editing: false,
            })
            .unwrap();
        drive(&mut rig, &events_rx, |r| {
            r.app.input.confirmed_target() == PointerOwner::Annotation
        });
        connection
            .warp_pointer(0_u32, root, 0, 0, 0, 0, 550, 400)
            .unwrap()
            .check()
            .unwrap();
        for event in [BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT] {
            connection
                .xtest_fake_input(event, 1, x11rb::CURRENT_TIME, root, 0, 0, 0)
                .unwrap()
                .check()
                .unwrap();
        }
        connection.flush().unwrap();
        drive(&mut rig, &events_rx, |r| {
            r.annotation_inputs.iter().any(|request| {
                matches!(
                    request,
                    AnnotationRequest::Pointer {
                        input: rpresent_annotation::PointerInput::Down { .. },
                        ..
                    }
                )
            }) && r.annotation_inputs.iter().any(|request| {
                matches!(
                    request,
                    AnnotationRequest::Pointer {
                        input: rpresent_annotation::PointerInput::Up { .. },
                        ..
                    }
                )
            })
        });
        let deadline = Instant::now() + Duration::from_millis(160);
        while Instant::now() < deadline {
            assert_eq!(snapshot(), initial, "held pixels changed for {shape:?}");
            if let Ok(event) = events_rx.recv_timeout(Duration::from_millis(16)) {
                rig.event(event);
            }
        }
        assert!(rig.zoom_inputs.is_empty());

        rig.app
            .annotation_status(AnnotationStatus {
                generation: 1,
                window: Some(annotation_window),
                ready: true,
                editing_requested: false,
                has_content: true,
                text_editing: false,
            })
            .unwrap();
        drive(&mut rig, &events_rx, |r| {
            r.app.input.confirmed_target() == PointerOwner::Spotlight
                && r.app.lifecycle.next_deadline().is_none()
        });
        let resumed = wait_for_pixel(&mut snapshot, geometry.width, 550, 400, [0, 0, 255, 255]);
        for y in 70..230 {
            for x in 70..230 {
                let actual = pixel(&resumed, geometry.width, x, y);
                assert_eq!(&actual[..3], &[0, 0, 0]);
                assert!(actual[3].abs_diff(153) <= 2, "old spotlight left a trail");
            }
        }
        rig.app
            .handle_command(AppCommand::Spotlight(
                rpresent_spotlight::SpotlightCommand::Off,
            ))
            .unwrap();
        drive(&mut rig, &events_rx, |r| {
            !r.app.layers.spotlight.mapped
                && r.app.input.confirmed_target() == PointerOwner::Desktop
                && r.app.lifecycle.next_deadline().is_none()
        });
        drop(rig);
        spot_thread.join().unwrap().unwrap();
        loop {
            if let AppEvent::Native(NativeEvent::Exited(result)) =
                events_rx.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                result.unwrap();
                break;
            }
        }
        connection
            .destroy_window(annotation_window)
            .unwrap()
            .check()
            .unwrap();
    }
}
