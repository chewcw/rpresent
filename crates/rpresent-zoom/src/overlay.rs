use crossbeam_channel::Receiver;

use crate::controller::ZoomState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomAppearance {
    pub initial_scale: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub scroll_speed: f32,
    pub pan_friction: f32,
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

pub fn zoom_sample_rect(frame_width: u32, frame_height: u32, state: &ZoomState) -> ZoomRect {
    let width = frame_width as f32 / state.scale;
    let height = frame_height as f32 / state.scale;
    let max_left = (frame_width as f32 - width).max(0.0);
    let max_top = (frame_height as f32 - height).max(0.0);
    let left = (state.pivot.0 - width / 2.0).clamp(0.0, max_left);
    let top = (state.pivot.1 - height / 2.0).clamp(0.0, max_top);

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
    rx: Receiver<ZoomState>,
) -> Result<(), OverlayError> {
    let _ = appearance;

    for state in rx {
        let _ = zoom_sample_rect(1920, 1080, &state);
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _appearance: ZoomAppearance,
    _rx: Receiver<ZoomState>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
