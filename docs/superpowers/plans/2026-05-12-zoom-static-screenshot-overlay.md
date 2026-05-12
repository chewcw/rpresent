# Zoom GLX Texture Overlay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the sluggish CPU zoom compositor with a frozen fullscreen screenshot rendered as an OpenGL texture in an X11/GLX overlay window. The screenshot is captured once when zoom turns on, uploaded once into a texture, and every subsequent frame only updates uniforms and swaps buffers.

**Architecture:** Keep the existing command and input surfaces, but change the X11 backend from “recapture the root window every frame” to “capture once, cache, and render the cached frame through the current camera transform.” Split the backend so `x11.rs` owns X11 window creation, message handling, capture-on-enable, and camera state, while a new `glx.rs` module owns the GLX context, shader program, texture upload, and draw call. `overlay.rs` stays a pure math helper that converts camera state into screen-space placement for the shader. This removes the current per-frame tiny-skia resample, CPU buffer copies, and `PutImage` upload that are making zoom feel sluggish.

**Tech Stack:** Rust, `x11rb` for capture, `x11` plus GLX/Xlib FFI for the overlay context, `glow` for OpenGL bindings, `crossbeam-channel`, existing tests.

**Deferred optimization:** Keep the initial screenshot capture on `x11rb::get_image` for the first slice. If enabling zoom is still expensive on the target hardware after the GLX rewrite, add an `XShm` capture path as a follow-up; do not start there because it is not the steady-state bottleneck.

---

### File map

- `crates/rpresent-zoom/src/x11.rs`: backend state, capture-on-enable flow, event handling, camera updates, show/hide.
- `crates/rpresent-zoom/src/glx.rs`: new GLX renderer, shader sources, texture upload, draw/swap.
- `crates/rpresent-zoom/src/overlay.rs`: `ScreenshotPlacement` math only; delete the crop-based helpers.
- `crates/rpresent-zoom/src/screenshot.rs`: one-shot refresh gate.
- `crates/rpresent-zoom/src/lib.rs`: add the internal `glx` module declaration and stop re-exporting deleted crop helpers.
- `crates/rpresent-zoom/Cargo.toml`: add `glow` and `x11`, remove `tiny-skia` after the CPU compositor is gone.
- `crates/rpresent-zoom/tests/overlay.rs`, `tests/screenshot.rs`, `tests/controller.rs`: regression guards for placement math, one-shot refresh, and command routing.
- `README.md`: mention that the zoom backend now needs GLX/OpenGL support on Linux.

---

### Task 1: Lock the current contract in tests

**Files:**
- `crates/rpresent-zoom/tests/overlay.rs`
- `crates/rpresent-zoom/tests/screenshot.rs`
- `crates/rpresent-zoom/tests/controller.rs`

- [ ] **Step 1: Keep the frozen-placement assertions in place**

```rust
use rpresent_zoom::{ScreenshotPlacement, ZoomAppearance, ZoomConfig, screenshot_placement};

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = ZoomAppearance::default();
    assert_eq!(appearance.initial_scale, 2.0);
    assert_eq!(appearance.min_scale, 1.0);
    assert_eq!(appearance.max_scale, 6.0);
    assert_eq!(appearance.scroll_speed, 0.25);
    assert_eq!(appearance.pan_friction, 0.82);
    assert_eq!(appearance.scale_friction, 5.0);
    assert!(appearance.follow_cursor);
}

#[test]
fn zoom_appearance_from_config_matches_fields() {
    let config = ZoomConfig {
        initial_scale: 3.0,
        min_scale: 1.5,
        max_scale: 8.0,
        scroll_speed: 0.5,
        pan_friction: 0.9,
        scale_friction: 6.0,
        follow_cursor: false,
        ..ZoomConfig::default()
    };
    let appearance = ZoomAppearance::from(config);
    assert_eq!(appearance.initial_scale, 3.0);
    assert_eq!(appearance.min_scale, 1.5);
    assert_eq!(appearance.max_scale, 8.0);
    assert_eq!(appearance.scroll_speed, 0.5);
    assert_eq!(appearance.pan_friction, 0.9);
    assert_eq!(appearance.scale_friction, 6.0);
    assert!(!appearance.follow_cursor);
}

#[test]
fn screenshot_placement_centers_the_frozen_frame_at_1x() {
    let placement = screenshot_placement(1920, 1080, 1.0, (960.0, 540.0));

    assert_eq!(
        placement,
        ScreenshotPlacement {
            translate_x: 0.0,
            translate_y: 0.0,
            scale: 1.0,
        }
    );
}

#[test]
fn screenshot_placement_preserves_drag_offset_at_1x() {
    let placement = screenshot_placement(1920, 1080, 1.0, (1000.0, 500.0));

    assert_eq!(
        placement,
        ScreenshotPlacement {
            translate_x: -40.0,
            translate_y: 40.0,
            scale: 1.0,
        }
    );
}
```

```rust
use rpresent_zoom::{CaptureMode, ScreenshotState};

#[test]
fn static_mode_requires_explicit_refresh_request() {
    let mut s = ScreenshotState::new(CaptureMode::Static);
    assert!(!s.should_refresh(1.0 / 60.0));

    s.request_refresh();
    assert!(s.should_refresh(1.0 / 60.0));
    assert!(!s.should_refresh(1.0 / 60.0));
}

#[test]
fn live_mode_refreshes_by_interval() {
    let mut s = ScreenshotState::new(CaptureMode::Live { hz: 30 });
    assert!(!s.should_refresh(1.0 / 120.0));

    let mut refreshed = false;
    for _ in 0..10 {
        if s.should_refresh(1.0 / 120.0) {
            refreshed = true;
            break;
        }
    }
    assert!(refreshed);
}
```

```rust
use rpresent_config::ZoomConfig;
use rpresent_zoom::{ZoomCommand, ZoomController, ZoomRuntimeMessage};

#[test]
fn initial_messages_include_config_and_enabled_state() {
    let config = ZoomConfig {
        enabled_on_start: true,
        initial_scale: 3.0,
        min_scale: 1.5,
        max_scale: 8.0,
        scroll_speed: 0.4,
        pan_friction: 3.2,
        scale_friction: 6.1,
        follow_cursor: false,
        capture_target: "active-monitor".to_owned(),
    };
    let controller = ZoomController::from_config(config.clone());

    let messages = controller.initial_messages();
    assert_eq!(
        messages[0],
        ZoomRuntimeMessage::Configure(rpresent_zoom::ZoomRuntimeConfig {
            initial_scale: config.initial_scale,
            min_scale: config.min_scale,
            max_scale: config.max_scale,
            scroll_speed: config.scroll_speed,
            pan_friction: config.pan_friction,
            scale_friction: config.scale_friction,
            follow_cursor: config.follow_cursor,
            capture_target: config.capture_target,
        })
    );
    assert_eq!(messages[1], ZoomRuntimeMessage::SetEnabled(true));
}

#[test]
fn zoom_and_drag_messages_require_enabled_zoom() {
    let mut controller = ZoomController::from_config(ZoomConfig {
        enabled_on_start: false,
        ..ZoomConfig::default()
    });

    assert_eq!(controller.zoom_by_scroll(1.0, (320.0, 200.0)), None);
    assert_eq!(controller.drag_start((10.0, 20.0)), None);
    assert_eq!(controller.drag_move((15.0, 25.0)), None);
    assert_eq!(controller.drag_end(), None);

    assert_eq!(controller.apply(ZoomCommand::On), Some(ZoomRuntimeMessage::SetEnabled(true)));
    assert_eq!(
        controller.zoom_by_scroll(-1.0, (400.0, 300.0)),
        Some(ZoomRuntimeMessage::Scroll {
            delta: -1.0,
            pivot: (400.0, 300.0),
        })
    );
    assert_eq!(
        controller.drag_start((1.0, 2.0)),
        Some(ZoomRuntimeMessage::DragStart { x: 1.0, y: 2.0 })
    );
    assert_eq!(
        controller.drag_move((3.0, 4.0)),
        Some(ZoomRuntimeMessage::DragMove { x: 3.0, y: 4.0 })
    );
    assert_eq!(controller.drag_end(), Some(ZoomRuntimeMessage::DragEnd));
}

#[test]
fn command_application_emits_expected_runtime_messages() {
    let mut controller = ZoomController::from_config(ZoomConfig::default());

    assert_eq!(controller.apply(ZoomCommand::On), Some(ZoomRuntimeMessage::SetEnabled(true)));
    assert_eq!(controller.apply(ZoomCommand::On), None);
    assert_eq!(controller.apply(ZoomCommand::Toggle), Some(ZoomRuntimeMessage::SetEnabled(false)));
    assert_eq!(controller.apply(ZoomCommand::Off), None);
    assert_eq!(controller.apply(ZoomCommand::Reset), Some(ZoomRuntimeMessage::Reset));
}
```

- [ ] **Step 2: Run the regression tests before touching the renderer**

Run: `cargo test -p rpresent-zoom --test overlay -v`

Run: `cargo test -p rpresent-zoom --test screenshot -v`

Run: `cargo test -p rpresent-zoom --test controller -v`

Expected: all three test files pass, which confirms the placement math, one-shot refresh gate, and command routing are stable before the backend swap.

### Task 2: Add the GLX renderer module

**Files:**
- `crates/rpresent-zoom/Cargo.toml`
- `crates/rpresent-zoom/src/glx.rs` *(new)*
- `crates/rpresent-zoom/src/lib.rs`

- [ ] **Step 1: Add the GLX/OpenGL dependencies and module declaration**

```toml
[dependencies]
rpresent-config = { path = "../rpresent-config" }
tracing = { workspace = true }
thiserror = "2"
crossbeam-channel = "0.5"
x11rb = { version = "0.13", features = ["render", "image"] }
glow = "0.16"
x11 = "2"
```

```rust
pub(crate) mod glx;
```

- [ ] **Step 2: Implement a dedicated renderer that uploads the screenshot once and draws a quad each frame**

```rust
use glow::HasContext;
use x11::glx;
use x11::xlib;

use crate::capture::CapturedFrame;
use crate::overlay::{OverlayError, ScreenshotPlacement};

pub(crate) struct GlxRenderer {
    display: *mut xlib::Display,
    window: xlib::Window,
    context: glx::GLXContext,
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    texture: Option<glow::Texture>,
    u_screen_size: glow::UniformLocation,
    u_frame_size: glow::UniformLocation,
    u_translate: glow::UniformLocation,
    u_scale: glow::UniformLocation,
}

impl GlxRenderer {
    pub unsafe fn new(display: *mut xlib::Display, window: xlib::Window) -> Result<Self, OverlayError>;

    pub fn upload_frame(&mut self, frame: &CapturedFrame) -> Result<(), OverlayError>;

    pub fn draw(
        &self,
        placement: ScreenshotPlacement,
        screen_size: (i32, i32),
    ) -> Result<(), OverlayError>;
}
```

```rust
const VERTEX_SHADER: &str = r#"#version 330 core
layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_uv;

uniform vec2 u_screen_size;
uniform vec2 u_frame_size;
uniform vec2 u_translate;
uniform float u_scale;

out vec2 v_uv;

void main() {
    vec2 pixel_pos = a_pos * u_frame_size * u_scale + u_translate;
    vec2 ndc = vec2(
        (pixel_pos.x / u_screen_size.x) * 2.0 - 1.0,
        1.0 - (pixel_pos.y / u_screen_size.y) * 2.0
    );
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = a_uv;
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 330 core
in vec2 v_uv;
uniform sampler2D u_texture;
out vec4 color;

void main() {
    color = texture(u_texture, v_uv);
}
"#;
```

`new()` should open the display, choose a 32-bit double-buffered GLX FBConfig, create the context against the existing overlay window, load GL symbols through `glXGetProcAddressARB`, compile the shaders, set up the quad buffers, and initialize `u_texture` to texture unit 0. `upload_frame()` should call `glTexImage2D` once per enable, keep the texture alive until zoom is disabled, and use `GL_LINEAR` filtering to match the current bilinear feel. `draw()` should clear black, bind the texture, set the four uniforms, draw the quad, and finish with `glXSwapBuffers`.

- [ ] **Step 3: Build the crate after the renderer module lands**

Run: `cargo check -p rpresent-zoom`

Run: `cargo test -p rpresent-zoom --lib -v`

Expected: the crate compiles with the new renderer module wired in, even before the X11 backend starts using it.

### Task 3: Wire the X11 backend to the renderer and drop the CPU compositor

**Files:**
- `crates/rpresent-zoom/src/x11.rs`
- `crates/rpresent-zoom/src/overlay.rs`

- [ ] **Step 1: Replace the CPU image upload state with a GLX renderer**

```rust
pub(crate) struct X11ZoomOverlay {
    connection: RustConnection,
    capture: ZoomCapture<X11FrameSource>,
    renderer: GlxRenderer,
    window: Window,
    screen_width: u16,
    screen_height: u16,
    enabled: bool,
    runtime: ZoomRuntimeConfig,
    camera: Camera,
    dragging: bool,
    last_cursor: (f32, f32),
    capture_state: ScreenshotState,
    visible: bool,
}
```

Remove the `gc`, `image_buffer`, `scaled_frame`, `depth`, and tiny-skia-specific imports from `x11.rs` once the renderer owns the drawing path.

- [ ] **Step 2: Upload the frozen screenshot exactly once when zoom turns on**

```rust
ZoomRuntimeMessage::SetEnabled(enabled) => {
    if enabled {
        self.capture_state.request_refresh();
        if self.capture_state.should_refresh(0.0) {
            let frame = self.capture.next_frame().map_err(runtime_error)?;
            self.renderer.upload_frame(&frame)?;
        }
        self.enabled = true;
        self.show()?;
    } else {
        self.enabled = false;
        self.dragging = false;
        self.hide()?;
    }
}
```

- [ ] **Step 3: Replace `render_current_frame` with a GPU draw call**

```rust
pub(crate) fn tick(&mut self, dt: f32) -> Result<(), OverlayError> {
    if !self.enabled {
        return Ok(());
    }

    let nav_cfg = NavigationConfig {
        min_scale: self.runtime.min_scale,
        max_scale: self.runtime.max_scale,
        drag_friction: self.runtime.pan_friction,
        scale_friction: self.runtime.scale_friction,
    };
    let mouse = MouseState {
        drag: self.dragging,
    };
    self.camera.update(
        &nav_cfg,
        dt,
        &mouse,
        (self.screen_width as f32, self.screen_height as f32),
    );

    let placement = screenshot_placement(
        self.screen_width as u32,
        self.screen_height as u32,
        self.camera.scale,
        self.camera.position,
    );

    self.renderer
        .draw(placement, (self.screen_width as i32, self.screen_height as i32))
}
```

This should be the only render path left: black clear, textured quad, buffer swap. The CPU path that currently does `PixmapRef::draw_pixmap`, buffer copies, and `PutImage` should be deleted entirely.

- [ ] **Step 4: Keep the drag and wheel interactions unchanged**

Do not add a new input mode. The existing `Scroll`, `DragStart`, `DragMove`, `DragEnd`, and `Reset` messages already map cleanly to the frozen overlay model; only the renderer changes.

- [ ] **Step 5: Validate the renderer swap with the real app**

Run: `cargo test -p rpresent --lib -v`

Run: `RUST_LOG=debug cargo run -p rpresent`

In a second terminal, run: `cargo run -p rpresent -- zoom=toggle`

Expected: the overlay appears as a black fullscreen window with the frozen screenshot drawn in it, dragging moves the screenshot smoothly, and the mouse wheel changes zoom without the old sluggish CPU redraw.

Run: `cargo run -p rpresent -- zoom=off`

Expected: the overlay hides and the next `zoom=toggle` starts from a fresh screenshot.

### Task 4: Remove the dead crop helpers and finish cleanup

**Files:**
- `crates/rpresent-zoom/src/overlay.rs`
- `crates/rpresent-zoom/src/lib.rs`
- `crates/rpresent-zoom/Cargo.toml`
- `README.md`

- [ ] **Step 1: Delete the crop-based zoom helper types**

Remove `ZoomRect` and `zoom_sample_rect` from `overlay.rs`. After the GLX rewrite, the zoom module no longer needs to compute a cropped live view; it only needs the placement helper for the frozen screenshot texture.

Keep `ScreenshotPlacement` and `screenshot_placement` exactly as they are because the shader still needs a screen-space translate and scale.

- [ ] **Step 2: Update the public re-exports**

Remove `ZoomRect` from `crates/rpresent-zoom/src/lib.rs` so the public surface only exposes the frozen-overlay helpers that still exist.

- [ ] **Step 3: Trim the crate dependencies**

Delete `tiny-skia` from `crates/rpresent-zoom/Cargo.toml` once no module still imports it. If the GLX renderer is the only draw path, this dependency should disappear completely.

- [ ] **Step 4: Update the README requirement note**

Add one short note to the zoom section in `README.md` that says the backend now depends on working X11/GLX/OpenGL support on Linux. Keep the user-facing zoom behavior text the same; only the runtime requirement changes.

```markdown
## Zoom

When zoom is enabled, rpresent captures a fullscreen screenshot of the current X11 desktop and shows it on top of a black overlay.

While zoom is active:
- Drag with the left mouse button to move the frozen screenshot around.
- Use the mouse wheel to zoom the frozen screenshot in and out.
- `zoom=reset` restores the default camera scale and centers the frozen screenshot again.

Known limitations:
- Linux/X11 only; Wayland is not supported.
- An ARGB32-capable compositor must be running for overlay transparency to render correctly.
- The zoom backend also needs working GLX/OpenGL support on the Linux desktop session.
```

- [ ] **Step 5: Format and run the final regression pass**

Run: `cargo fmt`

Run: `cargo test -p rpresent-zoom --lib -v`

Run: `cargo test -p rpresent --lib -v`

Expected: the workspace still passes the library tests, the new GLX backend is wired in, and the plan can be handed off for implementation.

## Execution Status

```text
## Execution Status
- Status: Not started
```

## Blockers

- None.

**Next Steps:** Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement the plan one task at a time.
