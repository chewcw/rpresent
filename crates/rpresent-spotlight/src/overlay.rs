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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CursorPosition {
    pub(crate) x: i16,
    pub(crate) y: i16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpotlightFrame {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) pixels: Vec<u8>,
}

impl SpotlightFrame {
    pub(crate) fn alpha_at(&self, x: u16, y: u16) -> u8 {
        let index = ((y as usize * self.width as usize) + x as usize) * 4 + 3;
        self.pixels[index]
    }
}

pub(crate) trait SpotlightOverlayBackend {
    fn show(&mut self) -> Result<(), OverlayError>;
    fn hide(&mut self) -> Result<(), OverlayError>;
    fn surface_size(&self) -> Result<(u16, u16), OverlayError>;
    fn cursor_position(&self) -> Result<CursorPosition, OverlayError>;
    fn redraw(&mut self, frame: &SpotlightFrame) -> Result<(), OverlayError>;
}

pub(crate) struct SpotlightOverlayRuntime<B> {
    appearance: SpotlightAppearance,
    backend: B,
    visible: bool,
}

impl<B: SpotlightOverlayBackend> SpotlightOverlayRuntime<B> {
    pub(crate) fn new(appearance: SpotlightAppearance, backend: B) -> Self {
        Self {
            appearance,
            backend,
            visible: false,
        }
    }

    pub(crate) fn run(&mut self, rx: Receiver<SpotlightState>) -> Result<(), OverlayError> {
        for state in rx {
            self.process_state(state)?;
        }

        Ok(())
    }

    pub(crate) fn process_state(&mut self, state: SpotlightState) -> Result<(), OverlayError> {
        match state {
            SpotlightState::On => {
                if !self.visible {
                    self.backend.show()?;
                    self.visible = true;
                }

                self.tick()?;
            }
            SpotlightState::Off => {
                if self.visible {
                    self.backend.hide()?;
                    self.visible = false;
                }
            }
        }

        Ok(())
    }

    pub(crate) fn tick(&mut self) -> Result<(), OverlayError> {
        if !self.visible {
            return Ok(());
        }

        let frame = build_spotlight_frame(
            self.backend.surface_size()?,
            self.backend.cursor_position()?,
            &self.appearance,
        );
        self.backend.redraw(&frame)?;
        Ok(())
    }
}

fn build_spotlight_frame(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    appearance: &SpotlightAppearance,
) -> SpotlightFrame {
    let (width, height) = surface_size;
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cursor.x as f32;
            let dy = y as f32 - cursor.y as f32;
            let distance = (dx * dx + dy * dy).sqrt();
            let spot = spotlight_alpha(
                distance,
                appearance.radius_px as f32,
                appearance.softness_px as f32,
            );
            let dim = appearance.dim_alpha * (1.0 - spot);
            let alpha = (255.0 * dim.clamp(0.0, 1.0)) as u8;
            let offset = ((y as usize * width as usize) + x as usize) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[0, 0, 0, alpha]);
        }
    }

    SpotlightFrame {
        width,
        height,
        pixels,
    }
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[derive(Debug, Default)]
    struct FakeBackend {
        calls: Vec<&'static str>,
        surface_size: (u16, u16),
        cursor: CursorPosition,
    }

    impl SpotlightOverlayBackend for FakeBackend {
        fn show(&mut self) -> Result<(), OverlayError> {
            self.calls.push("show");
            Ok(())
        }

        fn hide(&mut self) -> Result<(), OverlayError> {
            self.calls.push("hide");
            Ok(())
        }

        fn surface_size(&self) -> Result<(u16, u16), OverlayError> {
            Ok(self.surface_size)
        }

        fn cursor_position(&self) -> Result<CursorPosition, OverlayError> {
            Ok(self.cursor)
        }

        fn redraw(&mut self, frame: &SpotlightFrame) -> Result<(), OverlayError> {
            self.calls.push("redraw");
            assert_eq!(frame.width, self.surface_size.0);
            assert_eq!(frame.height, self.surface_size.1);
            Ok(())
        }
    }

    #[test]
    fn runtime_shows_redraws_and_hides_on_state_changes() {
        let appearance = SpotlightAppearance {
            radius_px: 4,
            softness_px: 2,
            dim_alpha: 0.72,
        };
        let (tx, rx) = mpsc::channel();
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(appearance, backend);

        tx.send(SpotlightState::On).unwrap();
        tx.send(SpotlightState::Off).unwrap();
        drop(tx);

        runtime.run(rx).unwrap();

        assert_eq!(runtime.backend.calls, vec!["show", "redraw", "hide"]);
    }

    #[test]
    fn tick_redraws_again_while_the_spotlight_is_visible() {
        let appearance = SpotlightAppearance {
            radius_px: 4,
            softness_px: 2,
            dim_alpha: 0.72,
        };
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(appearance, backend);

        runtime.process_state(SpotlightState::On).unwrap();
        runtime.tick().unwrap();

        assert_eq!(runtime.backend.calls, vec!["show", "redraw", "redraw"]);
    }

    #[test]
    fn spotlight_frame_is_centered_on_the_cursor() {
        let frame = build_spotlight_frame(
            (16, 16),
            CursorPosition { x: 8, y: 8 },
            &SpotlightAppearance {
                radius_px: 4,
                softness_px: 2,
                dim_alpha: 0.72,
            },
        );

        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 16);
        assert_eq!(frame.alpha_at(8, 8), 0);
        assert!(frame.alpha_at(13, 8) > frame.alpha_at(8, 8));
        assert!(frame.alpha_at(15, 8) >= frame.alpha_at(13, 8));
        assert!(frame.alpha_at(15, 15) > frame.alpha_at(8, 8));
    }
}
