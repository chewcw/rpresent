pub mod app;
pub mod command;
pub mod coordinator;
pub mod input;
pub mod ipc;
pub mod logging;
pub mod native_input;
#[cfg(target_os = "linux")]
pub(crate) mod native_keys;
pub mod overlay_lifecycle;
#[cfg(target_os = "linux")]
pub(crate) mod x11_input;
#[cfg(target_os = "linux")]
pub(crate) mod x11_shortcuts;

pub use app::{run, run_from_env};
