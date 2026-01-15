use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use directories::ProjectDirs;
use serde::Deserialize;

use crate::distro::Distro;

const DEFAULT_CONFIG: &str = include_str!("../config/default.json");

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub version: u8,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub requires_sudo: bool,
    #[serde(default)]
    pub enabled_by_default: bool,
    #[serde(default)]
    pub distros: Vec<String>,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        if let Some(path) = path {
            return Self::from_path(path);
        }

        if let Some(default_path) = default_config_path() {
            if default_path.exists() {
                return Self::from_path(&default_path);
            }
        }

        let config: Config = serde_json::from_str(DEFAULT_CONFIG)
            .context("Failed to parse embedded default config")?;
        config.ensure_supported()?;
        Ok(config)
    }

    fn from_path(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file {}", path.display()))?;
        let config: Config = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse config file {}", path.display()))?;
        config.ensure_supported()?;
        Ok(config)
    }

    pub fn available_rules(&self, distro: &Distro) -> Vec<Rule> {
        let ids = distro.identifiers();
        self.rules
            .iter()
            .cloned()
            .filter(|rule| rule.matches_distro(&ids))
            .collect()
    }

    fn ensure_supported(&self) -> Result<()> {
        if self.version != 1 {
            bail!("Unsupported config version {}", self.version);
        }
        Ok(())
    }
}

impl Rule {
    pub fn matches_distro(&self, distro_ids: &[String]) -> bool {
        if self.distros.is_empty() {
            return true;
        }
        let distros = self
            .distros
            .iter()
            .map(|d| d.to_lowercase())
            .collect::<Vec<_>>();
        distros.iter().any(|d| distro_ids.iter().any(|id| id == d))
    }

    pub fn expanded_paths(&self) -> Vec<PathBuf> {
        self.paths
            .iter()
            .map(|raw| shellexpand::full(raw).unwrap_or_else(|_| raw.into()))
            .map(|expanded| PathBuf::from(expanded.as_ref()))
            .collect()
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "vole", "vole").map(|dirs| dirs.config_dir().join("config.json"))
}
