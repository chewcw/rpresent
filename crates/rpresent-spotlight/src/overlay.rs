use std::sync::Arc;
use std::sync::mpsc::Receiver;

#[cfg(test)]
use crate::controller::SpotlightState;

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

pub(crate) fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CursorPosition {
    pub x: i16,
    pub y: i16,
}

/// Spotlight overlay frame. Pixels are stored in **BGRA** premultiplied byte order.
///
/// `width` and `height` describe the rendered region. A partial redraw returns only the
/// damage rectangle, so the caller must place the pixels at the rectangle origin.
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
    fn window_id(&self) -> u32;
    fn show(&mut self) -> Result<(), OverlayError>;
    fn hide(&mut self) -> Result<(), OverlayError>;
    fn surface_size(&self) -> Result<(u16, u16), OverlayError>;
    fn cursor_position(&self) -> Result<CursorPosition, OverlayError>;
    fn redraw(
        &mut self,
        cursor: CursorPosition,
        config: &rpresent_config::SpotlightConfig,
        radius_px: u32,
    ) -> Result<(), OverlayError>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpotlightRuntimeMessage {
    Present { operation: u64, visible: bool },
    SetFollowing { operation: u64, following: bool },
    ResizeBy(f32),
}

#[derive(Debug)]
pub enum SpotlightEvent {
    Ready {
        window: u32,
    },
    Presented {
        operation: u64,
        visible: bool,
        result: Result<(), String>,
    },
    FollowingApplied {
        operation: u64,
        following: bool,
        result: Result<(), String>,
    },
    Exited(Result<(), String>),
}

pub type SpotlightEventSink = Arc<dyn Fn(SpotlightEvent) + Send + Sync>;

pub(crate) struct SpotlightOverlayRuntime<B> {
    config_service: Arc<rpresent_config::ConfigService>,
    radius_px: u32,
    backend: B,
    visible: bool,
    following: bool,
    last_render_inputs: Option<RenderInputs>,
    needs_redraw: bool,
}

impl<B: SpotlightOverlayBackend> SpotlightOverlayRuntime<B> {
    pub(crate) fn new(config_service: Arc<rpresent_config::ConfigService>, backend: B) -> Self {
        let radius_px = config_service.spotlight().radius_px;
        Self {
            config_service,
            radius_px,
            backend,
            visible: false,
            following: false,
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
            SpotlightRuntimeMessage::Present { visible, .. } => self.process_visibility(visible),
            SpotlightRuntimeMessage::SetFollowing { following, .. } => {
                self.following = following;
                if following && self.visible {
                    self.tick()?;
                }
                Ok(())
            }
            SpotlightRuntimeMessage::ResizeBy(delta) => {
                let next = (self.radius_px as f32 + delta).round().clamp(24.0, 4096.0) as u32;
                if next != self.radius_px {
                    self.radius_px = next;
                    self.needs_redraw = true;
                    if self.visible {
                        self.tick()?;
                    }
                }
                Ok(())
            }
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible
    }

    fn process_visibility(&mut self, visible: bool) -> Result<(), OverlayError> {
        if visible {
            if !self.visible {
                self.backend.show()?;
                self.visible = true;
                self.needs_redraw = true;
            }
            self.tick()
        } else {
            if self.visible {
                self.backend.hide()?;
                self.visible = false;
                self.needs_redraw = false;
            }
            Ok(())
        }
    }

    #[cfg(test)]
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
        self.backend.redraw(
            inputs.cursor,
            self.config_service.spotlight(),
            self.radius_px,
        )?;
        self.last_render_inputs = Some(inputs);
        self.needs_redraw = false;
        Ok(())
    }

    fn current_render_inputs(&self) -> Result<RenderInputs, OverlayError> {
        let cursor = match (self.following, self.last_render_inputs) {
            (false, Some(previous)) => previous.cursor,
            _ => self.backend.cursor_position()?,
        };
        Ok(RenderInputs {
            surface_size: self.backend.surface_size()?,
            cursor,
        })
    }

    fn process_request(
        &mut self,
        message: SpotlightRuntimeMessage,
        events: &SpotlightEventSink,
    ) -> Result<(), OverlayError> {
        let (present, set_following) = match &message {
            SpotlightRuntimeMessage::Present { operation, visible } => {
                (Some((*operation, *visible)), None)
            }
            SpotlightRuntimeMessage::SetFollowing { operation, .. } => (None, Some(*operation)),
            SpotlightRuntimeMessage::ResizeBy(_) => (None, None),
        };
        let result = self.process_message(message);
        let acknowledgment = result.as_ref().map(|_| ()).map_err(ToString::to_string);
        if let Some((operation, visible)) = present {
            events(SpotlightEvent::Presented {
                operation,
                visible,
                result: acknowledgment,
            });
        } else if let Some(operation) = set_following {
            events(SpotlightEvent::FollowingApplied {
                operation,
                following: self.following,
                result: acknowledgment,
            });
        }
        result
    }
}

#[cfg(test)]
pub(crate) fn build_spotlight_frame(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    radius_px: u32,
    softness_px: u32,
    shade_opacity: f32,
) -> SpotlightFrame {
    let (width, height) = surface_size;
    let rgba_byte = 4; // 4 bytes per pixel in RGBA format
    let mut pixels = vec![0_u8; width as usize * height as usize * rgba_byte];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cursor.x as f32;
            let dy = y as f32 - cursor.y as f32;
            let distance = (dx * dx + dy * dy).sqrt();
            let spot = spotlight_alpha(distance, radius_px as f32, softness_px as f32);
            let dim = shade_opacity * (1.0 - spot);
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
    config_service: Arc<rpresent_config::ConfigService>,
    rx: Receiver<SpotlightRuntimeMessage>,
    events: SpotlightEventSink,
) -> Result<(), OverlayError> {
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    tracing::debug!("starting spotlight overlay");
    let backend = match crate::x11::X11SpotlightOverlay::new() {
        Ok(backend) => backend,
        Err(error) => {
            events(SpotlightEvent::Exited(Err(error.to_string())));
            return Err(error);
        }
    };
    events(SpotlightEvent::Ready {
        window: backend.window_id(),
    });
    let mut runtime = SpotlightOverlayRuntime::new(config_service, backend);

    loop {
        let next_msg = if runtime.is_visible() {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(msg) => Some(msg),
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(error) = runtime.tick() {
                        events(SpotlightEvent::Exited(Err(error.to_string())));
                        return Err(error);
                    }
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

        if let Some(msg) = next_msg
            && let Err(error) = runtime.process_request(msg, &events)
        {
            events(SpotlightEvent::Exited(Err(error.to_string())));
            return Err(error);
        }
    }

    events(SpotlightEvent::Exited(Ok(())));
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _config_service: Arc<rpresent_config::ConfigService>,
    _rx: Receiver<SpotlightRuntimeMessage>,
    events: SpotlightEventSink,
) -> Result<(), OverlayError> {
    let error = OverlayError::UnsupportedPlatform;
    events(SpotlightEvent::Exited(Err(error.to_string())));
    Err(error)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};

    use super::*;

    fn config_service(
        config: rpresent_config::SpotlightConfig,
    ) -> std::sync::Arc<rpresent_config::ConfigService> {
        std::sync::Arc::new(
            rpresent_config::ConfigService::new(
                std::path::PathBuf::from("test.toml"),
                rpresent_config::Config {
                    spotlight: config,
                    ..Default::default()
                },
            )
            .unwrap(),
        )
    }

    #[derive(Debug, Default)]
    struct FakeBackend {
        window_id: u32,
        calls: Vec<&'static str>,
        redraw_radii: Vec<u32>,
        surface_size: (u16, u16),
        cursor: CursorPosition,
        cursor_queries: Cell<usize>,
        redraw_positions: Vec<CursorPosition>,
        fail_redraw: bool,
        trace: Arc<Mutex<Vec<&'static str>>>,
    }

    impl SpotlightOverlayBackend for FakeBackend {
        fn window_id(&self) -> u32 {
            self.window_id
        }

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
            self.cursor_queries.set(self.cursor_queries.get() + 1);
            Ok(self.cursor)
        }

        fn redraw(
            &mut self,
            cursor: CursorPosition,
            _config: &rpresent_config::SpotlightConfig,
            radius_px: u32,
        ) -> Result<(), OverlayError> {
            self.calls.push("redraw");
            if self.fail_redraw {
                return Err(runtime_error("injected redraw failure"));
            }
            self.redraw_radii.push(radius_px);
            self.redraw_positions.push(cursor);
            self.trace.lock().unwrap().push("redraw");
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
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            backend,
        );

        tx.send(SpotlightRuntimeMessage::Present {
            operation: 1,
            visible: true,
        })
        .unwrap();
        tx.send(SpotlightRuntimeMessage::Present {
            operation: 2,
            visible: false,
        })
        .unwrap();
        drop(tx);

        runtime.run(rx).unwrap();

        assert_eq!(runtime.backend.calls, vec!["show", "redraw", "hide"]);
        assert_eq!(
            runtime.backend.redraw_radii,
            vec![rpresent_config::SpotlightConfig::default().radius_px]
        );
    }

    #[test]
    fn tick_does_not_redraw_when_inputs_stay_the_same() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            backend,
        );

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
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            backend,
        );

        runtime.process_state(SpotlightState::On).unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 1,
                following: true,
            })
            .unwrap();
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

        let config_service = config_service(rpresent_config::SpotlightConfig {
            radius_px: 100,
            ..rpresent_config::SpotlightConfig::default()
        });

        let mut runtime =
            SpotlightOverlayRuntime::new(std::sync::Arc::clone(&config_service), backend);
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 1,
                visible: true,
            })
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(20.0))
            .unwrap();

        assert_eq!(runtime.radius_px, 120);
        assert_eq!(runtime.backend.calls, vec!["show", "redraw", "redraw"]);
        assert_eq!(runtime.backend.redraw_radii, vec![100, 120]);
    }

    #[test]
    fn hidden_resize_is_forwarded_on_next_activation() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig {
                radius_px: 100,
                ..Default::default()
            }),
            backend,
        );

        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(20.0))
            .unwrap();
        assert!(runtime.backend.redraw_radii.is_empty());

        runtime.process_state(SpotlightState::On).unwrap();

        assert_eq!(runtime.backend.redraw_radii, vec![120]);
    }

    #[test]
    fn wheel_resize_clamps_the_live_radius_at_both_bounds() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig {
                radius_px: 100,
                ..Default::default()
            }),
            backend,
        );

        runtime.process_state(SpotlightState::On).unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(-1000.0))
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(10000.0))
            .unwrap();

        assert_eq!(runtime.backend.redraw_radii, vec![100, 24, 4096]);
    }

    #[test]
    fn spotlight_frame_is_centered_on_the_cursor() {
        let frame = build_spotlight_frame((16, 16), CursorPosition { x: 8, y: 8 }, 4, 2, 0.72);

        assert_eq!(frame.width, 16);
        assert_eq!(frame.height, 16);
        assert_eq!(frame.alpha_at(8, 8), 0);
        assert!(frame.alpha_at(13, 8) > frame.alpha_at(8, 8));
        assert!(frame.alpha_at(15, 8) >= frame.alpha_at(13, 8));
        assert!(frame.alpha_at(15, 15) > frame.alpha_at(8, 8));
    }

    #[test]
    fn resize_changes_the_runtime_radius_but_not_the_service_config() {
        let backend = FakeBackend {
            surface_size: (16, 16),
            cursor: CursorPosition { x: 8, y: 8 },
            ..Default::default()
        };
        let config_service = config_service(rpresent_config::SpotlightConfig {
            radius_px: 100,
            ..rpresent_config::SpotlightConfig::default()
        });

        let mut runtime =
            SpotlightOverlayRuntime::new(std::sync::Arc::clone(&config_service), backend);
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 1,
                visible: true,
            })
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(20.0))
            .unwrap();

        assert_eq!(runtime.radius_px, 120);
        assert_eq!(config_service.spotlight().radius_px, 100);
    }

    #[test]
    fn capture_hiding_preserves_the_live_radius() {
        let backend = FakeBackend {
            surface_size: (80, 60),
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            backend,
        );
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(12.0))
            .unwrap();
        let radius = runtime.radius_px;
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 1,
                visible: true,
            })
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 2,
                visible: false,
            })
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 3,
                visible: true,
            })
            .unwrap();
        assert_eq!(runtime.radius_px, radius);
        assert!(runtime.visible);
    }

    #[test]
    fn tick_redraws_when_the_surface_size_changes_without_cursor_motion() {
        let backend = FakeBackend {
            surface_size: (400, 300),
            cursor: CursorPosition { x: 200, y: 150 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            backend,
        );
        runtime.process_state(SpotlightState::On).unwrap();
        runtime.backend.surface_size = (600, 400);
        runtime.tick().unwrap();
        runtime.backend.surface_size = (300, 200);
        runtime.tick().unwrap();
        runtime.tick().unwrap();
        assert_eq!(
            runtime.backend.calls,
            vec!["show", "redraw", "redraw", "redraw"]
        );
    }

    fn visible_runtime() -> SpotlightOverlayRuntime<FakeBackend> {
        let backend = FakeBackend {
            surface_size: (800, 600),
            cursor: CursorPosition { x: 100, y: 100 },
            ..Default::default()
        };
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            backend,
        );
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 1,
                visible: true,
            })
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 2,
                following: true,
            })
            .unwrap();
        runtime
    }

    #[test]
    fn suspension_preserves_the_last_render_without_querying_the_pointer() {
        let mut runtime = visible_runtime();
        let retained = runtime.last_render_inputs.unwrap();
        let queries = runtime.backend.cursor_queries.get();
        let draws = runtime.backend.redraw_positions.len();
        runtime.backend.cursor = CursorPosition { x: 600, y: 400 };
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 3,
                following: false,
            })
            .unwrap();
        for _ in 0..8 {
            runtime.tick().unwrap();
        }
        assert_eq!(runtime.last_render_inputs, Some(retained));
        assert_eq!(runtime.backend.cursor_queries.get(), queries);
        assert_eq!(runtime.backend.redraw_positions.len(), draws);
    }

    #[test]
    fn held_redraw_and_hide_show_keep_the_rendered_position() {
        let mut runtime = visible_runtime();
        let retained = runtime.last_render_inputs.unwrap().cursor;
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 3,
                following: false,
            })
            .unwrap();
        let queries = runtime.backend.cursor_queries.get();
        runtime.backend.cursor = CursorPosition { x: 600, y: 400 };
        runtime.backend.surface_size = (1024, 768);
        runtime.tick().unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::ResizeBy(12.0))
            .unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 4,
                visible: false,
            })
            .unwrap();
        runtime.tick().unwrap();
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 5,
                visible: true,
            })
            .unwrap();
        assert_eq!(runtime.backend.cursor_queries.get(), queries);
        assert!(
            runtime
                .backend
                .redraw_positions
                .iter()
                .all(|p| *p == retained)
        );
        assert_eq!(
            runtime.last_render_inputs.unwrap().surface_size,
            (1024, 768)
        );
    }

    #[test]
    fn held_first_presentation_initializes_once_then_resume_samples_now() {
        let mut runtime = SpotlightOverlayRuntime::new(
            config_service(rpresent_config::SpotlightConfig::default()),
            FakeBackend {
                surface_size: (800, 600),
                cursor: CursorPosition { x: 100, y: 100 },
                ..Default::default()
            },
        );
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 1,
                following: false,
            })
            .unwrap();
        assert_eq!(runtime.backend.cursor_queries.get(), 0);
        runtime
            .process_message(SpotlightRuntimeMessage::Present {
                operation: 2,
                visible: true,
            })
            .unwrap();
        for _ in 0..8 {
            runtime.tick().unwrap();
        }
        assert_eq!(runtime.backend.cursor_queries.get(), 1);
        runtime.backend.cursor = CursorPosition { x: 600, y: 400 };
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 3,
                following: true,
            })
            .unwrap();
        assert_eq!(runtime.backend.cursor_queries.get(), 2);
        assert_eq!(
            runtime.backend.redraw_positions.last(),
            Some(&runtime.backend.cursor)
        );
    }

    #[test]
    fn following_acknowledgment_reports_redraw_failure() {
        let mut runtime = visible_runtime();
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 3,
                following: false,
            })
            .unwrap();
        runtime.backend.cursor = CursorPosition { x: 600, y: 400 };
        runtime.backend.fail_redraw = true;
        let previous = runtime.last_render_inputs;
        let (tx, rx) = mpsc::channel();
        let events: SpotlightEventSink = Arc::new(move |event| {
            tx.send(event).unwrap();
        });
        assert!(
            runtime
                .process_request(
                    SpotlightRuntimeMessage::SetFollowing {
                        operation: 4,
                        following: true,
                    },
                    &events
                )
                .is_err()
        );
        assert!(matches!(
            rx.try_recv().unwrap(),
            SpotlightEvent::FollowingApplied {
                operation: 4,
                following: true,
                result: Err(_),
            }
        ));
        assert_eq!(runtime.last_render_inputs, previous);
    }

    #[test]
    fn successful_resume_redraws_before_acknowledgment() {
        let mut runtime = visible_runtime();
        runtime
            .process_message(SpotlightRuntimeMessage::SetFollowing {
                operation: 3,
                following: false,
            })
            .unwrap();
        runtime.backend.cursor = CursorPosition { x: 600, y: 400 };
        let trace = Arc::clone(&runtime.backend.trace);
        trace.lock().unwrap().clear();
        let callback_trace = Arc::clone(&trace);
        let (tx, rx) = mpsc::channel();
        let events: SpotlightEventSink = Arc::new(move |event| {
            callback_trace.lock().unwrap().push("acknowledgment");
            tx.send(event).unwrap();
        });
        runtime
            .process_request(
                SpotlightRuntimeMessage::SetFollowing {
                    operation: 4,
                    following: true,
                },
                &events,
            )
            .unwrap();
        assert_eq!(*trace.lock().unwrap(), vec!["redraw", "acknowledgment"]);
        assert!(matches!(
            rx.try_recv().unwrap(),
            SpotlightEvent::FollowingApplied {
                operation: 4,
                following: true,
                result: Ok(()),
            }
        ));
        assert_eq!(
            runtime.last_render_inputs.unwrap().cursor,
            runtime.backend.cursor
        );
        assert_eq!(
            runtime.backend.redraw_positions.last(),
            Some(&runtime.backend.cursor)
        );
    }
}
