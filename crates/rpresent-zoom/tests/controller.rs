use rpresent_config::ZoomConfig;
use rpresent_zoom::{ZoomCommand, ZoomController};

#[test]
fn applies_zoom_commands_and_resets_geometry() {
    let mut config = ZoomConfig::default();
    config.enabled_on_start = false;
    config.initial_scale = 2.0;

    let mut controller = ZoomController::from_config(config);

    assert!(!controller.state().enabled);
    assert!(controller.apply(ZoomCommand::On));
    assert!(controller.state().enabled);
    assert!(controller.apply(ZoomCommand::Off));
    assert!(!controller.state().enabled);
    assert!(controller.apply(ZoomCommand::Toggle));
    assert!(controller.state().enabled);

    controller.zoom_by_scroll(1.0, (320.0, 200.0));
    assert!(controller.apply(ZoomCommand::Reset));

    let state = controller.state();
    assert_eq!(state.scale, 2.0);
    assert_eq!(state.pivot, (0.0, 0.0));
    assert!(state.enabled);
}

#[test]
fn zoom_by_scroll_uses_speed_and_clamps_and_pan_uses_friction() {
    let mut config = ZoomConfig::default();
    config.initial_scale = 2.0;
    config.scroll_speed = 0.5;
    config.pan_friction = 0.5;

    let mut controller = ZoomController::from_config(config);

    assert!(controller.zoom_by_scroll(1.0, (320.0, 200.0)));
    let state = controller.state();
    assert_eq!(state.scale, 3.0);
    assert_eq!(state.pivot, (320.0, 200.0));

    assert!(controller.zoom_by_scroll(100.0, (640.0, 360.0)));
    assert_eq!(controller.state().scale, 6.0);

    assert!(controller.pan_by(10.0, -4.0));
    assert_eq!(controller.state().pivot, (645.0, 358.0));
}
