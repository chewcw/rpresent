#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OverlayError {
    #[error("annotation overlay is only supported on Linux")]
    UnsupportedPlatform,
    #[error("annotation backend failed: {0}")]
    Recoverable(String),
    #[error("annotation connection lost: {0}")]
    ConnectionLost(String),
    #[error("annotation runtime error: {0}")]
    Runtime(String),
}

impl OverlayError {
    /// A recoverable failure keeps the document but drops native resources.
    pub fn is_recoverable(&self) -> bool {
        matches!(self, OverlayError::Recoverable(_))
    }
}
