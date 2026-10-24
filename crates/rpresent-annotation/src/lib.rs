mod catalog;
mod command;
mod controller;
mod document;
mod geometry;
mod history;
mod overlay;
mod text;
mod undo;

pub use catalog::{Preset, Rgb, TOOLS, cycle_index, parse_rgb};
pub use command::{AnnotationCommand, AnnotationCommandParseError};
pub use controller::{AnnotationController, AnnotationState};
pub use document::{Document, Object, ObjectData, ObjectId, Point};
pub use geometry::Rect;
pub use text::{TextDraft, TextSystem};
pub use history::{Stroke, StrokeHistory};
pub use overlay::{OverlayError, run_overlay};
pub use rpresent_config::AnnotationTool;
