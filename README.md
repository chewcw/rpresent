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

## Install

Open the repository's GitHub Releases page and download both
`rpresent-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` and `SHA256SUMS`.

Verify the archive, extract it, and install the binary on `PATH`:

```bash
sha256sum -c SHA256SUMS
tar -xzf rpresent-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 rpresent-vX.Y.Z-x86_64-unknown-linux-gnu/rpresent /usr/local/bin/rpresent
```

On Debian and Ubuntu, install the runtime libraries the binary links against:

```bash
sudo apt-get install libx11-6 libgl1
```

The archive also ships the example `config.toml`. Copy it into the config
directory before the first run:

```bash
mkdir -p ~/.config/rpresent
cp rpresent-vX.Y.Z-x86_64-unknown-linux-gnu/config.toml ~/.config/rpresent/config.toml
```

Releases support Linux/X11 on x86_64 only, and they do not support Wayland.
Within an X11 session, transparent compositing still requires a running
compositor, as described under Requirements.

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

### Shortcuts

Shortcuts are configured with two scoped action tables and one wheel modifier:

```toml
[shortcuts.wheel]
modifiers = "Ctrl"

[shortcuts.global]
"spotlight.toggle" = "Ctrl+1"
"annotation.toggle" = "Ctrl+2"
"zoom.toggle" = "Ctrl+3"

[shortcuts.annotation]
"annotation.undo" = "Ctrl+z"
"annotation.redo" = "Ctrl+Z"
"annotation.delete" = "Delete"
"annotation.tool.select" = "v"
"annotation.tool.pen" = "p"
"annotation.tool.line" = "l"
"annotation.tool.eraser" = "e"
"annotation.tool.text" = "t"
"annotation.color.next" = "c"
"annotation.color.prev" = "C"
"annotation.width.prev" = "["
"annotation.width.next" = "]"
"annotation.text-size.prev" = "-"
"annotation.text-size.next" = "="
```

`annotation.clear` has no default binding; add it to the annotation table to enable it. The defaults shown above are also what an omitted scope uses.

- An omitted scope uses defaults. An explicit scope replaces its entire map. An empty scope disables its configurable bindings.
- Character keys are case sensitive. `Super+Z` means the uppercase character, which is Shift plus `z` on the keyboard, and `Ctrl+z` differs from `Ctrl+Z`. Use `Ctrl+Z`, not `Ctrl+Shift+z`, for the default redo binding.
- Wheel combinations use the package modifier names such as `Super+Shift`. `Meta` is not a supported application alias.
- Global bindings take precedence. Annotation canvas bindings never intercept an active text draft.
- Keyboard shortcuts follow the current layout. Caps Lock and Num Lock do not change shortcut matching, while typed text still reflects them.
- A binding this display cannot register, or a grab conflict, is an error. Nothing silently falls back.

The shortcut backend is an implementation boundary used by the Linux/X11 input thread; no other platform is implemented.

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

Spotlight and zoom can be active together. Visual and mouse-input priority follow annotation editing, spotlight, zoom, then the desktop. Finished annotation artwork remains visible and click-through when editing is off.

When spotlight is active, the configured modifier-wheel gesture resizes spotlight. Spotlight consumes left-button dragging and other unsupported mouse actions. Disable spotlight to use zoom wheel and drag controls.

**Change zoom level:**
While zoom owns input, hold the configured wheel modifier (the default is `Ctrl`) and scroll the mouse wheel. Wheel direction reverses immediately, and the pointer position stays anchored in the screenshot. Hold the left mouse button to pan; releasing it continues the pan with time-based inertia.

**Reset zoom:**
```bash
cargo run -p rpresent -- zoom=reset
```
Restores the initial scale and re-centers the magnified view without capturing again. The next activation captures a fresh screenshot.

Activating spotlight or annotation editing cancels the current zoom drag and its remaining pan or scale motion. Input returns to zoom after the higher layer releases it. A new button press is required before zoom can pan again. Global mode shortcuts and explicit CLI/IPC commands remain available.

## Annotations

Press the configured annotation shortcut (the default is `Ctrl+2`) to start editing. The annotation window follows the root desktop coordinates.

While annotation editing owns input, spotlight stays at its last rendered position, including its center dot. Hovering, drawing, moving objects, erasing, and editing text do not move it. When editing stops, spotlight moves to the current pointer position after it regains input ownership. Finished artwork alone does not hold spotlight in place.

- Tools follow the configured bindings; the defaults are `v` Select, `p` Pen, `l` Line, `e` Eraser, and `t` Text.
- `c` cycles colors forward and `C` backward. `[` and `]` cycle stroke widths, and `-` and `=` cycle text sizes.
- `Ctrl+z` undoes, `Ctrl+Z` redoes, and `Delete` removes the selected object. `Ctrl+Y` is not a default redo binding.
- Press Enter to commit text, Shift+Enter to insert a newline, and Escape to cancel the draft.
- `Ctrl+V` pastes clipboard text into the current text draft.
- The eraser deletes whole objects, and drag with the Select tool moves one object.
- Turning annotation off keeps finished artwork visible but click-through. Input returns to spotlight if it is active, then zoom, then the desktop.

A drawing toolbar appears automatically while editing, at the top of the monitor under the pointer (or of the root desktop when monitor information is unavailable). It offers the tools, the configured color, stroke-width, and text-size pickers, and the Undo, Redo, Delete, Clear, and Exit actions. Preset pickers only appear for the active tool, and actions that would do nothing are disabled.

- Drag the handle to move the toolbar. It stays within a visible monitor, can be dragged onto another monitor, and wraps onto extra rows when a monitor is too narrow. Its position is remembered for the current daemon session only and is never saved to disk.
- Clicking a picker opens it; clicking another picker replaces it; clicking outside dismisses it without drawing or changing the selection; Escape closes an open picker before any other Escape behavior.
- Clear erases all committed objects immediately, without a confirmation dialog, and the next Undo restores them exactly.
- Exit stops editing while finished artwork stays visible and click-through.
- Choosing a tool or preset from the toolbar cancels an uncommitted text draft. Canvas shortcut bindings do not apply while a draft is active, so typing a shortcut letter inserts text instead.
- Toolbar Undo and Redo act on the document history; `Ctrl+Z` while editing text remains local to the draft.

Colors can also be selected from the command line; quote the value so the shell keeps the `#`:

```bash
cargo run -p rpresent -- 'annotation=color=#00FF00'
```

Deferred for now: IME input, automatic text wrapping, partial-stroke erasing, and disk persistence.

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
