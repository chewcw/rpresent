use std::fs;
use std::path::Path;

use tempfile::tempdir;

use rpresent_config::{
    AnnotationTool, Config, ConfigError, SpotShape, default_config_path, load_from_str,
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

        [shortcuts]
        spotlight_mode = "Ctrl+1"
        annotation_mode = "Ctrl+2"
        zoom_mode = "Ctrl+3"
        spotlight_wheel_modifier = "Ctrl"
        "#,
    )
    .unwrap();

    assert_eq!(config.app.socket_name, "present.sock");
    assert!(config.spotlight.enabled_on_start);
    assert_eq!(config.spotlight.radius_px, 260);
    assert_eq!(config.spotlight.softness_px, 80);
    assert!((config.spotlight.dim_alpha - 0.72).abs() < f32::EPSILON);
    // Projecteur-style defaults applied when not specified in TOML
    assert!(config.spotlight.show_spot_shade);
    assert_eq!(config.spotlight.spot_size_pct, 30);
    assert_eq!(config.spotlight.shape, SpotShape::Circle);
    assert!((config.spotlight.spot_rotation_deg - 0.0).abs() < f32::EPSILON);
    assert!(config.spotlight.show_center_dot);
    assert_eq!(config.spotlight.dot_size_px, 8);
    assert_eq!(config.spotlight.dot_color, "#ffffff");
    assert!((config.spotlight.dot_opacity - 0.8).abs() < f32::EPSILON);
    assert_eq!(config.spotlight.shade_color, "#000000");
    assert!((config.spotlight.shade_opacity - 0.6).abs() < f32::EPSILON);
    assert!(config.spotlight.show_border);
    assert_eq!(config.spotlight.border_color, "#ffffff");
    assert_eq!(config.spotlight.border_size_pct, 10);
    assert!((config.spotlight.border_opacity - 0.8).abs() < f32::EPSILON);
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
    assert_eq!(config.shortcuts.spotlight_mode, "Ctrl+1");
    assert_eq!(config.shortcuts.annotation_mode, "Ctrl+2");
    assert_eq!(config.shortcuts.zoom_mode, "Ctrl+3");
    assert_eq!(config.shortcuts.spotlight_wheel_modifier, "Ctrl");
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
    assert!(contents.contains("[shortcuts]"));
    assert!(contents.contains("socket_name = \"rpresent.sock\""));
    assert!(contents.contains("spotlight_mode = \"Ctrl+1\""));
    assert!(contents.contains("spotlight_wheel_modifier = \"Ctrl\""));
    assert!(contents.contains("spot_size_pct = 30"));
    assert!(contents.contains("shade_color = \"#000000\""));
    assert!(contents.contains("border_size_pct = 10"));
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
