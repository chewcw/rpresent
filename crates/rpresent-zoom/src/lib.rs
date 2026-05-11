pub mod capture;
pub mod command;
pub mod controller;
pub mod navigation;
pub mod overlay;
pub mod screenshot;

#[cfg(target_os = "linux")]
pub(crate) mod x11;

pub use capture::{CaptureError, CapturedFrame, FrameSource, ZoomCapture};
pub use command::{CommandParseError, ZoomCommand};
pub use controller::{ZoomController, ZoomRuntimeConfig, ZoomRuntimeMessage};
pub use overlay::{OverlayError, ZoomAppearance, ZoomRect, run_overlay, zoom_sample_rect};
pub use rpresent_config::ZoomConfig;
