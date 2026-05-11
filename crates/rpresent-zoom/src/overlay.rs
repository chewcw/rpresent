use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};

use crate::controller::ZoomRuntimeMessage;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomAppearance {
    pub initial_scale: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub scroll_speed: f32,
    pub pan_friction: f32,
    pub scale_friction: f32,
    pub follow_cursor: bool,
}

impl Default for ZoomAppearance {
    fn default() -> Self {
        Self {
            initial_scale: 2.0,
            min_scale: 1.0,
            max_scale: 6.0,
            scroll_speed: 0.25,
            pan_friction: 0.82,
            scale_friction: 5.0,
            follow_cursor: true,
        }
    }
}

impl From<rpresent_config::ZoomConfig> for ZoomAppearance {
    fn from(config: rpresent_config::ZoomConfig) -> Self {
        Self {
            initial_scale: config.initial_scale,
            min_scale: config.min_scale,
            max_scale: config.max_scale,
            scroll_speed: config.scroll_speed,
            pan_friction: config.pan_friction,
            scale_friction: config.scale_friction,
            follow_cursor: config.follow_cursor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

pub fn zoom_sample_rect(
    frame_width: u32,
    frame_height: u32,
    scale: f32,
    position: (f32, f32),
) -> ZoomRect {
    let width = frame_width as f32 / scale;
    let height = frame_height as f32 / scale;
    let max_left = (frame_width as f32 - width).max(0.0);
    let max_top = (frame_height as f32 - height).max(0.0);
    let left = (position.0 - width / 2.0).clamp(0.0, max_left);
    let top = (position.1 - height / 2.0).clamp(0.0, max_top);

    ZoomRect {
        left,
        top,
        width,
        height,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OverlayError {
    #[error("overlay is only supported on Linux in this slice")]
    UnsupportedPlatform,
    #[error("overlay runtime error: {0}")]
    Runtime(String),
}

#[cfg(target_os = "linux")]
pub fn run_overlay(
    appearance: ZoomAppearance,
    rx: Receiver<ZoomRuntimeMessage>,
) -> Result<(), OverlayError> {
    use crate::x11::X11ZoomOverlay;

    tracing::debug!(?appearance, "starting zoom overlay");

    let mut backend = X11ZoomOverlay::new().inspect_err(|e| {
        tracing::error!(?e, "failed to initialise zoom X11 backend");
    })?;

    let frame_budget = Duration::from_micros(16_667);
    let mut last_tick = Instant::now();
    let mut disconnected = false;

    while !disconnected {
        let frame_start = Instant::now();

        loop {
            match rx.try_recv() {
                Ok(message) => backend.apply_message(message)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        let now = Instant::now();
        let dt = (now - last_tick).as_secs_f32().max(1.0 / 240.0);
        last_tick = now;

        backend.tick(dt)?;

        let elapsed = frame_start.elapsed();
        if elapsed < frame_budget {
            std::thread::sleep(frame_budget - elapsed);
        }
    }

    // Clean up: hide the overlay before dropping the backend.
    let _ = backend.hide();
    tracing::debug!("zoom overlay exiting");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _appearance: ZoomAppearance,
    _rx: Receiver<ZoomRuntimeMessage>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
