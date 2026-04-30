pub mod command;
pub mod controller;
pub mod overlay;
pub(crate) mod render;

#[cfg(target_os = "linux")]
mod x11;

pub use command::{CommandParseError, SpotlightCommand};
pub use controller::{SpotlightController, SpotlightState};
pub use overlay::{
    CursorPosition, OverlayError, SpotlightAppearance, SpotlightFrame, SpotlightRuntimeMessage,
    run_overlay, spotlight_alpha,
};
pub use render::build_projecteur_frame;
