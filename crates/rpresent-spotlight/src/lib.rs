pub mod command;
pub mod controller;
pub mod overlay;

#[cfg(target_os = "linux")]
mod x11;

pub use command::{CommandParseError, SpotlightCommand};
pub use controller::{SpotlightController, SpotlightState};
pub use overlay::{OverlayError, SpotlightAppearance, run_overlay, spotlight_alpha};
