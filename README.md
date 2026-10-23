# rpresent

`rpresent` is a Linux/X11 presentation shell for toggling spotlight, annotation, and zoom overlays from the command line or through configured shortcuts.

## Workspace Layout

This repository is a Rust workspace with five crates:

- `crates/rpresent-config` - shared TOML config loading, defaults, and config-file writing
- `crates/rpresent-spotlight` - spotlight state and overlay rendering
- `crates/rpresent-annotation` - drawing tools and stroke history
- `crates/rpresent-zoom` - live capture and magnified rendering
- `crates/rpresent` - CLI, IPC, shortcut routing, and runtime composition

## Requirements

- Rust 2024 toolchain
- Linux with X11 support

The spotlight overlay uses X11 on Linux. It opens one overlay window, tracks the cursor, and redraws the spotlight mask from the current spotlight state. Transparent compositing requires a running compositor on the desktop session. The software render path requires MIT-SHM 1.2 and DAMAGE; the daemon exits with an error when either extension is missing. It writes frames through shared memory and re-renders only the rectangle that the cursor moved across.

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

## Zoom

When the daemon starts without a command it enters an idle loop — no overlay appears until a shortcut or IPC command triggers one.

**Activate zoom mode:**
- Press `Ctrl+3` to toggle zoom mode on/off.
- Or send a command directly: `cargo run -p rpresent -- zoom=on` / `zoom=off`.

**Change zoom level:**
While in zoom mode, hold `Ctrl` and scroll the mouse wheel to increase or decrease the zoom scale.

**Reset zoom:**
```bash
cargo run -p rpresent -- zoom=reset
```
Restores the default scale and re-centers the magnified view.

**Known limitations:**
- Linux/X11 only; Wayland is not supported.
- An ARGB32-capable compositor must be running (e.g., picom, compton, KWin, Mutter) for overlay transparency to render correctly.

## Notes

The top-level command parser accepts these routed commands:

- `spot=...`
- `annotation=...`
- `zoom=...`

Use `--config` with the main CLI to point at a different configuration file when running or sending commands.
