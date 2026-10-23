pub mod command;
pub mod controller;
mod damage;
pub mod overlay;
pub(crate) mod render;
pub(crate) mod shape;

#[cfg(target_os = "linux")]
mod shm;
#[cfg(target_os = "linux")]
mod x11;

pub use command::{CommandParseError, SpotlightCommand};
pub use controller::{SpotlightController, SpotlightState};
pub use overlay::{
    CursorPosition, OverlayError, SpotlightFrame, SpotlightRuntimeMessage, run_overlay,
    spotlight_alpha,
};
pub use render::{build_projecteur_frame, build_projecteur_frame_rect};
