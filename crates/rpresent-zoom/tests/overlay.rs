use rpresent_zoom::{ZoomState, zoom_sample_rect};

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
fn sample_rect_clamps_pivot_to_bounds() {
    let state = ZoomState {
        enabled: true,
        scale: 2.0,
        pivot: (99999.0, 99999.0),
        follow_cursor: false,
        capture_target: String::from("active-monitor"),
    };
    let rect = zoom_sample_rect(1920, 1080, &state);
    assert_eq!(rect.left, 960.0);
    assert_eq!(rect.top, 540.0);
    assert_eq!(rect.width, 960.0);
    assert_eq!(rect.height, 540.0);
}
