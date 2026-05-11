use rpresent_config::ZoomConfig;
use rpresent_zoom::{ZoomCommand, ZoomController, ZoomRuntimeMessage};

#[test]
fn initial_messages_include_config_and_enabled_state() {
    let config = ZoomConfig {
        enabled_on_start: true,
        initial_scale: 3.0,
        min_scale: 1.5,
        max_scale: 8.0,
        scroll_speed: 0.4,
        pan_friction: 3.2,
        scale_friction: 6.1,
        follow_cursor: false,
        capture_target: "active-monitor".to_owned(),
    };
    let controller = ZoomController::from_config(config.clone());

    let messages = controller.initial_messages();
    assert_eq!(
        messages[0],
        ZoomRuntimeMessage::Configure(rpresent_zoom::ZoomRuntimeConfig {
            initial_scale: config.initial_scale,
            min_scale: config.min_scale,
            max_scale: config.max_scale,
            scroll_speed: config.scroll_speed,
            pan_friction: config.pan_friction,
            scale_friction: config.scale_friction,
            follow_cursor: config.follow_cursor,
            capture_target: config.capture_target,
        })
    );
    assert_eq!(messages[1], ZoomRuntimeMessage::SetEnabled(true));
}

#[test]
fn zoom_and_drag_messages_require_enabled_zoom() {
    let mut controller = ZoomController::from_config(ZoomConfig {
        enabled_on_start: false,
        ..ZoomConfig::default()
    });

    assert_eq!(controller.zoom_by_scroll(1.0, (320.0, 200.0)), None);
    assert_eq!(controller.drag_start((10.0, 20.0)), None);
    assert_eq!(controller.drag_move((15.0, 25.0)), None);
    assert_eq!(controller.drag_end(), None);

    assert_eq!(controller.apply(ZoomCommand::On), Some(ZoomRuntimeMessage::SetEnabled(true)));
    assert_eq!(
        controller.zoom_by_scroll(-1.0, (400.0, 300.0)),
        Some(ZoomRuntimeMessage::Scroll {
            delta: -1.0,
            pivot: (400.0, 300.0),
        })
    );
    assert_eq!(
        controller.drag_start((1.0, 2.0)),
        Some(ZoomRuntimeMessage::DragStart { x: 1.0, y: 2.0 })
    );
    assert_eq!(
        controller.drag_move((3.0, 4.0)),
        Some(ZoomRuntimeMessage::DragMove { x: 3.0, y: 4.0 })
    );
    assert_eq!(controller.drag_end(), Some(ZoomRuntimeMessage::DragEnd));
}

#[test]
fn command_application_emits_expected_runtime_messages() {
    let mut controller = ZoomController::from_config(ZoomConfig::default());

    assert_eq!(controller.apply(ZoomCommand::On), Some(ZoomRuntimeMessage::SetEnabled(true)));
    assert_eq!(controller.apply(ZoomCommand::On), None);
    assert_eq!(controller.apply(ZoomCommand::Toggle), Some(ZoomRuntimeMessage::SetEnabled(false)));
    assert_eq!(controller.apply(ZoomCommand::Off), None);
    assert_eq!(controller.apply(ZoomCommand::Reset), Some(ZoomRuntimeMessage::Reset));
}
