pub mod command;
pub mod controller;
pub mod overlay;
pub(crate) mod render;

#[cfg(target_os = "linux")]
mod x11;

#[cfg(feature = "qml")]
pub(crate) mod qml_backend;
#[cfg(feature = "qml")]
pub use qml_backend::{IpcMessage, QmlSpotlightOverlay};

pub use command::{CommandParseError, SpotlightCommand};
pub use controller::{SpotlightController, SpotlightState};
pub use overlay::{CursorPosition, OverlayError, SpotlightAppearance, SpotlightFrame, run_overlay, spotlight_alpha};
pub use render::build_projecteur_frame;
