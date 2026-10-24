use std::time::Duration;

use rpresent_zoom::ZoomCommand;
use rpresent_zoom::navigation::NavigationConfig;
use rpresent_zoom::session::{MonitorRect, PointerInput, SessionEffect, ZoomRequest, ZoomSession};

fn monitor() -> MonitorRect {
    MonitorRect {
        x: -1920,
        y: -200,
        width: 1920,
        height: 1080,
    }
}

#[test]
fn activation_is_not_published_before_native_success() {
    let mut session = ZoomSession::new(NavigationConfig::default());
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::On)),
        SessionEffect::Activate
    );
    assert!(!session.is_active());
    session.activated(monitor());
    assert!(session.is_active());
    assert!(session.take_dirty());
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::On)),
        SessionEffect::None
    );
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Off)),
        SessionEffect::Close
    );
    session.closed();
    assert!(!session.is_active());
    assert!(session.camera().is_none());
    assert!(!session.take_dirty());
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Off)),
        SessionEffect::None
    );
}

#[test]
fn disabled_input_and_reset_do_not_activate_or_dirty_the_session() {
    let mut session = ZoomSession::new(NavigationConfig::default());
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Reset)),
        SessionEffect::None
    );
    session.request(ZoomRequest::Scroll {
        delta: 1.0,
        root: (-1800, -100),
    });
    session.pointer(PointerInput::Down {
        local: (10.0, 20.0),
        time: Duration::ZERO,
    });
    session.pointer(PointerInput::Move {
        local: (30.0, 40.0),
        time: Duration::from_millis(20),
    });
    session.pointer(PointerInput::Up {
        time: Duration::from_millis(20),
    });
    session.pointer(PointerInput::Cancel);
    session.mark_dirty();
    assert!(!session.is_active());
    assert!(!session.is_animating());
    assert!(!session.advance(Duration::from_secs(1)));
    assert!(!session.take_dirty());
}

#[test]
fn wheel_coordinates_use_the_signed_monitor_origin() {
    for selected in [
        monitor(),
        MonitorRect {
            x: 1920,
            y: 200,
            ..monitor()
        },
    ] {
        let mut session = ZoomSession::new(NavigationConfig::default());
        session.activated(selected);
        session.take_dirty();
        let local = (300.0, 250.0);
        let before = session.camera().unwrap().image_at(local);
        session.request(ZoomRequest::Scroll {
            delta: 1.0,
            root: (selected.x + 300, selected.y + 250),
        });
        assert!(session.is_animating());
        assert!(session.advance(Duration::from_millis(16)));
        let after = session.camera().unwrap().image_at(local);
        assert!((before.0 - after.0).abs() < 1e-8);
        assert!((before.1 - after.1).abs() < 1e-8);
        assert!(session.take_dirty());
        assert!(!session.take_dirty());
    }
}

#[test]
fn dragging_marks_dirty_but_idle_pointer_motion_does_not() {
    let mut session = ZoomSession::new(NavigationConfig::default());
    session.activated(monitor());
    session.take_dirty();
    session.pointer(PointerInput::Move {
        local: (50.0, 50.0),
        time: Duration::ZERO,
    });
    assert!(!session.take_dirty());
    session.pointer(PointerInput::Down {
        local: (50.0, 50.0),
        time: Duration::ZERO,
    });
    assert!(!session.take_dirty());
    session.pointer(PointerInput::Move {
        local: (50.0, 50.0),
        time: Duration::from_millis(10),
    });
    assert!(session.take_dirty());
    session.pointer(PointerInput::Move {
        local: (80.0, 50.0),
        time: Duration::from_millis(30),
    });
    assert!(session.take_dirty());
    session.pointer(PointerInput::Up {
        time: Duration::from_millis(30),
    });
    assert!(session.is_animating());
    session.pointer(PointerInput::Cancel);
    assert!(!session.is_animating());
    assert!(!session.advance(Duration::from_millis(16)));
    session.pointer(PointerInput::Move {
        local: (150.0, 50.0),
        time: Duration::from_millis(60),
    });
    assert!(!session.take_dirty());
}

#[test]
fn reset_clears_motion_without_reactivating() {
    let mut session = ZoomSession::new(NavigationConfig::default());
    session.activated(monitor());
    let initial = session.camera().unwrap().clone();
    session.request(ZoomRequest::Scroll {
        delta: 1.0,
        root: (-1700, 0),
    });
    session.advance(Duration::from_millis(30));
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Reset)),
        SessionEffect::None
    );
    assert!(session.is_active());
    assert!(!session.is_animating());
    assert_eq!(session.camera().unwrap().scale(), initial.scale());
    assert_eq!(
        session.camera().unwrap().translation(),
        initial.translation()
    );
    assert!(session.take_dirty());
    session.request(ZoomRequest::Command(ZoomCommand::Reset));
    assert!(session.take_dirty());
}

#[test]
fn toggle_close_and_reactivate_start_with_a_new_camera() {
    let mut session = ZoomSession::new(NavigationConfig::default());
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Toggle)),
        SessionEffect::Activate
    );
    session.activated(monitor());
    session.pointer(PointerInput::Down {
        local: (0.0, 0.0),
        time: Duration::ZERO,
    });
    session.pointer(PointerInput::Move {
        local: (50.0, 0.0),
        time: Duration::from_millis(20),
    });
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Toggle)),
        SessionEffect::Close
    );
    session.closed();
    assert_eq!(
        session.request(ZoomRequest::Command(ZoomCommand::Toggle)),
        SessionEffect::Activate
    );
    session.activated(MonitorRect {
        width: 800,
        height: 600,
        ..monitor()
    });
    assert_eq!(session.camera().unwrap().translation(), (-400.0, -300.0));
    assert!(!session.is_animating());
    session.take_dirty();
    session.pointer(PointerInput::Move {
        local: (100.0, 0.0),
        time: Duration::from_millis(40),
    });
    assert!(!session.take_dirty());
    session.mark_dirty();
    assert!(session.take_dirty());
}
