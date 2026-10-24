# rpresent

`rpresent` is a Linux/X11 presentation shell for toggling spotlight, annotation, and zoom overlays from the command line or through configured shortcuts.

## Workspace Layout

This repository is a Rust workspace with five crates:

- `crates/rpresent-config` - shared TOML config loading, defaults, and config-file writing
- `crates/rpresent-spotlight` - spotlight state and overlay rendering
- `crates/rpresent-annotation` - editable pen, line, text, and eraser annotations
- `crates/rpresent-zoom` - frozen screenshot capture and OpenGL magnified rendering
- `crates/rpresent` - CLI, IPC, shortcut routing, and runtime composition

## Requirements

- Rust 2024 toolchain
- Linux with X11 support

The spotlight overlay uses X11 on Linux. It opens one overlay window, tracks the cursor, and redraws the spotlight mask from the current spotlight state. Transparent compositing requires a running compositor on the desktop session. The software render path requires MIT-SHM 1.2, DAMAGE, and SHAPE 1.1; the daemon exits with an error when a required extension is missing. It writes frames through shared memory and re-renders only the rectangle that the cursor moved across.

## Logging

Logs go to stderr by default. If `RUST_LOG` is unset, `rpresent` falls back to the `info` level.

Example commands for debugging:

```bash
RUST_LOG=debug cargo run -p rpresent -- spot=toggle
RUST_LOG=trace cargo run -p rpresent -- zoom=reset
```

The `config init` command still writes the generated config path to stdout; that output is separate from log messages.

## Build and Test

Run the full workspace test suite:

```bash
cargo test --workspace
```

Format the code:

```bash
cargo fmt
```

## Run

Show the CLI help:

```bash
cargo run -p rpresent -- --help
```

Send a command directly:

```bash
cargo run -p rpresent -- spot=toggle
cargo run -p rpresent -- annotation=tool=line
cargo run -p rpresent -- zoom=reset
```

Start the app without a command to launch the daemon and route configured shortcuts:

```bash
cargo run -p rpresent
```

## Configuration

The app reads its config from the XDG config directory by default. To write a fresh config file:

```bash
cargo run -p rpresent -- config init
```

To choose a specific path:

```bash
cargo run -p rpresent -- config init --config ~/.config/rpresent/config.toml
```

`config init` writes the default config and will not overwrite an existing file.

### Spotlight settings

The spotlight uses one configured radius and one shade opacity for every render path:

- `radius_px` is the spotlight radius in pixels. Wheel resizing changes this live value for circles and other shapes.
- `shade_opacity` is the final shade alpha outside the spotlight fade.
- `softness_px` is the outward fade width in pixels. It affects the shade only.

The built-in defaults are `radius_px = 240`, `shade_opacity = 0.6`, and `softness_px = 80`. The repository example keeps `radius_px = 150`, `shade_opacity = 1.0`, and `softness_px = 50`.

Existing configurations must remove `dim_alpha` and `spot_size_pct`. To migrate a percentage radius, use `max(floor(screen_height * spot_size_pct / 100), 25)` for `radius_px`. If the old XRender opacity is preferred, copy `dim_alpha` to `shade_opacity` before removing it. Removed keys now produce the normal unknown-field parse error.

## Zoom

When the daemon starts without a command it enters an idle loop — no overlay appears until a shortcut or IPC command triggers one.

**Activate zoom mode:**
- Press the configured zoom shortcut (the default is `Ctrl+3`) to toggle zoom on/off.
- Or send a command directly: `cargo run -p rpresent -- zoom=on` / `zoom=off`.

Activation captures the monitor under the pointer once, before the opaque zoom window is shown. The image remains fixed until zoom is closed. Moving the pointer alone does not move the image.

Spotlight and zoom can be active together. The visual order is fixed: annotation, spotlight, zoom, then the desktop. Spotlight remains the same on-screen size while zoom pans or changes scale, and its transparent input region lets zoom receive gestures. The two features can be toggled independently.

**Change zoom level:**
While zoom is active, hold the configured wheel modifier (the default is `Ctrl`) and scroll the mouse wheel. Wheel direction reverses immediately, and the pointer position stays anchored in the screenshot. Hold the left mouse button to pan; releasing it continues the pan with time-based inertia.

**Reset zoom:**
```bash
cargo run -p rpresent -- zoom=reset
```
Restores the initial scale and re-centers the magnified view without capturing again. The next activation captures a fresh screenshot.

With zoom active, the configured wheel gesture controls zoom only (Ctrl+wheel by default). With zoom inactive, the same gesture retains the existing spotlight resize behavior and selected-mode rule.

## Annotations

Press the configured annotation shortcut (the default is `Ctrl+2`) to start editing. The annotation window follows the root desktop coordinates.

- Tools: `v` Select, `p` Pen, `l` Line, `e` Eraser, `t` Text.
- `c` cycles colors (hold Shift to cycle backward). `[` and `]` cycle stroke widths. `-` and `=` cycle text sizes.
- `Ctrl+Z` undoes, `Ctrl+Shift+Z` or `Ctrl+Y` redoes, and `Delete` removes the selected object.
- Press Enter to commit text, Shift+Enter to insert a newline, and Escape to cancel the draft.
- `Ctrl+V` pastes clipboard text into the current text draft.
- The eraser deletes whole objects, and drag with the Select tool moves one object.
- Turning annotation off keeps finished artwork visible but click-through, and releases input to zoom or the desktop.

Colors can also be selected from the command line; quote the value so the shell keeps the `#`:

```bash
cargo run -p rpresent -- 'annotation=color=#00FF00'
```

Deferred for now: a toolbar, IME input, automatic text wrapping, partial-stroke erasing, and disk persistence.

**Known limitations:**
- Linux/X11 only; Wayland is not supported.
- Linux/X11 GLX with OpenGL 3.3-compatible shaders is required. The renderer uses an opaque window and does not require a compositor.
- `follow_cursor`, `pan_friction`, and `capture_target` are removed configuration keys. Use `drag_friction` and `scale_friction` for motion damping.

## Notes

The top-level command parser accepts these routed commands:

- `spot=...`
- `annotation=...`
- `zoom=...`

Use `--config` with the main CLI to point at a different configuration file when running or sending commands.
