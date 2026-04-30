pub mod app;
pub mod command;
pub mod input;
pub mod ipc;
pub mod logging;
#[cfg(target_os = "linux")]
pub(crate) mod x11_input;

pub use app::{run, run_from_env};
