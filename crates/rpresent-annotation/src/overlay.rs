use std::sync::Arc;
use std::sync::mpsc::Receiver;

use crate::controller::AnnotationState;

pub use crate::error::OverlayError;

#[cfg(target_os = "linux")]
pub fn run_overlay(
    config_service: Arc<rpresent_config::ConfigService>,
    rx: Receiver<AnnotationState>,
) -> Result<(), OverlayError> {
    tracing::debug!(
        stroke_width_px = config_service.annotation().stroke_width_px,
        "starting annotation overlay"
    );

    let mut visible = false;

    for state in rx {
        let next_visible = matches!(state, AnnotationState::On);
        if next_visible != visible {
            tracing::debug!(state = ?state, visible = next_visible, "annotation overlay visibility changed");
            visible = next_visible;
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _config_service: Arc<rpresent_config::ConfigService>,
    _rx: Receiver<AnnotationState>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
