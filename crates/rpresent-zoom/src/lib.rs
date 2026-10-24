pub mod capture;
pub mod command;
pub mod controller;
pub mod navigation;
pub mod overlay;
pub mod session;
pub mod transport;

#[cfg(target_os = "linux")]
pub(crate) mod x11;

pub use capture::{CaptureError, CapturedFrame, FrameSource, ZoomCapture};
pub use command::{CommandParseError, ZoomCommand};
pub use controller::{ZoomController, ZoomState};
pub use overlay::{OverlayError, ZoomRect, run_overlay, zoom_sample_rect};
pub use rpresent_config::ZoomConfig;
pub use session::{MonitorRect, PointerInput, SessionEffect, ZoomRequest, ZoomSession};
pub use transport::{ZoomHandle, ZoomInbox, channel};
