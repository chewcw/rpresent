#![cfg(target_os = "linux")]

use rpresent_zoom::ZoomCommand;
use rpresent_zoom::session::ZoomRequest;
use rpresent_zoom::transport::channel;

#[test]
fn cloned_handles_share_inactive_status_and_can_send_requests() {
    let (handle, _inbox) = channel().unwrap();
    let cloned = handle.clone();
    handle.send(ZoomRequest::Command(ZoomCommand::On)).unwrap();
    cloned.send(ZoomRequest::Command(ZoomCommand::Off)).unwrap();
    assert!(!handle.is_active());
    assert!(!cloned.is_active());
}

#[test]
fn sending_after_the_runtime_drops_returns_broken_pipe() {
    let (handle, inbox) = channel().unwrap();
    drop(inbox);
    let error = handle
        .send(ZoomRequest::Command(ZoomCommand::On))
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}
