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
pub struct CursorPosition {
    pub x: i16,
    pub y: i16,
}

/// Spotlight overlay frame.  Pixels are stored in **BGRA** premultiplied byte order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightFrame {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RenderInputs {
    surface_size: (u16, u16),
    cursor: CursorPosition,
}

impl SpotlightFrame {
    #[cfg(test)]
    pub(crate) fn alpha_at(&self, x: u16, y: u16) -> u8 {
        let index = ((y as usize * self.width as usize) + x as usize) * 4 + 3;
        self.pixels[index]
    }

    /// Return the raw `[B, G, R, A]` bytes at pixel `(x, y)`.
    pub fn bgra_at(&self, x: u16, y: u16) -> [u8; 4] {
        let i = ((y as usize * self.width as usize) + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }
}

pub(crate) trait SpotlightOverlayBackend {
    fn show(&mut self) -> Result<(), OverlayError>;
    fn hide(&mut self) -> Result<(), OverlayError>;
    fn surface_size(&self) -> Result<(u16, u16), OverlayError>;
    fn cursor_position(&self) -> Result<CursorPosition, OverlayError>;
    fn redraw(
        &mut self,
        cursor: CursorPosition,
        config: &rpresent_config::SpotlightConfig,
    ) -> Result<(), OverlayError>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpotlightRuntimeMessage {
    Visibility(SpotlightState),
    ResizeBy(f32),
}

pub(crate) struct SpotlightOverlayRuntime<B> {
    config: rpresent_config::SpotlightConfig,
    backend: B,
    visible: bool,
    last_render_inputs: Option<RenderInputs>,
    needs_redraw: bool,
}

impl<B: SpotlightOverlayBackend> SpotlightOverlayRuntime<B> {
    pub(crate) fn new(config: rpresent_config::SpotlightConfig, backend: B) -> Self {
        Self {
            config,
            backend,
            visible: false,
            last_render_inputs: None,
            needs_redraw: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn run(
        &mut self,
        rx: Receiver<SpotlightRuntimeMessage>,
    ) -> Result<(), OverlayError> {
        for msg in rx {
            self.process_message(msg)?;
        }
        Ok(())
    }

    pub(crate) fn process_message(
        &mut self,
        message: SpotlightRuntimeMessage,
    ) -> Result<(), OverlayError> {
        match message {
            SpotlightRuntimeMessage::Visibility(state) => self.process_state(state),
            SpotlightRuntimeMessage::ResizeBy(delta) => {
                let next = (self.config.radius_px as f32 + delta)
                    .round()
                    .clamp(24.0, 4096.0) as u32;
                if next != self.config.radius_px {
                    self.config.radius_px = next;
                    self.needs_redraw = true;
                    if self.visible {
                        self.tick()?;
                    }
                }
                Ok(())
            }
        }
    }

    pub(crate) fn process_state(&mut self, state: SpotlightState) -> Result<(), OverlayError> {
        match state {
            SpotlightState::On => {
                if !self.visible {
                    tracing::debug!(state = ?state, "showing spotlight overlay");
                    self.backend.show()?;
                    self.visible = true;
                    self.needs_redraw = true;
                } else {
                    tracing::trace!(state = ?state, "spotlight overlay already visible");
                }

                tracing::trace!(state = ?state, "spotlight overlay redrawing after state update");
                self.tick()?;
            }
            SpotlightState::Off => {
                if self.visible {
                    tracing::debug!(state = ?state, "hiding spotlight overlay");
                    self.backend.hide()?;
                    self.visible = false;
                    self.needs_redraw = false;
                } else {
                    tracing::trace!(state = ?state, "spotlight overlay already hidden");
                }
            }
        }

        Ok(())
    }

    pub(crate) fn tick(&mut self) -> Result<(), OverlayError> {
        if !self.visible {
            return Ok(());
        }

        let inputs = self.current_render_inputs()?;
        if !self.needs_redraw && self.last_render_inputs == Some(inputs) {
            tracing::trace!(
                ?inputs,
                "spotlight overlay inputs unchanged; skipping redraw"
            );
            return Ok(());
        }

        tracing::trace!(?inputs, "spotlight overlay redraw tick");
        self.backend.redraw(inputs.cursor, &self.config)?;
        self.last_render_inputs = Some(inputs);
        self.needs_redraw = false;
        Ok(())
    }

    fn current_render_inputs(&self) -> Result<RenderInputs, OverlayError> {
        Ok(RenderInputs {
            surface_size: self.backend.surface_size()?,
            cursor: self.backend.cursor_position()?,
        })
    }
}

#[cfg(test)]
pub(crate) fn build_spotlight_frame(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    appearance: &SpotlightAppearance,
) -> SpotlightFrame {
    let (width, height) = surface_size;
    let rgba_byte = 4; // 4 bytes per pixel in RGBA format
    let mut pixels = vec![0_u8; width as usize * height as usize * rgba_byte];

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
            let offset = ((y as usize * width as usize) + x as usize) * rgba_byte;
            pixels[offset..offset + rgba_byte].copy_from_slice(&[0, 0, 0, alpha]);
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
    config: rpresent_config::SpotlightConfig,
    rx: Receiver<SpotlightRuntimeMessage>,
) -> Result<(), OverlayError> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    tracing::debug!("starting spotlight overlay");
    let backend = crate::x11::X11SpotlightOverlay::new()?;
    let mut runtime = SpotlightOverlayRuntime::new(config, backend);
    let mut visible = false;

    loop {
        let next_msg = if visible {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => {
                    runtime.tick()?;
                    None
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => break,
            }
        };

        if let Some(msg) = next_msg {
            // ResizeBy does not change visibility — only Visibility messages control the recv-mode timing
            if let SpotlightRuntimeMessage::Visibility(state) = msg {
                visible = matches!(state, SpotlightState::On);
            }
            runtime.process_message(msg)?;
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _config: rpresent_config::SpotlightConfig,
    _rx: Receiver<SpotlightRuntimeMessage>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}

#[cfg(all(target_os = "linux", feature = "qml"))]
pub fn run_overlay_qml(
    config: rpresent_config::SpotlightConfig,
    rx: Receiver<SpotlightRuntimeMessage>,
) -> Result<(), OverlayError> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    tracing::debug!("starting QML spotlight overlay");
    let backend = crate::qml_backend::QmlSpotlightOverlay::new()?;
    let mut runtime = SpotlightOverlayRuntime::new(config, backend);
    let mut visible = false;

    loop {
        let next_msg = if visible {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => {
                    runtime.tick()?;
                    None
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => break,
            }
        };

        if let Some(msg) = next_msg {
            if let SpotlightRuntimeMessage::Visibility(ref state) = msg {
                visible = matches!(state, SpotlightState::On);
            }
            runtime.process_message(msg)?;
        }
    }

    Ok(())
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

        fn redraw(
            &mut self,
            _cursor: CursorPosition,
            _config: &rpresent_config::SpotlightConfig,
        ) -> Result<(), OverlayError> {
            self.calls.push("redraw");
            Ok(())
        }
    }

    #[test]
    fn runtime_shows_redraws_and_hides_on_state_changes() {
        let (tx, rx) = mpsc::channel();
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime =
            SpotlightOverlayRuntime::new(rpresent_config::SpotlightConfig::default(), backend);

        tx.send(SpotlightRuntimeMessage::Visibility(SpotlightState::On))
            .unwrap();
        tx.send(SpotlightRuntimeMessage::Visibility(SpotlightState::Off))
            .unwrap();
        drop(tx);

        runtime.run(rx).unwrap();

        assert_eq!(runtime.backend.calls, vec!["show", "redraw", "hide"]);
    }

    #[test]
    fn tick_does_not_redraw_when_inputs_stay_the_same() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime =
            SpotlightOverlayRuntime::new(rpresent_config::SpotlightConfig::default(), backend);

        runtime.process_state(SpotlightState::On).unwrap();
        runtime.tick().unwrap();

        assert_eq!(runtime.backend.calls, vec!["show", "redraw"]);
    }

    #[test]
    fn tick_redraws_again_when_the_cursor_moves_while_visible() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime =
            SpotlightOverlayRuntime::new(rpresent_config::SpotlightConfig::default(), backend);

        runtime.process_state(SpotlightState::On).unwrap();
        runtime.backend.cursor = CursorPosition { x: 9, y: 8 };
        runtime.tick().unwrap();

        assert_eq!(runtime.backend.calls, vec!["show", "redraw", "redraw"]);
    }

    #[test]
    fn resize_message_updates_radius_and_marks_redraw() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };

        let config = rpresent_config::SpotlightConfig {
            radius_px: 100,
            ..rpresent_config::SpotlightConfig::default()
        };

        let mut runtime = SpotlightOverlayRuntime::new(config, backend);
        runtime
            .process_message(SpotlightRuntimeMessage::Visibility(SpotlightState::On))
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(20.0))
            .unwrap();

        assert_eq!(runtime.config.radius_px, 120);
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
