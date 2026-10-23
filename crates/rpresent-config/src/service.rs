use std::path::{Path, PathBuf};

use crate::{
    AnnotationConfig, AppConfig, Config, ConfigError, ShortcutConfig, SpotlightConfig, ZoomConfig,
    load_or_default_from_path,
};

/// Owns the loaded config and the path it came from.
#[derive(Debug)]
pub struct ConfigService {
    path: PathBuf,
    config: Config,
}

impl ConfigService {
    /// Load the config from `path`. A missing file gives the default config.
    pub fn load_or_default(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        Ok(Self {
            config: load_or_default_from_path(&path)?,
            path,
        })
    }

    /// Build a service from a config value that is already in memory.
    pub fn new(path: PathBuf, config: Config) -> Self {
        Self { path, config }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn app(&self) -> &AppConfig {
        &self.config.app
    }

    pub fn spotlight(&self) -> &SpotlightConfig {
        &self.config.spotlight
    }

    pub fn annotation(&self) -> &AnnotationConfig {
        &self.config.annotation
    }

    pub fn zoom(&self) -> &ZoomConfig {
        &self.config.zoom
    }

    pub fn shortcuts(&self) -> &ShortcutConfig {
        &self.config.shortcuts
    }
}
