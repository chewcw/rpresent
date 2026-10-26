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
        wheel_speed = 8.0

        [annotation]
        default_tool = "line"

        [zoom]
        initial_scale = 2.5
        drag_friction = 6.0
        scale_friction = 5.0

        [shortcuts.global]
        "spotlight.toggle" = "Ctrl+1"
        "annotation.toggle" = "Ctrl+2"
        "zoom.toggle" = "Ctrl+3"

        [shortcuts.wheel]
        modifiers = "Ctrl"
        "#,
    )
    .unwrap();

    assert_eq!(config.app.socket_name, "present.sock");
    assert!(config.spotlight.enabled_on_start);
    assert_eq!(config.spotlight.radius_px, 260);
    assert_eq!(config.spotlight.softness_px, 80);
    assert!((config.spotlight.wheel_speed - 8.0).abs() < f32::EPSILON);
    // Projecteur-style defaults applied when not specified in TOML
    assert!(config.spotlight.show_spot_shade);
    assert_eq!(config.spotlight.shape, SpotShape::Circle);
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
    assert_eq!(config.zoom.initial_scale, 2.5);
    assert_eq!(config.zoom.min_scale, 1.0);
    assert_eq!(config.zoom.max_scale, 6.0);
    assert_eq!(config.zoom.drag_friction, 6.0);
    assert_eq!(config.zoom.scale_friction, 5.0);
    assert_eq!(
        config
            .shortcuts
            .global
            .get("spotlight.toggle")
            .map(String::as_str),
        Some("Ctrl+1")
    );
    assert_eq!(
        config
            .shortcuts
            .global
            .get("zoom.toggle")
            .map(String::as_str),
        Some("Ctrl+3")
    );
    assert_eq!(config.shortcuts.wheel.modifiers, "Ctrl");
}

#[test]
fn annotation_presets_require_valid_initial_selections() {
    let mut config = rpresent_config::Config::default();
    assert_eq!(config.annotation.stroke_widths_px, vec![2, 4, 6, 10]);
    assert_eq!(config.annotation.text_sizes_px, vec![16, 24, 32, 48]);
    assert!(config.annotation.validate().is_ok());
    config.annotation.stroke_width_px = 7;
    assert!(config.annotation.validate().is_err());
}

#[test]
fn annotation_rejects_invalid_catalogs() {
    let cases: [fn(&mut Config); 8] = [
        |config| config.annotation.colors.clear(),
        |config| config.annotation.colors[0] = "red".to_owned(),
        |config| config.annotation.color = "#123456".to_owned(),
        |config| config.annotation.stroke_widths_px.clear(),
        |config| config.annotation.stroke_widths_px = vec![0, 6],
        |config| config.annotation.text_sizes_px.clear(),
        |config| config.annotation.text_size_px = 0,
        |config| config.annotation.history_depth = 0,
    ];
    for (index, mutate) in cases.into_iter().enumerate() {
        let mut config = Config::default();
        mutate(&mut config);
        assert!(
            config.annotation.validate().is_err(),
            "case {index} should be rejected"
        );
    }
}

#[test]
fn annotation_colors_accept_any_case_and_duplicates() {
    let config = load_from_str(
        r##"
        [annotation]
        color = "#ff0000"
        colors = ["#ff0000", "#FF0000", "#00ff00"]
        "##,
    )
    .unwrap();
    assert_eq!(config.annotation.color, "#ff0000");
    assert!(config.annotation.validate().is_ok());
}

#[test]
fn rejects_unknown_keys() {
    let error = load_from_str("[spotlight]\nunknown = 1").unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn removed_spotlight_keys_are_rejected() {
    for entry in ["dim_alpha = 0.9", "spot_size_pct = 10"] {
        let error = load_from_str(&format!("[spotlight]\n{entry}\n")).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}

#[test]
fn default_spotlight_config_uses_only_canonical_keys() {
    let text = rpresent_config::default_config_toml();
    assert!(text.contains("radius_px = 240"));
    assert!(text.contains("shade_opacity = 0.6"));
    assert!(!text.contains("dim_alpha"));
    assert!(!text.contains("spot_size_pct"));
    assert_eq!(load_from_str(&text).unwrap(), Config::default());
}

#[test]
fn rotation_is_no_longer_a_spotlight_config_key() {
    let error = load_from_str("[spotlight]\nspot_rotation_deg = 45.0").unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn rejects_unknown_top_level_keys() {
    let error = load_from_str("unexpected = true").unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn rejects_removed_zoom_keys() {
    for entry in [
        "follow_cursor = true",
        "pan_friction = 0.8",
        "capture_target = \"root\"",
    ] {
        let error = load_from_str(&format!("[zoom]\n{entry}\n")).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }
}

#[test]
fn zoom_requires_valid_finite_motion_parameters() {
    for body in [
        "initial_scale = nan",
        "min_scale = 0.5",
        "initial_scale = 7.0",
        "scroll_speed = 0.0",
        "drag_friction = -1.0",
        "scale_friction = inf",
    ] {
        let text = format!("[zoom]\n{body}\n");
        assert!(load_from_str(&text).is_err(), "{body}");
    }
}

#[test]
fn missing_config_path_returns_the_built_in_defaults() {
    let tempdir = tempdir().unwrap();
    let path = tempdir.path().join("missing.toml");
    let config = load_or_default_from_path(&path).unwrap();

    assert_eq!(config, Config::default());
}

#[test]
fn custom_global_table_replaces_defaults_and_keeps_the_default_wheel() {
    let config = load_from_str(
        "[shortcuts.global]\n\"zoom.toggle\" = \"Ctrl+F8\"\n[shortcuts.annotation]\n",
    )
    .unwrap();

    assert_eq!(config.shortcuts.global.len(), 1);
    assert_eq!(
        config
            .shortcuts
            .global
            .get("zoom.toggle")
            .map(String::as_str),
        Some("Ctrl+F8")
    );
    assert!(config.shortcuts.annotation.is_empty());
    assert_eq!(config.shortcuts.wheel.modifiers, "Ctrl");
}

#[test]
fn legacy_shortcut_fields_are_rejected() {
    for entry in [
        "spotlight_mode = \"Ctrl+1\"",
        "annotation_mode = \"Ctrl+2\"",
        "zoom_mode = \"Ctrl+3\"",
        "wheel_modifier = \"Ctrl\"",
    ] {
        let error = load_from_str(&format!("[shortcuts]\n{entry}\n")).unwrap_err();
        assert!(
            error.to_string().contains("unknown field"),
            "{entry}: {error}"
        );
    }
}

#[test]
fn invalid_shortcut_bindings_fail_loading() {
    let error = load_from_str("[shortcuts.global]\n\"zoom.toggle\" = \"Ctrl+\"\n").unwrap_err();
    assert!(matches!(error, ConfigError::InvalidShortcuts { .. }));
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
    assert!(contents.contains("[shortcuts.global]"));
    assert!(contents.contains("socket_name = \"rpresent.sock\""));
    assert!(contents.contains("\"zoom.toggle\" = \"Ctrl+3\""));
    assert!(contents.contains("[shortcuts.wheel]"));
    assert!(contents.contains("modifiers = \"Ctrl\""));
    assert!(contents.contains("radius_px = 240"));
    assert!(contents.contains("shade_opacity = 0.6"));
    assert!(!contents.contains("dim_alpha"));
    assert!(!contents.contains("spot_size_pct"));
    assert!(contents.contains("shade_color = \"#000000\""));
    assert!(contents.contains("border_size_pct = 10"));
    assert!(contents.contains("wheel_speed = 12.0"));
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
