use rpresent_zoom::navigation::{Camera, NavigationConfig};
use std::time::Duration;

#[test]
fn reversing_wheel_changes_the_next_frame_direction() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    camera.scroll(1.0, (700.0, 300.0));
    camera.advance(Duration::from_secs_f64(1.0 / 60.0));
    let before = camera.scale();
    let image_anchor = camera.image_at((700.0, 300.0));
    camera.scroll(-1.0, (700.0, 300.0));
    camera.advance(Duration::from_secs_f64(1.0 / 60.0));
    assert!(camera.scale() < before);
    let after = camera.image_at((700.0, 300.0));
    assert!((after.0 - image_anchor.0).abs() < 1e-8);
    assert!((after.1 - image_anchor.1).abs() < 1e-8);
}

#[test]
fn drag_release_continues_motion_then_decay_stops_it() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    camera.pointer_down((100.0, 100.0), Duration::ZERO);
    camera.pointer_move((140.0, 100.0), Duration::from_millis(40));
    camera.pointer_up(Duration::from_millis(40));
    let before = camera.translation();
    camera.advance(Duration::from_secs_f64(1.0 / 60.0));
    assert!(camera.translation().0 > before.0);
    for _ in 0..1200 {
        camera.advance(Duration::from_secs_f64(1.0 / 120.0));
    }
    assert!(!camera.is_animating());
}

#[test]
fn stationary_release_does_not_reuse_old_velocity() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    camera.pointer_down((100.0, 100.0), Duration::ZERO);
    camera.pointer_move((140.0, 100.0), Duration::from_millis(40));
    camera.pointer_up(Duration::from_millis(200));
    assert_eq!(camera.pan_velocity(), (0.0, 0.0));
}

#[test]
fn short_stationary_gap_before_release_stops_inertia() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    camera.pointer_down((100.0, 100.0), Duration::ZERO);
    camera.pointer_move((140.0, 100.0), Duration::from_millis(40));
    camera.pointer_up(Duration::from_millis(50));
    assert_eq!(camera.pan_velocity(), (0.0, 0.0));
    assert!(!camera.is_animating());
}

#[test]
fn timed_zoom_replay_is_frame_rate_independent() {
    let run = |fps: u32, frames: u32| {
        let frame = Duration::from_secs_f64(1.0 / f64::from(fps));
        let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
        camera.scroll(1.0, (700.0, 300.0));
        for _ in 0..frames {
            camera.advance(frame);
        }
        (camera.scale(), camera.translation())
    };

    let (scale_30, translation_30) = run(30, 60);
    let (scale_60, translation_60) = run(60, 120);
    let (scale_120, translation_120) = run(120, 240);

    assert!((scale_30 - scale_60).abs() < 1e-5);
    assert!((scale_30 - scale_120).abs() < 1e-5);
    assert!((translation_30.0 - translation_60.0).abs() < 0.05);
    assert!((translation_30.0 - translation_120.0).abs() < 0.05);
}

#[test]
fn constant_speed_drag_release_velocity_matches_event_rate() {
    let speed = 10.0_f64;
    let mut velocities = Vec::new();
    for fps in [30, 60, 120, 240] {
        let frame = Duration::from_secs_f64(1.0 / f64::from(fps));
        let frames = f64::from(fps) as u32;
        let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
        camera.pointer_down((0.0, 0.0), Duration::ZERO);
        let mut position = (0.0_f64, 0.0_f64);
        for step in 1..=frames {
            position = (position.0 + speed / f64::from(fps), position.1);
            camera.pointer_move(position, frame * step);
        }
        camera.pointer_up(frame * frames);
        velocities.push(camera.pan_velocity());
    }
    for velocity in &velocities {
        assert!((velocity.0 - speed).abs() < 0.1);
        assert!((velocity.1).abs() < 0.1);
    }
}

#[test]
fn zoom_reversal_works_immediately_after_reaching_a_bound() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    for _ in 0..6 {
        camera.scroll(1.0, (700.0, 300.0));
        for _ in 0..60 {
            camera.advance(Duration::from_secs_f64(1.0 / 60.0));
        }
    }
    assert!((camera.scale() - 6.0).abs() < 1e-9);
    assert!(!camera.is_animating());
    camera.scroll(-1.0, (700.0, 300.0));
    camera.advance(Duration::from_secs_f64(1.0 / 60.0));
    assert!(camera.scale() < 6.0);
}

#[test]
fn repeated_reset_restores_the_initial_camera_state() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    camera.scroll(1.0, (700.0, 300.0));
    camera.advance(Duration::from_secs_f64(1.0 / 60.0));
    camera.reset();
    let fresh = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    assert_eq!(camera.scale(), fresh.scale());
    assert_eq!(camera.translation(), fresh.translation());
    assert_eq!(camera.pan_velocity(), (0.0, 0.0));
    assert_eq!(camera.zoom_velocity(), 0.0);
}

#[test]
fn idle_pointer_movement_changes_no_camera_state() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    let translation = camera.translation();
    let scale = camera.scale();
    camera.pointer_move((500.0, 400.0), Duration::from_millis(16));
    camera.pointer_move((600.0, 450.0), Duration::from_millis(32));
    assert_eq!(camera.translation(), translation);
    assert_eq!(camera.scale(), scale);
    assert!(!camera.is_animating());
}

#[test]
fn zero_time_advance_changes_no_camera_state() {
    let mut camera = Camera::new(NavigationConfig::default(), (1920.0, 1080.0));
    camera.scroll(1.0, (700.0, 300.0));
    assert!(!camera.advance(Duration::ZERO));
    assert_eq!(camera.scale(), 2.0);
    assert_eq!(camera.pan_velocity(), (0.0, 0.0));
}
