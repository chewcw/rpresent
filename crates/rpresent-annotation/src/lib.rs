mod catalog;
mod command;
mod document;
mod error;
mod geometry;
mod render;
mod runtime;
mod session;
mod text;
mod toolbar;
mod transport;
mod undo;
mod x11_clipboard;

#[cfg(target_os = "linux")]
mod x11;

pub use catalog::{Preset, Rgb, TOOLS, cycle_index, parse_rgb};
pub use command::{AnnotationCommand, AnnotationCommandParseError};
pub use document::{Document, Object, ObjectData, ObjectId, Point};
pub use error::OverlayError;
pub use geometry::Rect;
pub use render::Renderer;
pub use rpresent_config::AnnotationTool;
pub use runtime::{
    AnnotationEvent, AnnotationEventSink, AnnotationRequest, AnnotationStatus,
    run_overlay as run_annotation_overlay,
};
pub use session::{KeyInput, Modifiers, PointerInput, Session, SessionStatus};
pub use text::{TextDraft, TextSystem};
pub use transport::{AnnotationHandle, AnnotationInbox, channel};
