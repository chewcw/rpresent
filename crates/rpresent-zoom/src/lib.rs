pub mod capture;
pub mod command;
pub mod navigation;
pub mod session;
pub mod transport;

#[cfg(target_os = "linux")]
pub(crate) mod glx;

#[cfg(target_os = "linux")]
pub(crate) mod native;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OverlayError {
    #[error("overlay is only supported on Linux in this slice")]
    UnsupportedPlatform,
    #[error("overlay runtime error: {0}")]
    Runtime(String),
}

pub use capture::{CaptureError, CapturedFrame};
pub use command::{CommandParseError, ZoomCommand};
pub use rpresent_config::ZoomConfig;
pub use session::{MonitorRect, PointerInput, SessionEffect, ZoomRequest, ZoomSession};
pub use transport::{ZoomHandle, ZoomInbox, channel};

#[cfg(target_os = "linux")]
pub use native::run_overlay;

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _config_service: std::sync::Arc<rpresent_config::ConfigService>,
    _inbox: ZoomInbox,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
