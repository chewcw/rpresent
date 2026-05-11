use rpresent_zoom::{
    ScreenshotPlacement, ZoomAppearance, ZoomConfig, screenshot_placement,
};

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = ZoomAppearance::default();
    assert_eq!(appearance.initial_scale, 2.0);
    assert_eq!(appearance.min_scale, 1.0);
    assert_eq!(appearance.max_scale, 6.0);
    assert_eq!(appearance.scroll_speed, 0.25);
    assert_eq!(appearance.pan_friction, 0.82);
    assert_eq!(appearance.scale_friction, 5.0);
    assert!(appearance.follow_cursor);
}

#[test]
fn zoom_appearance_from_config_matches_fields() {
    let config = ZoomConfig {
        initial_scale: 3.0,
        min_scale: 1.5,
        max_scale: 8.0,
        scroll_speed: 0.5,
        pan_friction: 0.9,
        scale_friction: 6.0,
        follow_cursor: false,
        ..ZoomConfig::default()
    };
    let appearance = ZoomAppearance::from(config);
    assert_eq!(appearance.initial_scale, 3.0);
    assert_eq!(appearance.min_scale, 1.5);
    assert_eq!(appearance.max_scale, 8.0);
    assert_eq!(appearance.scroll_speed, 0.5);
    assert_eq!(appearance.pan_friction, 0.9);
    assert_eq!(appearance.scale_friction, 6.0);
    assert!(!appearance.follow_cursor);
}

#[test]
fn screenshot_placement_centers_the_frozen_frame_at_1x() {
    let placement = screenshot_placement(1920, 1080, 1.0, (960.0, 540.0));

    assert_eq!(
        placement,
        ScreenshotPlacement {
            translate_x: 0.0,
            translate_y: 0.0,
            scale: 1.0,
        }
    );
}

#[test]
fn screenshot_placement_preserves_drag_offset_at_1x() {
    let placement = screenshot_placement(1920, 1080, 1.0, (1000.0, 500.0));

    assert_eq!(
        placement,
        ScreenshotPlacement {
            translate_x: -40.0,
            translate_y: 40.0,
            scale: 1.0,
        }
    );
}
