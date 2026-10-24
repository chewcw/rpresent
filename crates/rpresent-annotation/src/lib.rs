mod catalog;
mod command;
mod controller;
mod history;
mod overlay;

pub use catalog::{Preset, Rgb, TOOLS, cycle_index, parse_rgb};
pub use command::{AnnotationCommand, AnnotationCommandParseError};
pub use controller::{AnnotationController, AnnotationState};
pub use history::{Stroke, StrokeHistory};
pub use overlay::{OverlayError, run_overlay};
pub use rpresent_config::AnnotationTool;
