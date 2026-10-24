#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl CapturedFrame {
    /// Validates a full-monitor screenshot: nonzero dimensions and exactly
    /// `width * height * 4` BGRA bytes.
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Result<Self, CaptureError> {
        if width == 0 || height == 0 {
            return Err(CaptureError::InvalidDimensions);
        }

        let pixel_count = width as u64 * height as u64;
        let expected = pixel_count
            .checked_mul(4)
            .ok_or(CaptureError::InvalidDimensions)? as usize;

        if bgra.len() != expected {
            return Err(CaptureError::InvalidByteLength {
                expected,
                actual: bgra.len(),
                width,
                height,
            });
        }

        Ok(Self {
            width,
            height,
            bgra,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    #[error("capture backend unavailable")]
    Unavailable,
    #[error("capture backend error: {0}")]
    Backend(String),
    #[error("capture dimensions must be nonzero")]
    InvalidDimensions,
    #[error(
        "capture frame must be {expected} bytes for {width}x{height} BGRA pixels, got {actual}"
    )]
    InvalidByteLength {
        expected: usize,
        actual: usize,
        width: u32,
        height: u32,
    },
}
