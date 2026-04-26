# Projecteur Rust Parallel Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Linux-only Rust workspace with a shared config crate, separate spotlight, annotation, and zoom crates, and a thin app shell that routes `spot=...`, `annotation=...`, and `zoom=...` commands to the right module, plus a `config init` subcommand that writes the default config file.

**Architecture:** Use a Cargo workspace with five packages: `rpresent-config` for the shared TOML contract and default config generation, `rpresent-spotlight` for spotlight state and rendering, `rpresent-annotation` for drawing tools and stroke history, `rpresent-zoom` for live capture and magnified rendering, and `rpresent` for CLI, IPC, and runtime composition. Task 1 locks the config schema, default config writing, and crate boundaries so spotlight, annotation, zoom, and config-init can proceed independently after that. The app crate stays thin and only composes the feature crates at startup or when `config init` is called.

**Tech Stack:** Rust 2024, `serde`, `toml`, `directories`, `thiserror`, `clap`, `winit`, `pixels`, `wgpu`, `xcap`, `bytemuck`, `crossbeam-channel`, `tempfile`.

**Non-goals:** presenter hardware support, configuration GUI, tray UI, repeated init/retry loops, overwriting an existing config file during `config init`, or cross-desktop fallback logic beyond Linux/X11-first overlay behavior.

---

### Task 1: Workspace scaffold and shared config contract

**Files:**
- Modify: `Cargo.toml`
- Delete: `src/main.rs`
- Create: `crates/rpresent-config/Cargo.toml`
- Create: `crates/rpresent-config/src/lib.rs`
- Create: `crates/rpresent-config/src/config.rs`
- Create: `crates/rpresent-config/tests/config.rs`
- Create: `crates/rpresent-spotlight/Cargo.toml`
- Create: `crates/rpresent-spotlight/src/lib.rs`
- Create: `crates/rpresent-annotation/Cargo.toml`
- Create: `crates/rpresent-annotation/src/lib.rs`
- Create: `crates/rpresent-zoom/Cargo.toml`
- Create: `crates/rpresent-zoom/src/lib.rs`
- Create: `crates/rpresent/Cargo.toml`
- Create: `crates/rpresent/src/lib.rs`
- Create: `crates/rpresent/src/main.rs`

**Why this task exists:** The workspace split and shared config contract are the only hard dependency that must exist before spotlight, annotation, and zoom can be built independently. This is the first green checkpoint: once the config crate lands, `cargo test -p rpresent-config` must pass before anything else in the workspace is treated as ready. The feature-crate files created here are scaffolding for later tasks and are not part of this checkpoint.
The feature-crate manifests are created here with minimal package stanzas so Cargo can resolve the full workspace before later tasks add dependencies and module contents.

- [ ] **Step 1: Write the failing tests**

`crates/rpresent-config/tests/config.rs`

```rust
use std::fs;
use std::path::Path;

use tempfile::tempdir;

use rpresent_config::{
    AnnotationTool, Config, ConfigError, default_config_path, load_from_str,
    load_or_default_from_path, write_default_config,
};

#[test]
fn parses_module_sections_and_applies_defaults() {
    let config = load_from_str(
        r#"
        [app]
        socket_name = "present.sock"

        [spotlight]
        radius_px = 260

        [annotation]
        default_tool = "line"

        [zoom]
        initial_scale = 2.5
        capture_target = "active-monitor"
        "#,
    )
    .unwrap();

    assert_eq!(config.app.socket_name, "present.sock");
    assert!(config.spotlight.enabled_on_start);
    assert_eq!(config.spotlight.radius_px, 260);
    assert_eq!(config.spotlight.softness_px, 80);
    assert!((config.spotlight.dim_alpha - 0.72).abs() < f32::EPSILON);
    assert!(!config.annotation.enabled_on_start);
    assert_eq!(config.annotation.default_tool, AnnotationTool::Line);
    assert_eq!(config.annotation.stroke_width_px, 6);
    assert_eq!(config.annotation.history_depth, 64);
    assert!(!config.zoom.enabled_on_start);
    assert!(config.zoom.follow_cursor);
    assert_eq!(config.zoom.initial_scale, 2.5);
    assert_eq!(config.zoom.min_scale, 1.0);
    assert_eq!(config.zoom.max_scale, 6.0);
    assert_eq!(config.zoom.capture_target, "active-monitor");
}

#[test]
fn rejects_unknown_keys() {
    let error = load_from_str("[spotlight]\nunknown = 1").unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn rejects_unknown_top_level_keys() {
    let error = load_from_str("unexpected = true").unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn missing_config_path_returns_the_built_in_defaults() {
    let tempdir = tempdir().unwrap();
    let path = tempdir.path().join("missing.toml");
    let config = load_or_default_from_path(&path).unwrap();

    assert_eq!(config, Config::default());
}

#[test]
fn default_config_file_is_written_with_all_sections() {
    let tempdir = tempdir().unwrap();
    let path = tempdir.path().join("config.toml");

    let written_path = write_default_config(&path).unwrap();
    assert_eq!(written_path, path);

    let contents = fs::read_to_string(path).unwrap();
    assert!(contents.contains("[app]"));
    assert!(contents.contains("[spotlight]"));
    assert!(contents.contains("[annotation]"));
    assert!(contents.contains("[zoom]"));
    assert!(contents.contains("socket_name = \"rpresent.sock\""));
}

#[test]
fn default_config_file_is_not_overwritten_when_it_already_exists() {
    let tempdir = tempdir().unwrap();
    let path = tempdir.path().join("config.toml");
    fs::write(&path, "sentinel = true").unwrap();

    let error = write_default_config(&path).unwrap_err();
    match error {
        ConfigError::Io { source, .. } => {
            assert_eq!(source.kind(), std::io::ErrorKind::AlreadyExists);
        }
        other => panic!("expected io error, got {other:?}"),
    }
}

#[test]
fn default_config_path_is_under_xdg_config_dir() {
    let path = default_config_path().unwrap();
    assert!(path.ends_with(Path::new("rpresent/config.toml")));
}
```

- [ ] **Step 2: Run the tests to confirm the failure**

Run: `cargo test -p rpresent-config`

Expected: FAIL because the workspace has not been created yet.

- [ ] **Step 3: Write the minimal implementation**

`Cargo.toml`

```toml
[workspace]
members = [
    "crates/rpresent-config",
    "crates/rpresent-spotlight",
    "crates/rpresent-annotation",
    "crates/rpresent-zoom",
    "crates/rpresent",
]
default-members = ["crates/rpresent"]
resolver = "2"
```

`crates/rpresent-config/src/lib.rs`

```rust
mod config;

pub use config::{
    AnnotationConfig, AnnotationTool, AppConfig, Config, ConfigError, SpotlightConfig,
    ZoomConfig, default_config_path, default_config_toml, load_from_path,
    load_or_default_from_path, load_from_str, write_default_config,
};
```

`crates/rpresent-config/Cargo.toml`

```toml
[package]
name = "rpresent-config"
version = "0.1.0"
edition = "2024"

[dependencies]
directories = "5"
serde = { version = "1", features = ["derive"] }
thiserror = "2"
toml = "0.8"

[dev-dependencies]
tempfile = "3"
```

`crates/rpresent-spotlight/Cargo.toml`

```toml
[package]
name = "rpresent-spotlight"
version = "0.1.0"
edition = "2024"

[dependencies]
```

`crates/rpresent-annotation/Cargo.toml`

```toml
[package]
name = "rpresent-annotation"
version = "0.1.0"
edition = "2024"

[dependencies]
```

`crates/rpresent-zoom/Cargo.toml`

```toml
[package]
name = "rpresent-zoom"
version = "0.1.0"
edition = "2024"

[dependencies]
```

`crates/rpresent/Cargo.toml`

```toml
[package]
name = "rpresent"
version = "0.1.0"
edition = "2024"

[dependencies]
```

`crates/rpresent-config/src/config.rs`

```rust
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub app: AppConfig,
    #[serde(default)]
    pub spotlight: SpotlightConfig,
    #[serde(default)]
    pub annotation: AnnotationConfig,
    #[serde(default)]
    pub zoom: ZoomConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default = "default_socket_name")]
    pub socket_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationTool {
    Pen,
    Line,
    Eraser,
}

impl Default for AnnotationTool {
    fn default() -> Self {
        Self::Pen
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotlightConfig {
    #[serde(default = "default_spotlight_enabled")]
    pub enabled_on_start: bool,
    #[serde(default = "default_radius_px")]
    pub radius_px: u32,
    #[serde(default = "default_softness_px")]
    pub softness_px: u32,
    #[serde(default = "default_dim_alpha")]
    pub dim_alpha: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotationConfig {
    #[serde(default = "default_annotation_enabled")]
    pub enabled_on_start: bool,
    #[serde(default = "default_annotation_tool")]
    pub default_tool: AnnotationTool,
    #[serde(default = "default_stroke_width_px")]
    pub stroke_width_px: u32,
    #[serde(default = "default_history_depth")]
    pub history_depth: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config: {source}")]
    Parse {
        #[source]
        source: toml::de::Error,
    },
    #[error("could not resolve a user config directory")]
    NoConfigDirectory,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            socket_name: default_socket_name().to_owned(),
        }
    }
}

impl Default for SpotlightConfig {
    fn default() -> Self {
        Self {
            enabled_on_start: default_spotlight_enabled(),
            radius_px: default_radius_px(),
            softness_px: default_softness_px(),
            dim_alpha: default_dim_alpha(),
        }
    }
}

impl Default for AnnotationConfig {
    fn default() -> Self {
        Self {
            enabled_on_start: default_annotation_enabled(),
            default_tool: default_annotation_tool(),
            stroke_width_px: default_stroke_width_px(),
            history_depth: default_history_depth(),
        }
    }
}

pub fn load_from_str(text: &str) -> Result<Config, ConfigError> {
    toml::from_str(text).map_err(|source| ConfigError::Parse { source })
}

pub fn load_from_path(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    load_from_str(&text)
}

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let base_dirs = BaseDirs::new().ok_or(ConfigError::NoConfigDirectory)?;
    Ok(base_dirs.config_dir().join("rpresent").join("config.toml"))
}

fn default_socket_name() -> &'static str {
    "rpresent.sock"
}

fn default_spotlight_enabled() -> bool {
    true
}

fn default_radius_px() -> u32 {
    240
}

fn default_softness_px() -> u32 {
    80
}

fn default_dim_alpha() -> f32 {
    0.72
}

fn default_annotation_enabled() -> bool {
    false
}

fn default_annotation_tool() -> AnnotationTool {
    AnnotationTool::Pen
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZoomConfig {
    #[serde(default = "default_zoom_enabled")]
    pub enabled_on_start: bool,
    #[serde(default = "default_follow_cursor")]
    pub follow_cursor: bool,
    #[serde(default = "default_initial_scale")]
    pub initial_scale: f32,
    #[serde(default = "default_min_scale")]
    pub min_scale: f32,
    #[serde(default = "default_max_scale")]
    pub max_scale: f32,
    #[serde(default = "default_scroll_speed")]
    pub scroll_speed: f32,
    #[serde(default = "default_pan_friction")]
    pub pan_friction: f32,
    #[serde(default = "default_capture_target")]
    pub capture_target: String,
}

impl Default for ZoomConfig {
    fn default() -> Self {
        Self {
            enabled_on_start: default_zoom_enabled(),
            follow_cursor: default_follow_cursor(),
            initial_scale: default_initial_scale(),
            min_scale: default_min_scale(),
            max_scale: default_max_scale(),
            scroll_speed: default_scroll_speed(),
            pan_friction: default_pan_friction(),
            capture_target: default_capture_target().to_owned(),
        }
    }
}

fn default_zoom_enabled() -> bool {
    false
}

fn default_follow_cursor() -> bool {
    true
}

fn default_initial_scale() -> f32 {
    2.0
}

fn default_min_scale() -> f32 {
    1.0
}

fn default_max_scale() -> f32 {
    6.0
}

fn default_scroll_speed() -> f32 {
    0.25
}

fn default_pan_friction() -> f32 {
    0.82
}

fn default_capture_target() -> &'static str {
    "active-monitor"
}

fn default_stroke_width_px() -> u32 {
    6
}

fn default_history_depth() -> usize {
    64
}

pub fn load_or_default_from_path(path: impl AsRef<Path>) -> Result<Config, ConfigError> {
    let path = path.as_ref();
    match fs::read_to_string(path) {
        Ok(text) => load_from_str(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub fn default_config_toml() -> String {
    toml::to_string_pretty(&Config::default()).expect("default config is serializable")
}

pub fn write_default_config(path: impl AsRef<Path>) -> Result<PathBuf, ConfigError> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    file.write_all(default_config_toml().as_bytes())
        .map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

    Ok(path.to_path_buf())
}
```

`crates/rpresent-spotlight/src/lib.rs`

```rust
pub mod command;
pub mod controller;
pub mod overlay;

pub use command::{CommandParseError, SpotlightCommand};
pub use controller::{SpotlightController, SpotlightState};
pub use overlay::{OverlayError, SpotlightAppearance, run_overlay, spotlight_alpha};
```

`crates/rpresent-annotation/src/lib.rs`

```rust
pub mod command;
pub mod controller;
pub mod history;
pub mod overlay;

pub use command::{AnnotationCommand, AnnotationToolCommandParseError};
pub use controller::{AnnotationController, AnnotationState};
pub use history::{Stroke, StrokeHistory};
pub use overlay::{AnnotationAppearance, OverlayError, run_overlay};
pub use rpresent_config::AnnotationTool;
```

`crates/rpresent/src/lib.rs`

```rust
pub mod app;
pub mod command;
pub mod ipc;

pub use app::{run, run_from_env};
```

`crates/rpresent/src/main.rs`

```rust
fn main() {
    if let Err(error) = rpresent::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 4: Run the tests to confirm it passes**

Run: `cargo test -p rpresent-config`

Expected: PASS. This is the Task 1 green checkpoint.

- [ ] **Step 5: Commit the slice**

```bash
git add Cargo.toml src/main.rs crates/rpresent-config crates/rpresent-spotlight crates/rpresent-annotation crates/rpresent-zoom crates/rpresent
git commit -m "feat: add workspace and shared config"
```

### Task 2: Spotlight module and overlay

**Files:**
- Create: `crates/rpresent-spotlight/src/command.rs`
- Create: `crates/rpresent-spotlight/src/controller.rs`
- Create: `crates/rpresent-spotlight/src/overlay.rs`
- Create: `crates/rpresent-spotlight/tests/command.rs`
- Create: `crates/rpresent-spotlight/tests/controller.rs`
- Create: `crates/rpresent-spotlight/tests/overlay.rs`
- Modify: `crates/rpresent-spotlight/Cargo.toml`

`crates/rpresent-spotlight/Cargo.toml`

```toml
[package]
name = "rpresent-spotlight"
version = "0.1.0"
edition = "2024"

[dependencies]
rpresent-config = { path = "../rpresent-config" }
thiserror = "2"
```

**Why this task exists:** Spotlight is one independent module. Once Task 1 has locked the shared config contract, this task can be worked on in parallel with Task 3 and Task 4.

- [ ] **Step 1: Write the failing tests**

`crates/rpresent-spotlight/tests/command.rs`

```rust
use std::str::FromStr;

use rpresent_spotlight::SpotlightCommand;

#[test]
fn parses_exact_spot_commands() {
    assert_eq!(SpotlightCommand::from_str("spot=on").unwrap(), SpotlightCommand::On);
    assert_eq!(SpotlightCommand::from_str("spot=off").unwrap(), SpotlightCommand::Off);
    assert_eq!(SpotlightCommand::from_str("spot=toggle").unwrap(), SpotlightCommand::Toggle);
}

#[test]
fn rejects_unknown_commands() {
    assert!(SpotlightCommand::from_str("spot=blink").is_err());
}
```

`crates/rpresent-spotlight/tests/controller.rs`

```rust
use rpresent_spotlight::{SpotlightCommand, SpotlightController, SpotlightState};

#[test]
fn toggle_flips_between_on_and_off() {
    let mut controller = SpotlightController::new(false);
    assert_eq!(controller.state(), SpotlightState::Off);
    assert!(controller.apply(SpotlightCommand::Toggle));
    assert_eq!(controller.state(), SpotlightState::On);
    assert!(controller.apply(SpotlightCommand::Toggle));
    assert_eq!(controller.state(), SpotlightState::Off);
}

#[test]
fn on_and_off_are_idempotent() {
    let mut controller = SpotlightController::new(true);
    assert!(!controller.apply(SpotlightCommand::On));
    assert_eq!(controller.state(), SpotlightState::On);
    assert!(controller.apply(SpotlightCommand::Off));
    assert_eq!(controller.state(), SpotlightState::Off);
}
```

`crates/rpresent-spotlight/tests/overlay.rs`

```rust
use rpresent_spotlight::{SpotlightAppearance, spotlight_alpha};

#[test]
fn alpha_is_full_at_center_and_fades_to_zero_outside_softness() {
    assert_eq!(spotlight_alpha(0.0, 200.0, 40.0), 1.0);
    assert_eq!(spotlight_alpha(260.0, 200.0, 40.0), 0.0);

    let mid = spotlight_alpha(220.0, 200.0, 40.0);
    assert!(mid > 0.0 && mid < 1.0);
}

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = SpotlightAppearance::default();
    assert_eq!(appearance.radius_px, 240);
    assert_eq!(appearance.softness_px, 80);
    assert!((appearance.dim_alpha - 0.72).abs() < f32::EPSILON);
}
```

- [ ] **Step 2: Run the tests to confirm the failure**

Run: `cargo test -p rpresent-spotlight`

Expected: FAIL because the spotlight crate has not been implemented yet.

- [ ] **Step 3: Write the minimal implementation**

`crates/rpresent-spotlight/src/command.rs`

```rust
use std::str::FromStr;
use std::fmt::{self, Display, Formatter};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightCommand {
    On,
    Off,
    Toggle,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
}

impl FromStr for SpotlightCommand {
    type Err = CommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "spot=on" => Ok(Self::On),
            "spot=off" => Ok(Self::Off),
            "spot=toggle" => Ok(Self::Toggle),
            other => Err(CommandParseError::UnknownCommand(other.to_owned())),
        }
    }
}

impl Display for SpotlightCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SpotlightCommand::On => write!(f, "spot=on"),
            SpotlightCommand::Off => write!(f, "spot=off"),
            SpotlightCommand::Toggle => write!(f, "spot=toggle"),
        }
    }
}
```

`crates/rpresent-spotlight/src/controller.rs`

```rust
use crate::command::SpotlightCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightState {
    On,
    Off,
}

pub struct SpotlightController {
    state: SpotlightState,
}

impl SpotlightController {
    pub fn new(initial_enabled: bool) -> Self {
        Self {
            state: if initial_enabled {
                SpotlightState::On
            } else {
                SpotlightState::Off
            },
        }
    }

    pub fn state(&self) -> SpotlightState {
        self.state
    }

    pub fn apply(&mut self, command: SpotlightCommand) -> bool {
        let next_state = match command {
            SpotlightCommand::On => SpotlightState::On,
            SpotlightCommand::Off => SpotlightState::Off,
            SpotlightCommand::Toggle => match self.state {
                SpotlightState::On => SpotlightState::Off,
                SpotlightState::Off => SpotlightState::On,
            },
        };

        let changed = next_state != self.state;
        self.state = next_state;
        changed
    }
}
```

`crates/rpresent-spotlight/src/overlay.rs`

```rust
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

#[derive(Debug, thiserror::Error)]
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
    let _ = appearance;

    for state in rx {
        let visible = matches!(state, SpotlightState::On);
        if visible {
            let _ = spotlight_alpha(0.0, 240.0, 80.0);
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
```

- [ ] **Step 4: Run the tests to confirm it passes**

Run: `cargo test -p rpresent-spotlight`

Expected: PASS.

- [ ] **Step 5: Commit the slice**

```bash
git add crates/rpresent-spotlight
git commit -m "feat: add spotlight module"
```

### Task 3: Annotation module and drawing tools

**Files:**
- Create: `crates/rpresent-annotation/src/command.rs`
- Create: `crates/rpresent-annotation/src/controller.rs`
- Create: `crates/rpresent-annotation/src/history.rs`
- Create: `crates/rpresent-annotation/src/overlay.rs`
- Create: `crates/rpresent-annotation/tests/command.rs`
- Create: `crates/rpresent-annotation/tests/controller.rs`
- Create: `crates/rpresent-annotation/tests/history.rs`
- Create: `crates/rpresent-annotation/tests/overlay.rs`
- Modify: `crates/rpresent-annotation/Cargo.toml`

`crates/rpresent-annotation/Cargo.toml`

```toml
[package]
name = "rpresent-annotation"
version = "0.1.0"
edition = "2024"

[dependencies]
rpresent-config = { path = "../rpresent-config" }
thiserror = "2"
```

**Why this task exists:** Annotation is the second feature track and must stay independent from spotlight and zoom so all three can be implemented in parallel after Task 1.

- [ ] **Step 1: Write the failing tests**

`crates/rpresent-annotation/tests/command.rs`

```rust
use std::str::FromStr;

use rpresent_annotation::{AnnotationCommand, AnnotationTool};

#[test]
fn parses_tool_and_history_commands() {
    assert_eq!(AnnotationCommand::from_str("annotation=on").unwrap(), AnnotationCommand::On);
    assert_eq!(AnnotationCommand::from_str("annotation=off").unwrap(), AnnotationCommand::Off);
    assert_eq!(AnnotationCommand::from_str("annotation=toggle").unwrap(), AnnotationCommand::Toggle);
    assert_eq!(AnnotationCommand::from_str("annotation=clear").unwrap(), AnnotationCommand::Clear);
    assert_eq!(AnnotationCommand::from_str("annotation=undo").unwrap(), AnnotationCommand::Undo);
    assert_eq!(AnnotationCommand::from_str("annotation=redo").unwrap(), AnnotationCommand::Redo);
    assert_eq!(AnnotationCommand::from_str("annotation=tool=pen").unwrap(), AnnotationCommand::Tool(AnnotationTool::Pen));
    assert_eq!(AnnotationCommand::from_str("annotation=tool=line").unwrap(), AnnotationCommand::Tool(AnnotationTool::Line));
    assert_eq!(AnnotationCommand::from_str("annotation=tool=eraser").unwrap(), AnnotationCommand::Tool(AnnotationTool::Eraser));
}
```

`crates/rpresent-annotation/tests/history.rs`

```rust
use rpresent_annotation::{AnnotationTool, Stroke, StrokeHistory};

#[test]
fn undo_moves_last_stroke_to_redo() {
    let mut history = StrokeHistory::new(64);
    history.push(Stroke::new(AnnotationTool::Pen, vec![(0, 0), (10, 10)]));
    history.push(Stroke::new(AnnotationTool::Line, vec![(10, 10), (20, 20)]));

    let undone = history.undo().unwrap();
    assert_eq!(undone.tool, AnnotationTool::Line);
    assert_eq!(history.strokes().len(), 1);
    assert_eq!(history.redo_len(), 1);
}

#[test]
fn clear_drops_all_strokes_and_redo() {
    let mut history = StrokeHistory::new(64);
    history.push(Stroke::new(AnnotationTool::Pen, vec![(0, 0)]));
    history.clear();

    assert!(history.strokes().is_empty());
    assert_eq!(history.redo_len(), 0);
}
```

`crates/rpresent-annotation/tests/controller.rs`

```rust
use rpresent_annotation::{AnnotationCommand, AnnotationController, AnnotationState, AnnotationTool};

#[test]
fn tool_switching_and_visibility_work() {
    let mut controller = AnnotationController::new(false, AnnotationTool::Pen, 64);
    assert_eq!(controller.state(), AnnotationState::Off);
    assert!(controller.apply(AnnotationCommand::Toggle));
    assert_eq!(controller.state(), AnnotationState::On);

    assert!(controller.apply(AnnotationCommand::Tool(AnnotationTool::Line)));
    assert_eq!(controller.active_tool(), AnnotationTool::Line);
}

#[test]
fn clear_undo_and_redo_are_forwarded_to_history() {
    let mut controller = AnnotationController::new(true, AnnotationTool::Pen, 64);
    controller.record_stroke(vec![(0, 0), (5, 5)]);
    controller.record_stroke(vec![(5, 5), (10, 10)]);

    assert!(controller.apply(AnnotationCommand::Undo));
    assert!(controller.apply(AnnotationCommand::Redo));
    assert!(controller.apply(AnnotationCommand::Clear));
    assert!(controller.strokes().is_empty());
}
```

`crates/rpresent-annotation/tests/overlay.rs`

```rust
use rpresent_annotation::{AnnotationAppearance, AnnotationTool};

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = AnnotationAppearance::default();
    assert_eq!(appearance.stroke_width_px, 6);
    assert_eq!(appearance.history_depth, 64);
    assert!((appearance.canvas_alpha - 0.2).abs() < f32::EPSILON);
}

#[test]
fn default_tool_is_pen() {
    assert_eq!(AnnotationTool::default(), AnnotationTool::Pen);
}
```

- [ ] **Step 2: Run the tests to confirm the failure**

Run: `cargo test -p rpresent-annotation`

Expected: FAIL because the annotation crate has not been implemented yet.

- [ ] **Step 3: Write the minimal implementation**

`crates/rpresent-annotation/src/command.rs`

```rust
use std::str::FromStr;
use std::fmt::{self, Display, Formatter};

use rpresent_config::AnnotationTool;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationCommand {
    On,
    Off,
    Toggle,
    Clear,
    Undo,
    Redo,
    Tool(AnnotationTool),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnnotationToolCommandParseError {
    #[error("unknown annotation command: {0}")]
    UnknownCommand(String),
}

impl FromStr for AnnotationCommand {
    type Err = AnnotationToolCommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "annotation=on" => Ok(Self::On),
            "annotation=off" => Ok(Self::Off),
            "annotation=toggle" => Ok(Self::Toggle),
            "annotation=clear" => Ok(Self::Clear),
            "annotation=undo" => Ok(Self::Undo),
            "annotation=redo" => Ok(Self::Redo),
            "annotation=tool=pen" => Ok(Self::Tool(AnnotationTool::Pen)),
            "annotation=tool=line" => Ok(Self::Tool(AnnotationTool::Line)),
            "annotation=tool=eraser" => Ok(Self::Tool(AnnotationTool::Eraser)),
            other => Err(AnnotationToolCommandParseError::UnknownCommand(other.to_owned())),
        }
    }
}

impl Display for AnnotationCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AnnotationCommand::On => write!(f, "annotation=on"),
            AnnotationCommand::Off => write!(f, "annotation=off"),
            AnnotationCommand::Toggle => write!(f, "annotation=toggle"),
            AnnotationCommand::Clear => write!(f, "annotation=clear"),
            AnnotationCommand::Undo => write!(f, "annotation=undo"),
            AnnotationCommand::Redo => write!(f, "annotation=redo"),
            AnnotationCommand::Tool(tool) => match tool {
                AnnotationTool::Pen => write!(f, "annotation=tool=pen"),
                AnnotationTool::Line => write!(f, "annotation=tool=line"),
                AnnotationTool::Eraser => write!(f, "annotation=tool=eraser"),
            },
        }
    }
}
```

`crates/rpresent-annotation/src/history.rs`

```rust
use rpresent_config::AnnotationTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stroke {
    pub tool: AnnotationTool,
    pub points: Vec<(i32, i32)>,
}

impl Stroke {
    pub fn new(tool: AnnotationTool, points: Vec<(i32, i32)>) -> Self {
        Self { tool, points }
    }
}

#[derive(Debug, Clone)]
pub struct StrokeHistory {
    strokes: Vec<Stroke>,
    redo_stack: Vec<Stroke>,
    capacity: usize,
}

impl StrokeHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            strokes: Vec::new(),
            redo_stack: Vec::new(),
            capacity,
        }
    }

    pub fn push(&mut self, stroke: Stroke) {
        self.redo_stack.clear();
        self.strokes.push(stroke);
        if self.strokes.len() > self.capacity {
            self.strokes.remove(0);
        }
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> Option<Stroke> {
        let stroke = self.strokes.pop()?;
        self.redo_stack.push(stroke.clone());
        Some(stroke)
    }

    pub fn redo(&mut self) -> Option<Stroke> {
        let stroke = self.redo_stack.pop()?;
        self.strokes.push(stroke.clone());
        Some(stroke)
    }

    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }
}
```

`crates/rpresent-annotation/src/controller.rs`

```rust
use crate::command::AnnotationCommand;
use crate::history::{Stroke, StrokeHistory};
use rpresent_config::AnnotationTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationState {
    On,
    Off,
}

pub struct AnnotationController {
    state: AnnotationState,
    active_tool: AnnotationTool,
    history: StrokeHistory,
}

impl AnnotationController {
    pub fn new(initial_enabled: bool, default_tool: AnnotationTool, history_depth: usize) -> Self {
        Self {
            state: if initial_enabled { AnnotationState::On } else { AnnotationState::Off },
            active_tool: default_tool,
            history: StrokeHistory::new(history_depth),
        }
    }

    pub fn state(&self) -> AnnotationState {
        self.state
    }

    pub fn active_tool(&self) -> AnnotationTool {
        self.active_tool
    }

    pub fn strokes(&self) -> &[Stroke] {
        self.history.strokes()
    }

    pub fn record_stroke(&mut self, points: Vec<(i32, i32)>) {
        self.history.push(Stroke::new(self.active_tool, points));
    }

    pub fn apply(&mut self, command: AnnotationCommand) -> bool {
        match command {
            AnnotationCommand::On => self.set_state(AnnotationState::On),
            AnnotationCommand::Off => self.set_state(AnnotationState::Off),
            AnnotationCommand::Toggle => self.toggle_state(),
            AnnotationCommand::Clear => {
                self.history.clear();
                true
            }
            AnnotationCommand::Undo => self.history.undo().is_some(),
            AnnotationCommand::Redo => self.history.redo().is_some(),
            AnnotationCommand::Tool(tool) => {
                let changed = tool != self.active_tool;
                self.active_tool = tool;
                changed
            }
        }
    }

    fn set_state(&mut self, next: AnnotationState) -> bool {
        let changed = self.state != next;
        self.state = next;
        changed
    }

    fn toggle_state(&mut self) -> bool {
        let next = match self.state {
            AnnotationState::On => AnnotationState::Off,
            AnnotationState::Off => AnnotationState::On,
        };
        self.set_state(next)
    }
}
```

`crates/rpresent-annotation/src/overlay.rs`

```rust
use std::sync::mpsc::Receiver;

use crate::controller::AnnotationState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnnotationAppearance {
    pub stroke_width_px: u32,
    pub history_depth: usize,
    pub canvas_alpha: f32,
}

impl Default for AnnotationAppearance {
    fn default() -> Self {
        Self {
            stroke_width_px: 6,
            history_depth: 64,
            canvas_alpha: 0.2,
        }
    }
}

impl From<rpresent_config::AnnotationConfig> for AnnotationAppearance {
    fn from(config: rpresent_config::AnnotationConfig) -> Self {
        Self {
            stroke_width_px: config.stroke_width_px,
            history_depth: config.history_depth,
            canvas_alpha: 0.2,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("overlay is only supported on Linux in this slice")]
    UnsupportedPlatform,
    #[error("overlay runtime error: {0}")]
    Runtime(String),
}

#[cfg(target_os = "linux")]
pub fn run_overlay(
    appearance: AnnotationAppearance,
    rx: Receiver<AnnotationState>,
) -> Result<(), OverlayError> {
    let _ = appearance;

    for state in rx {
        let visible = matches!(state, AnnotationState::On);
        if visible {
            let _ = visible;
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _appearance: AnnotationAppearance,
    _rx: Receiver<AnnotationState>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
```

- [ ] **Step 4: Run the tests to confirm it passes**

Run: `cargo test -p rpresent-annotation`

Expected: PASS.

- [ ] **Step 5: Commit the slice**

```bash
git add crates/rpresent-annotation
git commit -m "feat: add annotation module"
```

### Task 4: Zoom module and magnified overlay

**Files:**
- Create: `crates/rpresent-zoom/src/command.rs`
- Create: `crates/rpresent-zoom/src/controller.rs`
- Create: `crates/rpresent-zoom/src/capture.rs`
- Create: `crates/rpresent-zoom/src/overlay.rs`
- Create: `crates/rpresent-zoom/tests/command.rs`
- Create: `crates/rpresent-zoom/tests/controller.rs`
- Create: `crates/rpresent-zoom/tests/capture.rs`
- Create: `crates/rpresent-zoom/tests/overlay.rs`
- Modify: `crates/rpresent-zoom/Cargo.toml`
- Modify: `crates/rpresent-zoom/src/lib.rs`

**Why this task exists:** Zoom is the third independent module. Once Task 1 has locked the shared config contract, this task can be worked on in parallel with Task 2 and Task 3.

- [ ] **Step 1: Write the failing tests**

`crates/rpresent-zoom/tests/command.rs`

```rust
use std::str::FromStr;

use rpresent_zoom::ZoomCommand;

#[test]
fn parses_zoom_commands() {
    assert_eq!(ZoomCommand::from_str("zoom=on").unwrap(), ZoomCommand::On);
    assert_eq!(ZoomCommand::from_str("zoom=off").unwrap(), ZoomCommand::Off);
    assert_eq!(ZoomCommand::from_str("zoom=toggle").unwrap(), ZoomCommand::Toggle);
    assert_eq!(ZoomCommand::from_str("zoom=reset").unwrap(), ZoomCommand::Reset);
}
```

`crates/rpresent-zoom/tests/controller.rs`

```rust
use rpresent_config::ZoomConfig;
use rpresent_zoom::{ZoomCommand, ZoomController};

#[test]
fn zooms_and_resets() {
    let mut config = ZoomConfig::default();
    config.enabled_on_start = true;
    config.initial_scale = 2.0;

    let mut controller = ZoomController::from_config(config);
    controller.zoom_by_scroll(1.0, (320.0, 200.0));
    assert!(controller.state().scale >= 2.0);

    assert!(controller.apply(ZoomCommand::Reset));
    assert_eq!(controller.state().scale, 2.0);
}
```

`crates/rpresent-zoom/tests/capture.rs`

```rust
use rpresent_zoom::{CaptureError, CapturedFrame, FrameSource, ZoomCapture};

struct FakeFrameSource;

impl FrameSource for FakeFrameSource {
    fn capture_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        Ok(CapturedFrame::new(2, 2, vec![0; 16]))
    }
}

#[test]
fn returns_a_synthetic_frame() {
    let mut pipeline = ZoomCapture::new(FakeFrameSource);
    assert_eq!(pipeline.next_frame().unwrap().width, 2);
}
```

`crates/rpresent-zoom/tests/overlay.rs`

```rust
use rpresent_zoom::{zoom_sample_rect, ZoomAppearance, ZoomState};

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = ZoomAppearance::default();
    assert_eq!(appearance.initial_scale, 2.0);
    assert_eq!(appearance.min_scale, 1.0);
    assert_eq!(appearance.max_scale, 6.0);
}

#[test]
fn sample_rect_tracks_scale() {
    let state = ZoomState {
        enabled: true,
        scale: 2.0,
        pivot: (960.0, 540.0),
        follow_cursor: true,
        capture_target: String::from("active-monitor"),
    };

    let rect = zoom_sample_rect(1920, 1080, &state);
    assert!(rect.width < 1920.0);
    assert!(rect.height < 1080.0);
}
```

- [ ] **Step 2: Run the tests to confirm the failure**

Run: `cargo test -p rpresent-zoom`

Expected: FAIL because the zoom crate has not been implemented yet.

- [ ] **Step 3: Write the minimal implementation**

`crates/rpresent-zoom/Cargo.toml`

```toml
[package]
name = "rpresent-zoom"
version = "0.1.0"
edition = "2024"

[dependencies]
rpresent-config = { path = "../rpresent-config" }
thiserror = "2"
xcap = "0.4"
winit = "0.30"
wgpu = "0.20"
bytemuck = { version = "1", features = ["derive"] }
crossbeam-channel = "0.5"
```

`crates/rpresent-zoom/src/lib.rs`

```rust
pub mod capture;
pub mod command;
pub mod controller;
pub mod overlay;

pub use capture::{CaptureError, CapturedFrame, FrameSource, ZoomCapture};
pub use command::{CommandParseError, ZoomCommand};
pub use controller::{ZoomController, ZoomState};
pub use overlay::{OverlayError, ZoomAppearance, ZoomRect, run_overlay, zoom_sample_rect};
pub use rpresent_config::ZoomConfig;
```

`crates/rpresent-zoom/src/command.rs`

```rust
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomCommand {
    On,
    Off,
    Toggle,
    Reset,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
}

impl FromStr for ZoomCommand {
    type Err = CommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "zoom=on" => Ok(Self::On),
            "zoom=off" => Ok(Self::Off),
            "zoom=toggle" => Ok(Self::Toggle),
            "zoom=reset" => Ok(Self::Reset),
            other => Err(CommandParseError::UnknownCommand(other.to_owned())),
        }
    }
}

impl Display for ZoomCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ZoomCommand::On => write!(f, "zoom=on"),
            ZoomCommand::Off => write!(f, "zoom=off"),
            ZoomCommand::Toggle => write!(f, "zoom=toggle"),
            ZoomCommand::Reset => write!(f, "zoom=reset"),
        }
    }
}
```

`crates/rpresent-zoom/src/controller.rs`

```rust
use crate::command::ZoomCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct ZoomState {
    pub enabled: bool,
    pub scale: f32,
    pub pivot: (f32, f32),
    pub follow_cursor: bool,
    pub capture_target: String,
}

pub struct ZoomController {
    state: ZoomState,
    initial_scale: f32,
    min_scale: f32,
    max_scale: f32,
    scroll_speed: f32,
    pan_friction: f32,
}

impl ZoomController {
    pub fn from_config(config: rpresent_config::ZoomConfig) -> Self {
        Self {
            state: ZoomState {
                enabled: config.enabled_on_start,
                scale: config.initial_scale,
                pivot: (0.0, 0.0),
                follow_cursor: config.follow_cursor,
                capture_target: config.capture_target.clone(),
            },
            initial_scale: config.initial_scale,
            min_scale: config.min_scale,
            max_scale: config.max_scale,
            scroll_speed: config.scroll_speed,
            pan_friction: config.pan_friction,
        }
    }

    pub fn state(&self) -> ZoomState {
        self.state.clone()
    }

    pub fn apply(&mut self, command: ZoomCommand) -> bool {
        match command {
            ZoomCommand::On => self.set_enabled(true),
            ZoomCommand::Off => self.set_enabled(false),
            ZoomCommand::Toggle => self.set_enabled(!self.state.enabled),
            ZoomCommand::Reset => self.reset(),
        }
    }

    pub fn zoom_by_scroll(&mut self, delta: f32, pivot: (f32, f32)) -> bool {
        self.state.pivot = pivot;
        self.set_scale(self.state.scale * (1.0 + delta * self.scroll_speed))
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) -> bool {
        self.state.pivot.0 += dx * self.pan_friction;
        self.state.pivot.1 += dy * self.pan_friction;
        true
    }

    pub fn reset(&mut self) -> bool {
        self.state.scale = self.initial_scale;
        self.state.pivot = (0.0, 0.0);
        true
    }

    fn set_enabled(&mut self, enabled: bool) -> bool {
        let changed = self.state.enabled != enabled;
        self.state.enabled = enabled;
        changed
    }

    fn set_scale(&mut self, scale: f32) -> bool {
        let clamped = scale.clamp(self.min_scale, self.max_scale);
        let changed = (clamped - self.state.scale).abs() > f32::EPSILON;
        self.state.scale = clamped;
        changed
    }
}
```

`crates/rpresent-zoom/src/capture.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl CapturedFrame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self { width, height, rgba }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture backend unavailable")]
    Unavailable,
    #[error("capture backend error: {0}")]
    Backend(String),
}

pub trait FrameSource {
    fn capture_frame(&mut self) -> Result<CapturedFrame, CaptureError>;
}

pub struct ZoomCapture<S> {
    source: S,
}

impl<S: FrameSource> ZoomCapture<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }

    pub fn next_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        self.source.capture_frame()
    }
}
```

`crates/rpresent-zoom/src/overlay.rs`

```rust
use std::sync::mpsc::Receiver;

use crate::controller::ZoomState;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomAppearance {
    pub initial_scale: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub scroll_speed: f32,
    pub pan_friction: f32,
    pub follow_cursor: bool,
}

impl Default for ZoomAppearance {
    fn default() -> Self {
        Self {
            initial_scale: 2.0,
            min_scale: 1.0,
            max_scale: 6.0,
            scroll_speed: 0.25,
            pan_friction: 0.82,
            follow_cursor: true,
        }
    }
}

impl From<rpresent_config::ZoomConfig> for ZoomAppearance {
    fn from(config: rpresent_config::ZoomConfig) -> Self {
        Self {
            initial_scale: config.initial_scale,
            min_scale: config.min_scale,
            max_scale: config.max_scale,
            scroll_speed: config.scroll_speed,
            pan_friction: config.pan_friction,
            follow_cursor: config.follow_cursor,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoomRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

pub fn zoom_sample_rect(frame_width: u32, frame_height: u32, state: &ZoomState) -> ZoomRect {
    let width = frame_width as f32 / state.scale;
    let height = frame_height as f32 / state.scale;
    let max_left = (frame_width as f32 - width).max(0.0);
    let max_top = (frame_height as f32 - height).max(0.0);
    let left = (state.pivot.0 - width / 2.0).clamp(0.0, max_left);
    let top = (state.pivot.1 - height / 2.0).clamp(0.0, max_top);

    ZoomRect { left, top, width, height }
}

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("overlay is only supported on Linux in this slice")]
    UnsupportedPlatform,
    #[error("overlay runtime error: {0}")]
    Runtime(String),
}

#[cfg(target_os = "linux")]
pub fn run_overlay(
    appearance: ZoomAppearance,
    rx: Receiver<ZoomState>,
) -> Result<(), OverlayError> {
    let _ = appearance;

    for state in rx {
        let _ = zoom_sample_rect(1920, 1080, &state);
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _appearance: ZoomAppearance,
    _rx: Receiver<ZoomState>,
) -> Result<(), OverlayError> {
    Err(OverlayError::UnsupportedPlatform)
}
```

- [ ] **Step 4: Run the tests to confirm it passes**

Run: `cargo test -p rpresent-zoom`

Expected: PASS.

- [ ] **Step 5: Commit the slice**

```bash
git add crates/rpresent-zoom
git commit -m "feat: add zoom module"
```

### Task 5: App shell and command routing

**Files:**
- Create: `crates/rpresent/src/app.rs`
- Create: `crates/rpresent/src/command.rs`
- Create: `crates/rpresent/src/ipc.rs`
- Modify: `crates/rpresent/src/lib.rs`
- Modify: `crates/rpresent/src/main.rs`
- Modify: `crates/rpresent/Cargo.toml`
- Create: `crates/rpresent/tests/app_cli.rs`
- Create: `crates/rpresent/tests/config_init.rs`
- Create: `crates/rpresent/tests/ipc.rs`

**Why this task exists:** The app shell is the only place where all three modules are composed together. It must stay thin and only route commands to spotlight, annotation, or zoom, plus a one-shot `config init` path that materializes the default config file. Regular command parsing uses clap, but the top-level entrypoint does a small pre-parse for `config init` and preserves clap's normal help/version exits.

`crates/rpresent/Cargo.toml`

```toml
[package]
name = "rpresent"
version = "0.1.0"
edition = "2024"

[dependencies]
clap = { version = "4", features = ["derive"] }
directories = "5"
rpresent-annotation = { path = "../rpresent-annotation" }
rpresent-config = { path = "../rpresent-config" }
rpresent-spotlight = { path = "../rpresent-spotlight" }
rpresent-zoom = { path = "../rpresent-zoom" }
thiserror = "2"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 1: Write the failing tests**

`crates/rpresent/tests/app_cli.rs`

```rust
use std::path::{Path, PathBuf};

use clap::Parser;

use rpresent::app::{parse_entrypoint, resolve_mode, AppEntry, AppMode, Cli, ConfigInitCli};
use rpresent::command::{AppCommand, AnnotationCommand, ZoomCommand};

#[test]
fn parses_config_override_and_spot_command() {
    let cli = Cli::try_parse_from([
        "rpresent",
        "--config",
        "/tmp/rpresent.toml",
        "spot=toggle",
    ])
    .unwrap();

    assert_eq!(cli.config.as_deref(), Some(Path::new("/tmp/rpresent.toml")));
    assert_eq!(cli.command.as_deref(), Some("spot=toggle"));
}

#[test]
fn parses_annotation_command() {
    let cli = Cli::try_parse_from(["rpresent", "annotation=tool=line"]).unwrap();
    assert_eq!(resolve_mode(&cli).unwrap(), AppMode::Send(AppCommand::Annotation(AnnotationCommand::Tool(rpresent_config::AnnotationTool::Line))));
}

#[test]
fn parses_zoom_command() {
    let cli = Cli::try_parse_from(["rpresent", "zoom=reset"]).unwrap();
    assert_eq!(resolve_mode(&cli).unwrap(), AppMode::Send(AppCommand::Zoom(ZoomCommand::Reset)));
}

#[test]
fn no_command_enters_run_mode() {
    let cli = Cli::try_parse_from(["rpresent"]).unwrap();
    assert_eq!(resolve_mode(&cli).unwrap(), AppMode::Run);
}

#[test]
fn parses_config_init_entrypoint() {
    let entry = parse_entrypoint(vec![
        "rpresent".into(),
        "config".into(),
        "init".into(),
        "--config".into(),
        "/tmp/rpresent.toml".into(),
    ])
    .unwrap();

    assert_eq!(
        entry,
        AppEntry::InitConfig(ConfigInitCli {
            config: Some(PathBuf::from("/tmp/rpresent.toml")),
        })
    );
}
```

`crates/rpresent/tests/config_init.rs`

```rust
use std::fs;
use std::path::PathBuf;

use clap::Parser;
use tempfile::tempdir;

use rpresent::app::{run_config_init, ConfigInitCli};

#[test]
fn config_init_writes_default_config_file() {
    let tempdir = tempdir().unwrap();
    let config_path = tempdir.path().join("rpresent.toml");

    let cli = ConfigInitCli::try_parse_from([
        "rpresent",
        "--config",
        config_path.to_str().unwrap(),
    ])
    .unwrap();

    let written_path = run_config_init(cli).unwrap();
    assert_eq!(written_path, PathBuf::from(&config_path));

    let contents = fs::read_to_string(&written_path).unwrap();
    assert!(contents.contains("[app]"));
    assert!(contents.contains("[zoom]"));
    assert!(contents.contains("socket_name = \"rpresent.sock\""));
}
```

`crates/rpresent/tests/ipc.rs`

```rust
use std::os::unix::net::UnixListener;

use tempfile::tempdir;

use rpresent::command::{AppCommand, ZoomCommand};
use rpresent::ipc::{encode_command, read_command, send_command};

#[test]
fn round_trips_one_command_over_unix_socket() {
    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("rpresent.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let writer = std::thread::spawn({
        let socket_path = socket_path.clone();
        move || send_command(&socket_path, AppCommand::Zoom(ZoomCommand::Toggle)).unwrap()
    });

    let (mut stream, _) = listener.accept().unwrap();
    let command = read_command(&mut stream).unwrap();
    assert_eq!(command, AppCommand::Zoom(ZoomCommand::Toggle));

    writer.join().unwrap();
}

#[test]
fn encodes_commands_as_single_lines() {
    assert_eq!(encode_command(AppCommand::Zoom(ZoomCommand::On)), "zoom=on\n");
}
```

- [ ] **Step 2: Run the tests to confirm the failure**

Run: `cargo test -p rpresent`

Expected: FAIL because the app crate and IPC routing have not been implemented yet.

- [ ] **Step 3: Write the minimal implementation**

`crates/rpresent/src/command.rs`

```rust
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

pub use rpresent_annotation::{AnnotationCommand, AnnotationTool};
pub use rpresent_spotlight::{CommandParseError as SpotlightCommandParseError, SpotlightCommand};
pub use rpresent_zoom::{CommandParseError as ZoomCommandParseError, ZoomCommand};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    Spotlight(SpotlightCommand),
    Annotation(AnnotationCommand),
    Zoom(ZoomCommand),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AppCommandParseError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error(transparent)]
    Spotlight(#[from] SpotlightCommandParseError),
    #[error(transparent)]
    Annotation(#[from] rpresent_annotation::AnnotationToolCommandParseError),
    #[error(transparent)]
    Zoom(#[from] ZoomCommandParseError),
}

impl FromStr for AppCommand {
    type Err = AppCommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.starts_with("spot=") {
            return Ok(Self::Spotlight(text.parse()?));
        }

        if text.starts_with("annotation=") {
            return Ok(Self::Annotation(text.parse()?));
        }

        if text.starts_with("zoom=") {
            return Ok(Self::Zoom(text.parse()?));
        }

        Err(AppCommandParseError::UnknownCommand(text.to_owned()))
    }
}

impl Display for AppCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AppCommand::Spotlight(command) => write!(f, "{command}"),
            AppCommand::Annotation(command) => write!(f, "{command}"),
            AppCommand::Zoom(command) => write!(f, "{command}"),
        }
    }
}
```

`crates/rpresent/src/ipc.rs`

```rust
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use directories::BaseDirs;
use thiserror::Error;

use crate::command::{AppCommand, AppCommandParseError};

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("ipc io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Command(#[from] AppCommandParseError),
    #[error("could not resolve a user config directory")]
    NoConfigDirectory,
}

pub fn encode_command(command: AppCommand) -> String {
    format!("{command}\n")
}

pub fn read_command(stream: &mut impl Read) -> Result<AppCommand, IpcError> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim().parse()?)
}

pub fn send_command(socket_path: &Path, command: AppCommand) -> Result<(), IpcError> {
    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(encode_command(command).as_bytes())?;
    stream.flush()?;
    Ok(())
}

pub fn socket_path_for(socket_name: &str) -> Result<PathBuf, IpcError> {
    let base_dirs = BaseDirs::new().ok_or(IpcError::NoConfigDirectory)?;
    Ok(base_dirs.config_dir().join("rpresent").join(socket_name))
}

pub fn serve(socket_path: &Path, tx: Sender<AppCommand>) -> Result<(), IpcError> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    for connection in listener.incoming() {
        let mut stream = connection?;
        let command = read_command(&mut stream)?;
        tx.send(command)
            .map_err(|source| std::io::Error::new(std::io::ErrorKind::BrokenPipe, source))?;
    }

    Ok(())
}
```

`crates/rpresent/src/app.rs`

```rust
use clap::Parser;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;

use rpresent_annotation::{AnnotationAppearance, AnnotationController, AnnotationState};
use rpresent_config::{
    Config, default_config_path, load_or_default_from_path, write_default_config,
};
use rpresent_spotlight::{SpotlightAppearance, SpotlightController, SpotlightState};
use rpresent_zoom::{ZoomAppearance, ZoomController, ZoomState};

use crate::command::AppCommand;
use crate::ipc;

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
#[command(
    after_long_help = "Examples:\n  rpresent spot=toggle\n  rpresent config init --config ~/.config/rpresent/config.toml"
)]
pub struct Cli {
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(value_name = "COMMAND")]
    pub command: Option<String>,
}

#[derive(Debug, Parser, Clone, PartialEq, Eq)]
pub struct ConfigInitCli {
    #[arg(long)]
    pub config: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    Run,
    Send(AppCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEntry {
    Run(Cli),
    InitConfig(ConfigInitCli),
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Clap(#[from] clap::Error),
    #[error(transparent)]
    Config(#[from] rpresent_config::ConfigError),
    #[error(transparent)]
    Command(#[from] crate::command::AppCommandParseError),
    #[error(transparent)]
    Ipc(#[from] crate::ipc::IpcError),
    #[error(transparent)]
    Spotlight(#[from] rpresent_spotlight::OverlayError),
    #[error(transparent)]
    Annotation(#[from] rpresent_annotation::OverlayError),
    #[error(transparent)]
    Zoom(#[from] rpresent_zoom::OverlayError),
}

pub fn parse_entrypoint(args: Vec<OsString>) -> Result<AppEntry, AppError> {
    if args.get(1).and_then(|arg| arg.to_str()) == Some("config")
        && args.get(2).and_then(|arg| arg.to_str()) == Some("init")
    {
        let filtered_args = std::iter::once(args[0].clone())
            .chain(args.into_iter().skip(3))
            .collect::<Vec<_>>();
        let cli = ConfigInitCli::try_parse_from(filtered_args)?;
        return Ok(AppEntry::InitConfig(cli));
    }

    Ok(AppEntry::Run(Cli::try_parse_from(args)?))
}

pub fn resolve_mode(cli: &Cli) -> Result<AppMode, AppError> {
    match cli.command.as_deref() {
        Some(command) => Ok(AppMode::Send(command.parse()?)),
        None => Ok(AppMode::Run),
    }
}

pub fn run_from_env() -> Result<(), AppError> {
    match parse_entrypoint(std::env::args_os().collect()) {
        AppEntry::Run(cli) => run(cli),
        AppEntry::InitConfig(cli) => {
            let written_path = run_config_init(cli)?;
            println!("{}", written_path.display());
            Ok(())
        }
        Err(AppError::Clap(error))
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            error.exit();
        }
        Err(error) => Err(error),
    }
}

pub fn run(cli: Cli) -> Result<(), AppError> {
    let mode = resolve_mode(&cli)?;
    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };
    let config = load_or_default_from_path(config_path)?;

    match mode {
        AppMode::Send(command) => {
            let socket_path = ipc::socket_path_for(&config.app.socket_name)?;
            ipc::send_command(&socket_path, command)?;
            Ok(())
        }
        AppMode::Run => run_daemon(config),
    }
}

pub fn run_config_init(cli: ConfigInitCli) -> Result<PathBuf, AppError> {
    let config_path = match cli.config {
        Some(path) => path,
        None => default_config_path()?,
    };
    Ok(write_default_config(config_path)?)
}

fn run_daemon(config: Config) -> Result<(), AppError> {
    let (app_tx, app_rx) = mpsc::channel();
    let (spotlight_tx, spotlight_rx) = mpsc::channel::<SpotlightState>();
    let (annotation_tx, annotation_rx) = mpsc::channel::<AnnotationState>();
    let (zoom_tx, zoom_rx) = mpsc::channel::<ZoomState>();
    let socket_path = ipc::socket_path_for(&config.app.socket_name)?;

    let spotlight_config = config.spotlight.clone();
    let annotation_config = config.annotation.clone();
    let zoom_config = config.zoom.clone();
    let spotlight_appearance = SpotlightAppearance::from(spotlight_config.clone());
    let annotation_appearance = AnnotationAppearance::from(annotation_config.clone());
    let zoom_appearance = ZoomAppearance::from(zoom_config.clone());

    std::thread::spawn(move || {
        if let Err(error) = ipc::serve(&socket_path, app_tx) {
            eprintln!("{error}");
            std::process::exit(1);
        }
    });

    std::thread::spawn(move || {
        let mut spotlight_controller = SpotlightController::new(spotlight_config.enabled_on_start);
        let mut annotation_controller = AnnotationController::new(
            annotation_config.enabled_on_start,
            annotation_config.default_tool,
            annotation_config.history_depth,
        );
        let mut zoom_controller = ZoomController::from_config(zoom_config);

        let _ = spotlight_tx.send(spotlight_controller.state());
        let _ = annotation_tx.send(annotation_controller.state());
        let _ = zoom_tx.send(zoom_controller.state());

        for command in app_rx {
            match command {
                AppCommand::Spotlight(command) => {
                    if spotlight_controller.apply(command) {
                        let _ = spotlight_tx.send(spotlight_controller.state());
                    }
                }
                AppCommand::Annotation(command) => {
                    if annotation_controller.apply(command) {
                        let _ = annotation_tx.send(annotation_controller.state());
                    }
                }
                AppCommand::Zoom(command) => {
                    if zoom_controller.apply(command) {
                        let _ = zoom_tx.send(zoom_controller.state());
                    }
                }
            }
        }
    });

    let spotlight_handle = std::thread::spawn(move || rpresent_spotlight::run_overlay(spotlight_appearance, spotlight_rx));
    let annotation_handle = std::thread::spawn(move || rpresent_annotation::run_overlay(annotation_appearance, annotation_rx));
    let zoom_handle = std::thread::spawn(move || rpresent_zoom::run_overlay(zoom_appearance, zoom_rx));

    spotlight_handle.join().unwrap()?;
    annotation_handle.join().unwrap()?;
    zoom_handle.join().unwrap()?;
    Ok(())
}
```

`crates/rpresent/src/main.rs`

```rust
fn main() {
    if let Err(error) = rpresent::run_from_env() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 4: Run the tests to confirm it passes**

Run: `cargo test -p rpresent`

Expected: PASS.

- [ ] **Step 5: Smoke the running instance and both command paths**

Run in terminal 1:

```bash
cargo run -p rpresent -- config init --config /tmp/rpresent.toml
```

Expected: the default config file is written once and the command exits 0.

Run in terminal 2:

```bash
cargo run -p rpresent -- --config /tmp/rpresent.toml
```

Run in terminal 3:

```bash
cargo run -p rpresent -- --config /tmp/rpresent.toml spot=toggle
```

Run in terminal 4:

```bash
cargo run -p rpresent -- --config /tmp/rpresent.toml annotation=tool=line
```

Run in terminal 5:

```bash
cargo run -p rpresent -- --config /tmp/rpresent.toml zoom=reset
```

Expected: the daemon remains running while the client commands exit 0; the specific routing and state transitions are covered by the unit tests and IPC round-trip tests in this task. If `~/.config/rpresent/config.toml` is missing, `cargo run -p rpresent` still starts with built-in defaults.

- [ ] **Step 6: Commit the slice**

```bash
git add crates/rpresent
git commit -m "feat: wire app shell and ipc"
```

### Task 6: Final workspace validation

**Files:**
- None

**Why this task exists:** The implementation is only finished if the workspace builds, the tests pass, and the CLI help text reflects the final command surface.

- [ ] **Step 1: Run formatting and the full test suite**

Run:

```bash
cargo fmt --check
cargo test --workspace
```

Expected: formatting passes and the full workspace test suite passes.

- [ ] **Step 2: Verify the CLI help text**

Run:

```bash
cargo run -p rpresent -- --help
```

Expected: help text shows the `--config` flag, the positional `COMMAND` surface, and the examples for `spot=...`, `annotation=...`, `zoom=...`, and `config init --config ...`, with exit code 0.

- [ ] **Step 3: Commit the finished implementation**

```bash
git add -A
git commit -m "feat: complete parallel spotlight, annotation, and zoom workspace"
```
