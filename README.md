# rpresent

`rpresent` is a Linux/X11 presentation daemon with three overlays:

- spotlight, which follows the pointer and shades the rest of the screen
- annotation, which provides pen, line, arrow, rectangle, ellipse, text, and eraser tools
- zoom, which magnifies a captured monitor image

The daemon runs in the background. The CLI sends commands to it over a Unix socket, and configured X11 shortcuts can send the same commands.

## Workspace layout

- `crates/rpresent-config` loads TOML configuration, applies defaults, and validates shortcuts.
- `crates/rpresent-spotlight` renders the spotlight overlay.
- `crates/rpresent-annotation` manages drawing objects, text editing, undo and redo, and the annotation toolbar.
- `crates/rpresent-zoom` captures a monitor and renders the magnified image with GLX/OpenGL.
- `crates/rpresent` contains the CLI, Unix-socket IPC, shortcut routing, and runtime coordinator.

## Requirements

- Linux with an X11 session
- Rust 2024 toolchain
- X11 development libraries and `pkg-config` to build
- An X11 compositor for the transparent spotlight and annotation windows
- GLX 1.3 and OpenGL 3.3 with GLSL 330 support for zoom

The spotlight backend also requires these X11 extensions:

- DAMAGE
- MIT-SHM 1.2
- SHAPE 1.1

The server must provide a 32-bit-per-pixel visual. Circular spotlights use XRender when the required visual is available. Other spotlight shapes use the MIT-SHM renderer.

Wayland is not supported. The release workflow builds an x86_64 Linux binary.

## Build and test

Run the workspace tests:

```bash
cargo test --workspace --locked
```

Format the code:

```bash
cargo fmt
```

The CI job also runs Clippy with warnings treated as errors:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

## Install a release

Download the release archive and `SHA256SUMS` from the repository's GitHub Releases page. Verify and extract the archive:

```bash
sha256sum -c SHA256SUMS
tar -xzf rpresent-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 755 rpresent-vX.Y.Z-x86_64-unknown-linux-gnu/rpresent /usr/local/bin/rpresent
```

On Debian or Ubuntu, install the runtime libraries:

```bash
sudo apt-get install libx11-6 libgl1
```

Each archive includes the example `config.toml`. Install it before the first run if you want an explicit configuration file:

```bash
mkdir -p ~/.config/rpresent
cp rpresent-vX.Y.Z-x86_64-unknown-linux-gnu/config.toml ~/.config/rpresent/config.toml
```

## Run

Start the daemon with the default configuration path:

```bash
rpresent
```

A missing configuration file uses the built-in defaults. To use another file, pass `--config` before the subcommand:

```bash
rpresent --config /path/to/config.toml
```

Send commands from another shell:

```bash
rpresent spot toggle
rpresent annotation tool arrow
rpresent zoom reset
```

The structured command forms are:

```text
spot <on|off|toggle>
zoom <on|off|toggle|reset>
annotation <on|off|toggle|clear|undo|redo|delete>
annotation tool <select|pen|line|arrow|rect|ellipse|eraser|text>
annotation color <#RRGGBB|next|prev>
annotation width <N|next|prev>
annotation text-size <N|next|prev>
```

`config init` writes a default configuration and does not overwrite an existing file:

```bash
rpresent config init
rpresent config init ~/.config/rpresent/config.toml
```

The command prints the path it wrote to stdout. Logs go to stderr. If `RUST_LOG` is unset, the daemon uses the `info` level.

The `send` subcommand accepts the legacy command strings:

```bash
rpresent send 'spot=toggle'
rpresent send 'annotation=color=#00FF00'
rpresent send 'zoom=reset'
```

## Configuration

By default, `rpresent` reads:

```text
~/.config/rpresent/config.toml
```

The checked-in `config.toml` is a complete example. Configuration uses strict TOML parsing, so removed or unknown keys produce an error.

### Shortcuts

The default global shortcuts are:

```toml
[shortcuts.global]
"spotlight.toggle" = "Ctrl+Alt+1"
"annotation.toggle" = "Ctrl+Alt+2"
"zoom.toggle" = "Ctrl+Alt+3"
```

The default annotation shortcuts are:

```toml
[shortcuts.annotation]
"annotation.undo" = "Ctrl+z"
"annotation.redo" = "Ctrl+Z"
"annotation.delete" = "Delete"
"annotation.clear" = "x"
"annotation.tool.select" = "v"
"annotation.tool.pen" = "p"
"annotation.tool.line" = "l"
"annotation.tool.arrow" = "a"
"annotation.tool.rect" = "r"
"annotation.tool.ellipse" = "o"
"annotation.tool.eraser" = "e"
"annotation.tool.text" = "t"
"annotation.color.next" = "c"
"annotation.color.prev" = "C"
"annotation.width.prev" = "["
"annotation.width.next" = "]"
"annotation.text-size.prev" = "-"
"annotation.text-size.next" = "="
```

The default wheel modifier is `Ctrl`:

```toml
[shortcuts.wheel]
modifiers = "Ctrl"
```

An omitted scope uses its defaults. An explicit scope replaces the whole scope. An empty table disables that scope. Character bindings are case-sensitive, so `Ctrl+z` and `Ctrl+Z` are different bindings. `Meta` is not an application alias for `Super`.

A shortcut that cannot register, including one that conflicts with another grab, is an error. Global bindings take precedence over annotation bindings. Annotation canvas bindings do not intercept text entered into an active text draft.

### Spotlight

The spotlight has a configurable shade, border, center dot, radius, and fade width. Its default values are:

```text
radius_px = 120
shade_opacity = 0.8
softness_px = 20
```

The spotlight shape is configured under `[spotlight.shape]`:

```toml
[spotlight.shape]
type = "circle"
```

Supported shapes are `circle`, `square`, `star`, and `ngon`. A square accepts `corner_radius_pct`. A star accepts `points` and `inner_radius_pct`. An `ngon` accepts `sides`. Wheel resizing changes `radius_px` while the spotlight is active.

### Zoom

Zoom is idle until activated. Activation captures the monitor under the pointer once, then displays that fixed image. Moving the pointer does not recapture the screen.

While zoom owns input:

- hold the configured wheel modifier and scroll to change scale
- hold the left mouse button to pan
- release the button to let the current pan continue with inertia
- run `rpresent zoom reset` to restore the initial scale and center the view

Zoom and spotlight can be active at the same time. Annotation editing has the highest input priority, followed by spotlight, zoom, and the desktop. Activating spotlight or annotation editing cancels the current zoom motion. The next zoom drag needs a new button press.

### Annotation

Annotation editing uses the configured shortcut, `Ctrl+Alt+2` by default. The toolbar appears at the top of the monitor under the pointer while editing.

The default tools are Select, Pen, Line, Arrow, Rect, Ellipse, Eraser, and Text, selected with `v`, `p`, `l`, `a`, `r`, `o`, `e`, and `t`. The initial tool is Arrow. Arrow heads scale with stroke width. Rectangle and ellipse interiors are not selectable or erasable. Hold Shift while dragging a rectangle to draw a square, or while dragging an ellipse to draw a circle. The eraser deletes whole objects, and Select moves one object at a time.

Text editing uses these controls:

- Enter commits the draft.
- Shift+Enter inserts a newline.
- Escape cancels the draft.
- Ctrl+V pastes clipboard text into the draft.

Undo, redo, Delete, and Clear operate on the annotation document. `Ctrl+z` undoes, `Ctrl+Z` redoes, `Delete` removes the current selection, and `x` clears all committed objects. Toolbar actions use the same document history.

Finished artwork remains visible and click-through after editing stops. It is kept only for the current daemon session. Disk persistence, IME input, automatic text wrapping, and partial-stroke erasing are not implemented.

Set a color from the command line by quoting the value so the shell preserves `#`:

```bash
rpresent annotation color '#00FF00'
```

## Logging and troubleshooting

Enable more logging when diagnosing startup or input problems:

```bash
RUST_LOG=debug rpresent
RUST_LOG=trace rpresent spot toggle
```

An overlay can fail to start if X11, the required spotlight extensions, a compositor, or the required GLX/OpenGL support is unavailable. Check that the process is running in an X11 session and that the X11 development or runtime libraries are installed for the operation you are testing.
