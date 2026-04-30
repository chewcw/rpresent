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
    #[serde(default)]
    pub shortcuts: ShortcutConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default = "default_socket_name")]
    pub socket_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationTool {
    #[default]
    Pen,
    Line,
    Eraser,
}

/// Shape of the spotlight cutout, mirroring Projecteur's shape variants.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpotShape {
    #[default]
    Circle,
    Square {
        #[serde(default = "default_corner_radius_pct")]
        corner_radius_pct: u32,
    },
    Star {
        #[serde(default = "default_star_points")]
        points: u32,
        #[serde(default = "default_star_inner_radius_pct")]
        inner_radius_pct: u32,
    },
    Ngon {
        #[serde(default = "default_ngon_sides")]
        sides: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotlightConfig {
    #[serde(default = "default_spotlight_enabled")]
    pub enabled_on_start: bool,

    // Projecteur-style visual fields
    #[serde(default = "default_show_spot_shade")]
    pub show_spot_shade: bool,
    #[serde(default = "default_spot_size_pct")]
    pub spot_size_pct: u32,
    #[serde(default)]
    pub shape: SpotShape,
    #[serde(default = "default_spot_rotation_deg")]
    pub spot_rotation_deg: f32,
    #[serde(default = "default_show_center_dot")]
    pub show_center_dot: bool,
    #[serde(default = "default_dot_size_px")]
    pub dot_size_px: u32,
    #[serde(default = "default_dot_color")]
    pub dot_color: String,
    #[serde(default = "default_dot_opacity")]
    pub dot_opacity: f32,
    #[serde(default = "default_shade_color")]
    pub shade_color: String,
    #[serde(default = "default_shade_opacity")]
    pub shade_opacity: f32,
    #[serde(default = "default_show_border")]
    pub show_border: bool,
    #[serde(default = "default_border_color")]
    pub border_color: String,
    #[serde(default = "default_border_size_pct")]
    pub border_size_pct: u32,
    #[serde(default = "default_border_opacity")]
    pub border_opacity: f32,

    // Legacy compatibility fields — kept as a bridge; new renderer does not use these.
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
            socket_name: default_socket_name(),
        }
    }
}

impl Default for SpotlightConfig {
    fn default() -> Self {
        Self {
            enabled_on_start: default_spotlight_enabled(),
            show_spot_shade: default_show_spot_shade(),
            spot_size_pct: default_spot_size_pct(),
            shape: SpotShape::default(),
            spot_rotation_deg: default_spot_rotation_deg(),
            show_center_dot: default_show_center_dot(),
            dot_size_px: default_dot_size_px(),
            dot_color: default_dot_color(),
            dot_opacity: default_dot_opacity(),
            shade_color: default_shade_color(),
            shade_opacity: default_shade_opacity(),
            show_border: default_show_border(),
            border_color: default_border_color(),
            border_size_pct: default_border_size_pct(),
            border_opacity: default_border_opacity(),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortcutConfig {
    #[serde(default = "default_spotlight_mode_shortcut")]
    pub spotlight_mode: String,
    #[serde(default = "default_annotation_mode_shortcut")]
    pub annotation_mode: String,
    #[serde(default = "default_zoom_mode_shortcut")]
    pub zoom_mode: String,
    #[serde(default = "default_wheel_modifier")]
    pub wheel_modifier: String,
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
            capture_target: default_capture_target(),
        }
    }
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            spotlight_mode: default_spotlight_mode_shortcut(),
            annotation_mode: default_annotation_mode_shortcut(),
            zoom_mode: default_zoom_mode_shortcut(),
            wheel_modifier: default_wheel_modifier(),
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

pub fn default_config_path() -> Result<PathBuf, ConfigError> {
    let base_dirs = BaseDirs::new().ok_or(ConfigError::NoConfigDirectory)?;
    Ok(base_dirs.config_dir().join("rpresent").join("config.toml"))
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

fn default_socket_name() -> String {
    "rpresent.sock".to_owned()
}

fn default_spotlight_enabled() -> bool {
    true
}

fn default_show_spot_shade() -> bool {
    true
}

fn default_spot_size_pct() -> u32 {
    30
}

fn default_spot_rotation_deg() -> f32 {
    0.0
}

fn default_show_center_dot() -> bool {
    true
}

fn default_dot_size_px() -> u32 {
    8
}

fn default_dot_color() -> String {
    "#ffffff".to_owned()
}

fn default_dot_opacity() -> f32 {
    0.8
}

fn default_shade_color() -> String {
    "#000000".to_owned()
}

fn default_shade_opacity() -> f32 {
    0.6
}

fn default_show_border() -> bool {
    true
}

fn default_border_color() -> String {
    "#ffffff".to_owned()
}

fn default_border_size_pct() -> u32 {
    10
}

fn default_border_opacity() -> f32 {
    0.8
}

fn default_corner_radius_pct() -> u32 {
    20
}

fn default_star_points() -> u32 {
    5
}

fn default_star_inner_radius_pct() -> u32 {
    50
}

fn default_ngon_sides() -> u32 {
    3
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

fn default_capture_target() -> String {
    "active-monitor".to_owned()
}

fn default_spotlight_mode_shortcut() -> String {
    "Ctrl+1".to_owned()
}

fn default_annotation_mode_shortcut() -> String {
    "Ctrl+2".to_owned()
}

fn default_zoom_mode_shortcut() -> String {
    "Ctrl+3".to_owned()
}

fn default_wheel_modifier() -> String {
    "Ctrl".to_owned()
}

fn default_stroke_width_px() -> u32 {
    6
}

fn default_history_depth() -> usize {
    64
}
