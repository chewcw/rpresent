use std::sync::mpsc::Receiver;

use crate::controller::SpotlightState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpotlightAppearance {
    pub radius_px: u32,
    pub softness_px: u32,
    pub dim_alpha: f32,
}

impl Default for SpotlightAppearance {
    fn default() -> Self {
        Self {
            radius_px: 240,
            softness_px: 80,
            dim_alpha: 0.72,
        }
    }
}

impl From<rpresent_config::SpotlightConfig> for SpotlightAppearance {
    fn from(config: rpresent_config::SpotlightConfig) -> Self {
        Self {
            radius_px: config.radius_px,
            softness_px: config.softness_px,
            dim_alpha: config.dim_alpha,
        }
    }
}

pub fn spotlight_alpha(distance_px: f32, radius_px: f32, softness_px: f32) -> f32 {
    if distance_px <= radius_px {
        1.0
    } else if distance_px >= radius_px + softness_px {
        0.0
    } else {
        1.0 - ((distance_px - radius_px) / softness_px)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OverlayError {
    #[error("overlay is only supported on Linux in this slice")]
    UnsupportedPlatform,
    #[error("overlay runtime error: {0}")]
    Runtime(String),
}

#[cfg(target_os = "linux")]
pub fn run_overlay(
    appearance: SpotlightAppearance,
    rx: Receiver<SpotlightState>,
) -> Result<(), OverlayError> {
    for state in rx {
        if matches!(state, SpotlightState::On) {
            let _ = spotlight_alpha(
                0.0,
                appearance.radius_px as f32,
                appearance.softness_px as f32,
            );
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _appearance: SpotlightAppearance,
    _rx: Receiver<SpotlightState>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
