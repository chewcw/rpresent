use rpresent_config::ZoomConfig;
use rpresent_zoom::{ZoomCommand, ZoomController};

#[test]
fn applies_zoom_commands_and_resets_geometry() {
    let config = ZoomConfig {
        enabled_on_start: false,
        initial_scale: 2.0,
        ..ZoomConfig::default()
    };

    let mut controller = ZoomController::from_config(&config);

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
    let config = ZoomConfig {
        initial_scale: 2.0,
        scroll_speed: 0.5,
        pan_friction: 0.5,
        ..ZoomConfig::default()
    };

    let mut controller = ZoomController::from_config(&config);

    assert!(controller.zoom_by_scroll(1.0, (320.0, 200.0)));
    let state = controller.state();
    assert_eq!(state.scale, 3.0);
    assert_eq!(state.pivot, (320.0, 200.0));

    assert!(controller.zoom_by_scroll(100.0, (640.0, 360.0)));
    assert_eq!(controller.state().scale, 6.0);

    assert!(controller.pan_by(10.0, -4.0));
    assert_eq!(controller.state().pivot, (645.0, 358.0));
}

#[test]
fn zoom_by_scroll_with_follow_cursor_updates_pivot() {
    let config = ZoomConfig::default();
    let mut controller = ZoomController::from_config(&config);

    controller.zoom_by_scroll(1.0, (500.0, 400.0));
    assert_eq!(controller.state().pivot, (500.0, 400.0));
}

#[test]
fn zoom_by_scroll_with_fixed_pivot_keeps_pivot_stable() {
    let config = ZoomConfig::default();
    let mut controller = ZoomController::from_config(&config);

    controller.zoom_by_scroll(0.5, (100.0, 200.0));
    controller.zoom_by_scroll(0.5, (100.0, 200.0));
    assert_eq!(controller.state().pivot, (100.0, 200.0));
}

#[test]
fn reset_restores_scale_and_pivot() {
    let config = ZoomConfig {
        initial_scale: 2.0,
        ..ZoomConfig::default()
    };
    let mut controller = ZoomController::from_config(&config);

    controller.zoom_by_scroll(1.0, (320.0, 200.0));
    controller.apply(ZoomCommand::On);
    controller.reset();

    let state = controller.state();
    assert_eq!(state.scale, 2.0);
    assert_eq!(state.pivot, (0.0, 0.0));
}
