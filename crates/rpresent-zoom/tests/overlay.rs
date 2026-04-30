use rpresent_zoom::{ZoomAppearance, ZoomConfig, ZoomState, zoom_sample_rect};

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = ZoomAppearance::default();
    assert_eq!(appearance.initial_scale, 2.0);
    assert_eq!(appearance.min_scale, 1.0);
    assert_eq!(appearance.max_scale, 6.0);
    assert_eq!(appearance.scroll_speed, 0.25);
    assert_eq!(appearance.pan_friction, 0.82);
    assert!(appearance.follow_cursor);
}

#[test]
fn sample_rect_tracks_scale_and_clamps_to_bounds() {
    let state = ZoomState {
        enabled: true,
        scale: 2.0,
        pivot: (100.0, 100.0),
        follow_cursor: true,
        capture_target: String::from("active-monitor"),
    };

    let rect = zoom_sample_rect(1920, 1080, &state);
    assert_eq!(rect.width, 960.0);
    assert_eq!(rect.height, 540.0);
    assert_eq!(rect.left, 0.0);
    assert_eq!(rect.top, 0.0);
}

#[test]
fn zoom_appearance_from_config_matches_fields() {
    let config = ZoomConfig {
        initial_scale: 3.0,
        min_scale: 1.5,
        max_scale: 8.0,
        scroll_speed: 0.5,
        pan_friction: 0.9,
        follow_cursor: false,
        ..ZoomConfig::default()
    };
    let appearance = ZoomAppearance::from(config);
    assert_eq!(appearance.initial_scale, 3.0);
    assert_eq!(appearance.min_scale, 1.5);
    assert_eq!(appearance.max_scale, 8.0);
    assert_eq!(appearance.scroll_speed, 0.5);
    assert_eq!(appearance.pan_friction, 0.9);
    assert!(!appearance.follow_cursor);
}

#[test]
fn sample_rect_clamps_pivot_to_bounds() {
    let state = ZoomState {
        enabled: true,
        scale: 2.0,
        pivot: (99999.0, 99999.0),
        follow_cursor: false,
        capture_target: String::from("active-monitor"),
    };
    let rect = zoom_sample_rect(1920, 1080, &state);
    // At scale 2, width=960, height=540; max_left=960, max_top=540
    assert_eq!(rect.left, 960.0);
    assert_eq!(rect.top, 540.0);
    assert_eq!(rect.width, 960.0);
    assert_eq!(rect.height, 540.0);
}
