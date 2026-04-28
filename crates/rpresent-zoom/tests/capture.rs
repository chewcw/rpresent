use rpresent_zoom::{CaptureError, CapturedFrame, FrameSource, ZoomCapture};

struct FakeFrameSource;

impl FrameSource for FakeFrameSource {
    fn capture_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        Ok(CapturedFrame::new(2, 2, vec![0; 16]))
    }
}

#[test]
fn returns_a_synthetic_frame() {
    let mut pipeline = ZoomCapture::new(FakeFrameSource);
    let frame = pipeline.next_frame().unwrap();

    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 2);
    assert_eq!(frame.rgba.len(), 16);
}
