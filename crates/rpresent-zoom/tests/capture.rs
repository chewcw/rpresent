use rpresent_zoom::{CaptureError, CapturedFrame};

#[test]
fn screenshot_requires_complete_bgra_rows() {
    assert!(CapturedFrame::new(2, 1, vec![0; 7]).is_err());
    assert!(CapturedFrame::new(0, 1, vec![]).is_err());
    let frame = CapturedFrame::new(2, 1, vec![0, 0, 255, 255, 255, 0, 0, 255]).unwrap();
    assert_eq!(frame.bgra.len(), 8);
}

#[test]
fn screenshot_rejects_zero_dimensions() {
    assert!(CapturedFrame::new(0, 0, Vec::new()).is_err());
    assert!(CapturedFrame::new(0, 4, vec![0; 16]).is_err());
    assert!(CapturedFrame::new(4, 0, vec![0; 16]).is_err());
}

#[test]
fn screenshot_rejects_overflowing_dimensions() {
    assert!(CapturedFrame::new(u32::MAX, u32::MAX, vec![0; 4]).is_err());
    assert!(CapturedFrame::new(u32::MAX, 2, vec![0; 8]).is_err());
}

#[test]
fn screenshot_requires_exact_bgra_byte_count() {
    let frame = CapturedFrame::new(2, 2, vec![0; 16]).unwrap();
    assert_eq!(frame.bgra.len(), 16);

    assert!(CapturedFrame::new(2, 2, vec![0; 15]).is_err());
    assert!(CapturedFrame::new(2, 2, vec![0; 17]).is_err());
}

#[test]
fn screenshot_accepts_asymmetric_color_image() {
    let frame = CapturedFrame::new(2, 1, vec![0, 0, 255, 255, 255, 0, 0, 255]).unwrap();

    assert_eq!(frame.width, 2);
    assert_eq!(frame.height, 1);
    assert_eq!(frame.bgra.len(), 8);
    // BGRA byte order: blue, green, red, alpha.
    assert_eq!(&frame.bgra[0..4], &[0, 0, 255, 255]);
    assert_eq!(&frame.bgra[4..8], &[255, 0, 0, 255]);
}

#[test]
fn capture_error_distinguishes_validation_failures() {
    let error = CapturedFrame::new(2, 1, vec![0; 7]).unwrap_err();
    assert_eq!(
        error,
        CaptureError::InvalidByteLength {
            expected: 8,
            actual: 7,
            width: 2,
            height: 1,
        }
    );

    let error = CapturedFrame::new(0, 1, Vec::new()).unwrap_err();
    assert_eq!(error, CaptureError::InvalidDimensions);
}
