use rpresent_zoom::{ZoomAppearance, ZoomState, zoom_sample_rect};

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
