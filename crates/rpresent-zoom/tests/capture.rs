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

struct ConfigurableFakeFrameSource {
    width: u32,
    height: u32,
    color: [u8; 4],
}

impl FrameSource for ConfigurableFakeFrameSource {
    fn capture_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        let pixel_count = (self.width * self.height) as usize;
        let rgba: Vec<u8> = self
            .color
            .iter()
            .copied()
            .cycle()
            .take(pixel_count * 4)
            .collect();
        Ok(CapturedFrame::new(self.width, self.height, rgba))
    }
}

#[test]
fn fake_frame_source_returns_injected_frame() {
    let fake = ConfigurableFakeFrameSource {
        width: 4,
        height: 3,
        color: [255, 0, 128, 255],
    };
    let mut pipeline = ZoomCapture::new(fake);
    let frame = pipeline.next_frame().unwrap();

    assert_eq!(frame.width, 4);
    assert_eq!(frame.height, 3);
    assert_eq!(frame.rgba.len(), 48); // 4 * 3 * 4 bytes
    assert_eq!(&frame.rgba[0..4], &[255, 0, 128, 255]);
}
