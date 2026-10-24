#![cfg(target_os = "linux")]

use rpresent_zoom::runtime::ZoomRuntimeRequest;
use rpresent_zoom::transport::channel;

#[test]
fn cloned_handles_share_inactive_status_and_can_send_requests() {
    let (handle, _inbox) = channel().unwrap();
    let cloned = handle.clone();
    handle
        .send(ZoomRuntimeRequest::Reset { operation: 1 })
        .unwrap();
    cloned
        .send(ZoomRuntimeRequest::Reset { operation: 2 })
        .unwrap();
    assert!(!handle.is_active());
    assert!(!cloned.is_active());
}

#[test]
fn sending_after_the_runtime_drops_returns_broken_pipe() {
    let (handle, inbox) = channel().unwrap();
    drop(inbox);
    let error = handle
        .send(ZoomRuntimeRequest::Reset { operation: 1 })
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}
