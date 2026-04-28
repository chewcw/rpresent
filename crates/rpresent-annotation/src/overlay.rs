use std::sync::mpsc::Receiver;

use crate::controller::AnnotationState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnnotationAppearance {
    pub stroke_width_px: u32,
    pub history_depth: usize,
    pub canvas_alpha: f32,
}

impl Default for AnnotationAppearance {
    fn default() -> Self {
        Self {
            stroke_width_px: 6,
            history_depth: 64,
            canvas_alpha: 0.2,
        }
    }
}

impl From<rpresent_config::AnnotationConfig> for AnnotationAppearance {
    fn from(config: rpresent_config::AnnotationConfig) -> Self {
        Self {
            stroke_width_px: config.stroke_width_px,
            history_depth: config.history_depth,
            canvas_alpha: 0.2,
        }
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
    appearance: AnnotationAppearance,
    rx: Receiver<AnnotationState>,
) -> Result<(), OverlayError> {
    let _ = appearance;

    for state in rx {
        let visible = matches!(state, AnnotationState::On);
        let _ = visible;
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _appearance: AnnotationAppearance,
    _rx: Receiver<AnnotationState>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
