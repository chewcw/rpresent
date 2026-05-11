use rpresent_zoom::navigation::{Camera, MouseState, NavigationConfig, apply_follow_cursor};

#[test]
fn p0_p1_correction_preserves_cursor_world_point() {
    let mut cam = Camera::new(1.0, (960.0, 540.0));
    let cfg = NavigationConfig {
        min_scale: 0.01,
        max_scale: 8.0,
        drag_friction: 6.0,
        scale_friction: 4.0,
    };
    cam.scale_pivot = (700.0, 300.0);
    cam.delta_scale = 10.0;

    let before = cam.world_of_screen(cam.scale_pivot, (1920.0, 1080.0));
    cam.update(&cfg, 1.0 / 60.0, &MouseState::default(), (1920.0, 1080.0));
    let after = cam.world_of_screen(cam.scale_pivot, (1920.0, 1080.0));

    assert!((before.0 - after.0).abs() < 1e-3);
    assert!((before.1 - after.1).abs() < 1e-3);
}

#[test]
fn velocity_decays_when_not_dragging() {
    let mut cam = Camera::new(1.0, (960.0, 540.0));
    let cfg = NavigationConfig {
        min_scale: 0.01,
        max_scale: 8.0,
        drag_friction: 6.0,
        scale_friction: 4.0,
    };
    cam.velocity = (200.0, 0.0);

    let v0 = cam.velocity.0;
    cam.update(&cfg, 1.0 / 60.0, &MouseState::default(), (1920.0, 1080.0));
    assert!(cam.velocity.0 < v0);
}

#[test]
fn scale_is_clamped_to_runtime_bounds() {
    let mut cam = Camera::new(2.0, (960.0, 540.0));
    let cfg = NavigationConfig {
        min_scale: 1.5,
        max_scale: 2.5,
        drag_friction: 6.0,
        scale_friction: 4.0,
    };

    cam.delta_scale = 100.0;
    cam.update(&cfg, 1.0, &MouseState::default(), (1920.0, 1080.0));
    assert!((cam.scale - cfg.max_scale).abs() < 1e-6);

    cam.delta_scale = -100.0;
    cam.update(&cfg, 1.0, &MouseState::default(), (1920.0, 1080.0));
    assert!((cam.scale - cfg.min_scale).abs() < 1e-6);
}

#[test]
fn velocity_is_not_applied_while_dragging() {
    let mut cam = Camera::new(1.0, (960.0, 540.0));
    let cfg = NavigationConfig {
        min_scale: 1.0,
        max_scale: 6.0,
        drag_friction: 6.0,
        scale_friction: 4.0,
    };
    cam.velocity = (300.0, -120.0);
    let pos0 = cam.position;

    cam.update(&cfg, 1.0 / 60.0, &MouseState { drag: true }, (1920.0, 1080.0));

    assert_eq!(cam.position, pos0);
    assert_eq!(cam.velocity, (300.0, -120.0));
}

#[test]
fn follow_cursor_idle_updates_camera_position() {
    let mut cam = Camera::new(2.0, (960.0, 540.0));
    cam.velocity = (100.0, -40.0);

    apply_follow_cursor(&mut cam, (420.0, 315.0), true, false);

    assert_eq!(cam.scale_pivot, (420.0, 315.0));
    assert_eq!(cam.position, (420.0, 315.0));
    assert_eq!(cam.velocity, (0.0, 0.0));
}

#[test]
fn follow_cursor_disabled_keeps_camera_position() {
    let mut cam = Camera::new(2.0, (960.0, 540.0));
    cam.position = (300.0, 220.0);
    cam.velocity = (50.0, 10.0);

    apply_follow_cursor(&mut cam, (420.0, 315.0), false, false);

    assert_eq!(cam.scale_pivot, (960.0, 540.0));
    assert_eq!(cam.position, (300.0, 220.0));
    assert_eq!(cam.velocity, (50.0, 10.0));
}

#[test]
fn follow_cursor_while_dragging_only_updates_pivot() {
    let mut cam = Camera::new(2.0, (960.0, 540.0));
    cam.position = (300.0, 220.0);
    cam.velocity = (50.0, 10.0);

    apply_follow_cursor(&mut cam, (420.0, 315.0), true, true);

    assert_eq!(cam.scale_pivot, (420.0, 315.0));
    assert_eq!(cam.position, (300.0, 220.0));
    assert_eq!(cam.velocity, (50.0, 10.0));
}

#[test]
fn wheel_zoom_in_out_is_symmetric_over_time() {
    let mut cam = Camera::new(2.0, (960.0, 540.0));
    let cfg = NavigationConfig {
        min_scale: 1.0,
        max_scale: 6.0,
        drag_friction: 4.0,
        scale_friction: 5.0,
    };
    let initial = cam.scale;

    cam.scale_pivot = (700.0, 300.0);
    cam.delta_scale += 4.0;
    for _ in 0..600 {
        cam.update(&cfg, 1.0 / 240.0, &MouseState::default(), (1920.0, 1080.0));
    }

    cam.delta_scale -= 4.0;
    for _ in 0..600 {
        cam.update(&cfg, 1.0 / 240.0, &MouseState::default(), (1920.0, 1080.0));
    }

    assert!((cam.scale - initial).abs() < 0.05);
}

#[test]
fn drag_pan_moves_camera_in_world_space() {
    let mut cam = Camera::new(2.0, (960.0, 540.0));

    cam.drag_move((100.0, 100.0), (140.0, 70.0));

    assert!((cam.position.0 - 940.0).abs() < 1e-4);
    assert!((cam.position.1 - 555.0).abs() < 1e-4);
    assert!((cam.velocity.0 + 1200.0).abs() < 1e-3);
    assert!((cam.velocity.1 - 900.0).abs() < 1e-3);
}
