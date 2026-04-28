pub mod capture;
pub mod command;
pub mod controller;
pub mod overlay;

pub use capture::{CaptureError, CapturedFrame, FrameSource, ZoomCapture};
pub use command::{CommandParseError, ZoomCommand};
pub use controller::{ZoomController, ZoomState};
pub use overlay::{OverlayError, ZoomAppearance, ZoomRect, run_overlay, zoom_sample_rect};
pub use rpresent_config::ZoomConfig;
