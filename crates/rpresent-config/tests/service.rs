use std::path::{Path, PathBuf};

use rpresent_config::{Config, ConfigError, ConfigService, SpotlightConfig, ZoomConfig};

#[test]
fn load_or_default_keeps_the_caller_path_and_gives_defaults_for_a_missing_file() {
    let path = PathBuf::from("/nonexistent/rpresent/config.toml");

    let config_service = ConfigService::load_or_default(path.clone()).unwrap();

    assert_eq!(config_service.path(), path.as_path());
    assert_eq!(config_service.spotlight(), &SpotlightConfig::default());
    assert_eq!(config_service.app(), &Config::default().app);
}

#[test]
fn load_or_default_exposes_the_loaded_sections() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("config.toml");
    std::fs::write(&path, "[spotlight]\nradius_px = 42\n").unwrap();

    let config_service = ConfigService::load_or_default(path).unwrap();

    assert_eq!(config_service.spotlight().radius_px, 42);
    assert_eq!(config_service.zoom(), &ZoomConfig::default());
}

#[test]
fn new_builds_a_service_from_an_in_memory_config() {
    let config = Config {
        spotlight: SpotlightConfig {
            radius_px: 70,
            ..SpotlightConfig::default()
        },
        ..Config::default()
    };

    let config_service = ConfigService::new(PathBuf::from("test.toml"), config).unwrap();

    assert_eq!(config_service.path(), Path::new("test.toml"));
    assert_eq!(config_service.spotlight().radius_px, 70);
    assert_eq!(config_service.bindings().global.len(), 3);
}

#[test]
fn load_or_default_reports_a_parse_error_for_unknown_fields() {
    let tempdir = tempfile::tempdir().unwrap();
    let path = tempdir.path().join("config.toml");
    std::fs::write(&path, "[spotlight]\nnot_a_field = 1\n").unwrap();

    let error = ConfigService::load_or_default(path).unwrap_err();

    assert!(matches!(error, ConfigError::Parse { .. }));
}
