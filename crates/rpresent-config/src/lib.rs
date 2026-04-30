mod config;

pub use config::{
    AnnotationConfig, AnnotationTool, AppConfig, Config, ConfigError, ShortcutConfig, SpotShape,
    SpotlightConfig, ZoomConfig, default_config_path, default_config_toml, load_from_path,
    load_from_str, load_or_default_from_path, write_default_config,
};
