#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl CapturedFrame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self {
            width,
            height,
            rgba,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    #[error("capture backend unavailable")]
    Unavailable,
    #[error("capture backend error: {0}")]
    Backend(String),
}

pub trait FrameSource {
    fn capture_frame(&mut self) -> Result<CapturedFrame, CaptureError>;
}

pub struct ZoomCapture<S> {
    source: S,
}

impl<S> ZoomCapture<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: FrameSource> ZoomCapture<S> {
    pub fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        self.source.capture_frame().inspect_err(|error| {
            tracing::error!(backend = std::any::type_name::<S>(), ?error, "failed to capture zoom frame");
        })
    }
}
