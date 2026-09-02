use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// The parsed `.relic/config.yaml` configuration. This is the stable,
/// user-editable contract for a vault; unknown fields are ignored so a newer
/// config does not break an older binary.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub vault: VaultConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub evolution: EvolutionConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    pub name: String,
    pub default_confidence: f64,
    pub default_decay_rate: f64,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            name: "Relic Vault".into(),
            default_confidence: 0.7,
            default_decay_rate: 0.05,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SyncConfig {
    pub mode: String,
    pub remotes: Vec<Remote>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            mode: "manual".into(),
            remotes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EvolutionConfig {
    pub fading_threshold: f64,
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            fading_threshold: 0.3,
        }
    }
}

impl Config {
    /// Load and validate the vault config at `<root>/.relic/config.yaml`.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(".relic").join("config.yaml");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("missing vault config {}", path.display()))?;
        let config: Config = serde_yaml::from_str(&content).context("invalid vault config")?;
        config.validate()?;
        Ok(config)
    }

    /// Enforce the stable schema contract: supported version, bounded numeric
    /// ranges, and well-formed, unique sync remotes.
    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported schema version {} (expected 1)", self.version);
        }
        if !(0.0..=1.0).contains(&self.vault.default_confidence) {
            bail!("vault.default_confidence must be between 0 and 1");
        }
        if self.vault.default_decay_rate < 0.0 {
            bail!("vault.default_decay_rate cannot be negative");
        }
        if !(0.0..=1.0).contains(&self.evolution.fading_threshold) {
            bail!("evolution.fading_threshold must be between 0 and 1");
        }
        if !matches!(self.sync.mode.as_str(), "manual" | "auto") {
            bail!("sync.mode must be 'manual' or 'auto'");
        }
        let mut names = HashSet::new();
        for remote in &self.sync.remotes {
            if remote.name.trim().is_empty() || remote.url.trim().is_empty() {
                bail!("each sync remote must have a name and a url");
            }
            if !names.insert(&remote.name) {
                bail!("duplicate sync remote '{}'", remote.name);
            }
        }
        Ok(())
    }
}
