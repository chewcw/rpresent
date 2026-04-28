# rpresent Spotlight X11 Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dummy spotlight overlay with a Linux/X11 renderer that opens one transparent overlay window, tracks the cursor with x11rb, and paints the spotlight mask from the existing spotlight state stream.

**Architecture:** Keep `SpotlightController` and `SpotlightAppearance` as the control-plane edge. Add a small backend-agnostic runtime in `overlay.rs` so the state machine and frame math can be tested without X11, then implement a private `X11SpotlightOverlay` backend in `src/x11.rs` that uses x11rb to create the window, choose an alpha-capable 32-bit visual when available, query pointer position, and upload the spotlight frame bytes. `run_overlay` stays the public entrypoint, and the non-Linux stub remains unchanged.

**Tech Stack:** Rust 2024, `std::sync::mpsc`, `x11rb` with the `render` feature, `thiserror`, `cargo test`.

**Non-goals:** presenter hardware support, keyboard shortcut routing, tray UI, repeated reconnect loops, or a cross-platform overlay backend beyond Linux/X11.

**Status:** Task 1 is complete in `crates/rpresent-spotlight/src/overlay.rs`; Task 2 implementation and validation are complete in `crates/rpresent-spotlight/src/x11.rs`, with the commit step still pending.

---

### Task 1: Split the overlay loop into a testable runtime and pure frame math

**Files:**
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Modify: `crates/rpresent-spotlight/tests/overlay.rs`

**Why this task exists:** The current file only consumes spotlight state and discards it. Before any X11 code lands, the overlay needs a small runtime that can be tested with a fake backend, plus a pure frame builder that turns cursor position and `SpotlightAppearance` into pixels.

- [x] **Step 1: Write the failing tests**

`crates/rpresent-spotlight/src/overlay.rs`

```rust
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
```

`crates/rpresent-spotlight/tests/overlay.rs`

```rust
use rpresent_config::SpotlightConfig;
use rpresent_spotlight::{SpotlightAppearance, spotlight_alpha};

#[test]
fn alpha_is_full_inside_the_radius_and_fades_linearly() {
    assert_eq!(spotlight_alpha(0.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(200.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(220.0, 200.0, 40.0), 0.5);
    assert_eq!(spotlight_alpha(240.0, 200.0, 40.0), 0.0);
    assert_eq!(spotlight_alpha(260.0, 200.0, 40.0), 0.0);
}

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = SpotlightAppearance::default();

    assert_eq!(appearance.radius_px, 240);
    assert_eq!(appearance.softness_px, 80);
    assert!((appearance.dim_alpha - 0.72).abs() < f32::EPSILON);
}

#[test]
fn appearance_can_be_built_from_config() {
    let appearance = SpotlightAppearance::from(SpotlightConfig {
        enabled_on_start: false,
        radius_px: 300,
        softness_px: 90,
        dim_alpha: 0.8,
    });

    assert_eq!(appearance.radius_px, 300);
    assert_eq!(appearance.softness_px, 90);
    assert!((appearance.dim_alpha - 0.8).abs() < f32::EPSILON);
}
```

- [x] **Step 2: Run the tests to verify they fail before implementation**

Run: `cargo test -p rpresent-spotlight --lib`

Expected: fail with unresolved names for `SpotlightOverlayRuntime`, `SpotlightOverlayBackend`, `CursorPosition`, `SpotlightFrame`, and `build_spotlight_frame`.

- [x] **Step 3: Write the minimal runtime and frame builder**

`crates/rpresent-spotlight/src/overlay.rs`

```rust
use std::sync::mpsc::Receiver;

use crate::controller::SpotlightState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    #[cfg(test)]
    fn alpha_at(&self, x: u16, y: u16) -> u8 {
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

    #[cfg(test)]
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
```

- [x] **Step 4: Run the targeted library tests again**

Run: `cargo test -p rpresent-spotlight --lib`

Expected: PASS, with the runtime tests proving that `On` shows and redraws, `Off` hides, and the frame builder centers the spotlight on the cursor.

- [x] **Step 5: Commit the slice**

```bash
git add crates/rpresent-spotlight/src/overlay.rs crates/rpresent-spotlight/tests/overlay.rs
git commit -m "test: add spotlight overlay runtime seam"
```

Completed in commit `a9bc146` after `cargo test -p rpresent-spotlight --lib` passed.

### Task 2: Add the X11 backend and wire it into the runtime loop

**Files:**
- Modify: `crates/rpresent-spotlight/Cargo.toml`
- Modify: `crates/rpresent-spotlight/src/lib.rs`
- Modify: `crates/rpresent-spotlight/src/overlay.rs`
- Create: `crates/rpresent-spotlight/src/x11.rs`
- Modify: `README.md`

**Why this task exists:** The runtime seam from Task 1 is what makes the X11 work small. This task plugs x11rb into that seam, keeps the backend Linux-only, and preserves the existing `run_overlay` API and non-Linux stub.

- [x] **Step 1: Write the failing backend tests**

`crates/rpresent-spotlight/src/x11.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_window_spec_matches_screen_geometry() {
        let spec = overlay_window_spec((1920, 1080));

        assert_eq!(spec.x, 0);
        assert_eq!(spec.y, 0);
        assert_eq!(spec.width, 1920);
        assert_eq!(spec.height, 1080);
        assert!(spec.override_redirect);
        assert!(spec.input_transparent);
    }

    #[test]
    fn frame_to_put_image_bytes_keeps_the_alpha_payload() {
        let frame = SpotlightFrame {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 0, 255, 0, 0, 0, 0],
        };

        assert_eq!(frame_to_put_image_bytes(&frame), frame.pixels);
    }
}
```

- [x] **Step 2: Run the backend tests before the implementation exists**

Run: `cargo test -p rpresent-spotlight --lib overlay_window_spec_matches_screen_geometry`

Expected: fail because `overlay_window_spec`, `frame_to_put_image_bytes`, and `X11SpotlightOverlay` do not exist yet.

- [x] **Step 3: Implement the x11rb backend and connect it to the runtime**

`crates/rpresent-spotlight/Cargo.toml`

```toml
[package]
name = "rpresent-spotlight"
version = "0.1.0"
edition = "2024"

[dependencies]
rpresent-config = { path = "../rpresent-config" }
thiserror = "2"
x11rb = { version = "0.13", features = ["render"] }
```

`crates/rpresent-spotlight/src/lib.rs`

```rust
pub mod command;
pub mod controller;
pub mod overlay;

pub use command::{CommandParseError, SpotlightCommand};
pub use controller::{SpotlightController, SpotlightState};
pub use overlay::{OverlayError, SpotlightAppearance, run_overlay, spotlight_alpha};

#[cfg(target_os = "linux")]
mod x11;
```

`crates/rpresent-spotlight/src/overlay.rs`

```rust
#[cfg(target_os = "linux")]
pub fn run_overlay(
    appearance: SpotlightAppearance,
    rx: Receiver<SpotlightState>,
) -> Result<(), OverlayError> {
    use crate::controller::SpotlightState;
    use std::sync::mpsc::RecvTimeoutError;
    use std::time::Duration;

    let backend = crate::x11::X11SpotlightOverlay::new()?;
    let mut runtime = SpotlightOverlayRuntime::new(appearance, backend);
    let mut visible = false;

    loop {
        let next_state = if visible {
            match rx.recv_timeout(Duration::from_millis(16)) {
                Ok(state) => Some(state),
                Err(RecvTimeoutError::Timeout) => {
                    runtime.tick()?;
                    None
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(state) => Some(state),
                Err(_) => break,
            }
        };

        if let Some(state) = next_state {
            let next_visible = matches!(state, SpotlightState::On);
            runtime.process_state(state)?;
            visible = next_visible;
        }
    }

    Ok(())
}
```

`crates/rpresent-spotlight/src/x11.rs`

```rust
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConnectionExt, CreateWindowAux, EventMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::overlay::{CursorPosition, OverlayError, SpotlightFrame, SpotlightOverlayBackend};

struct OverlayWindowSpec {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    override_redirect: bool,
    input_transparent: bool,
}

fn overlay_window_spec(surface_size: (u16, u16)) -> OverlayWindowSpec {
    OverlayWindowSpec {
        x: 0,
        y: 0,
        width: surface_size.0,
        height: surface_size.1,
        override_redirect: true,
        input_transparent: true,
    }
}

fn frame_to_put_image_bytes(frame: &SpotlightFrame) -> Vec<u8> {
    frame.pixels.clone()
}

pub(crate) struct X11SpotlightOverlay {
    connection: RustConnection,
    screen_num: usize,
    window: x11rb::protocol::xproto::Window,
    surface_size: (u16, u16),
}

impl X11SpotlightOverlay {
    pub(crate) fn new() -> Result<Self, OverlayError> {
        let (connection, screen_num) = x11rb::connect(None)
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;
        let screen = &connection.setup().roots[screen_num];
        let spec = overlay_window_spec((screen.width_in_pixels, screen.height_in_pixels));

        let window = connection
            .generate_id()
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;

        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                screen.root,
                spec.x,
                spec.y,
                spec.width,
                spec.height,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(spec.override_redirect)
                    .background_pixel(screen.black_pixel)
                    .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;

        Ok(Self {
            connection,
            screen_num,
            window,
            surface_size: (spec.width, spec.height),
        })
    }

    fn flush(&self) -> Result<(), OverlayError> {
        self.connection
            .flush()
            .map_err(|error| OverlayError::Runtime(error.to_string()))
    }
}

impl SpotlightOverlayBackend for X11SpotlightOverlay {
    fn show(&mut self) -> Result<(), OverlayError> {
        self.connection
            .map_window(self.window)
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;
        self.flush()
    }

    fn hide(&mut self) -> Result<(), OverlayError> {
        self.connection
            .unmap_window(self.window)
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;
        self.flush()
    }

    fn surface_size(&self) -> Result<(u16, u16), OverlayError> {
        Ok(self.surface_size)
    }

    fn cursor_position(&self) -> Result<CursorPosition, OverlayError> {
        let screen = &self.connection.setup().roots[self.screen_num];
        let reply = self
            .connection
            .query_pointer(screen.root)
            .map_err(|error| OverlayError::Runtime(error.to_string()))?
            .reply()
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;

        Ok(CursorPosition {
            x: reply.root_x,
            y: reply.root_y,
        })
    }

    fn redraw(&mut self, frame: &SpotlightFrame) -> Result<(), OverlayError> {
        let bytes = frame_to_put_image_bytes(frame);
        self.connection
            .put_image(
                x11rb::protocol::xproto::ImageFormat::Z_PIXMAP,
                self.window,
                self.connection.setup().roots[self.screen_num].default_gc,
                frame.width,
                frame.height,
                0,
                0,
                0,
                32,
                &bytes,
            )
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;
        self.flush()
    }
}
```

The runtime loop stays one-shot and simple: create the backend once, then on each `SpotlightState::On` draw immediately and on a 16 ms active timeout refresh the frame from the current cursor position. The backend should create its alpha-capable visual, colormap, and GC once, and map every X11 error into `OverlayError::Runtime(error.to_string())` rather than retrying.

- [x] **Step 4: Add the Linux/X11 note and validate the crate**

`README.md`

```md
## Spotlight Overlay

The spotlight overlay uses X11 on Linux. It opens one overlay window, tracks the cursor, and redraws the spotlight mask from the current spotlight state. Transparent compositing requires a running compositor on the desktop session.
```

Run:

```bash
cargo test -p rpresent-spotlight
```

Expected: PASS.

If you have an interactive X11 session available, run a manual smoke check in two terminals:

```bash
cargo run -p rpresent
```

and then:

```bash
cargo run -p rpresent -- spot=toggle
```

Expected: the daemon stays up, the command exits successfully, and the spotlight window appears and disappears on the X11 desktop.

Validation completed: `cargo test -p rpresent-spotlight --lib` and `cargo test -p rpresent-spotlight` both passed.

- [ ] **Step 5: Commit the slice**

```bash
git add crates/rpresent-spotlight/Cargo.toml crates/rpresent-spotlight/src/lib.rs crates/rpresent-spotlight/src/overlay.rs crates/rpresent-spotlight/src/x11.rs README.md
git commit -m "feat: wire spotlight overlay to x11rb"
```