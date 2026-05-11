use rpresent_zoom::screenshot::{CaptureMode, ScreenshotState};

#[test]
fn static_mode_requires_explicit_refresh_request() {
    let mut s = ScreenshotState::new(CaptureMode::Static);
    assert!(!s.should_refresh(1.0 / 60.0));

    s.request_refresh();
    assert!(s.should_refresh(1.0 / 60.0));
    assert!(!s.should_refresh(1.0 / 60.0));
}

#[test]
fn live_mode_refreshes_by_interval() {
    let mut s = ScreenshotState::new(CaptureMode::Live { hz: 30 });
    assert!(!s.should_refresh(1.0 / 120.0));

    let mut refreshed = false;
    for _ in 0..10 {
        if s.should_refresh(1.0 / 120.0) {
            refreshed = true;
            break;
        }
    }
    assert!(refreshed);
}
