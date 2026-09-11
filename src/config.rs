use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// The parsed `.relic/config.yaml` configuration. This is the stable,
/// user-editable contract for a vault; unknown fields are ignored so a newer
/// config does not break an older binary.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub vault: VaultConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub evolution: EvolutionConfig,
    #[serde(default)]
    pub graph: GraphConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// Settings for the derived knowledge graph and its vector layer.
///
/// Every field is a threshold on *derived* data, so changing one only changes
/// what `relic graph build` infers. Nothing here rewrites Markdown, and the
/// defaults are conservative: the graph should state relations a reader would
/// accept, not every coincidence cosine can find.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GraphConfig {
    /// Dimensions of the hashed feature space. More dimensions mean fewer
    /// collisions and more memory; fewer mean faster builds and more noise.
    pub dimensions: usize,
    /// Neighbours kept per memory when deriving semantic edges.
    pub semantic_top_k: usize,
    /// Minimum cosine similarity for a semantic edge.
    ///
    /// Calibrated from measured values rather than guessed: over realistic
    /// memories, pairs that share a subject score 0.18–0.32 while unrelated
    /// pairs score 0.00–0.03, so the floor sits in the middle of that gap on a
    /// ratio scale. `tests/graph_calibration.rs` guards the separation.
    pub semantic_min_similarity: f64,
    /// Minimum Jaccard overlap of subject tags for a tag edge. The default
    /// excludes pairs that share only one tag out of three, which is too weak a
    /// claim to draw a line for.
    pub tag_min_jaccard: f64,
    /// Minimum cosine similarity for a corroboration edge. Corroboration also
    /// requires a shared subject and matching claim polarity, and claims more
    /// than proximity, so it demands more than the semantic floor.
    pub corroborate_min_similarity: f64,
    /// Maximum inferred edges kept per memory, strongest first. Declared links,
    /// version history, and contradictions are never pruned.
    pub max_edges_per_node: usize,
}

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            dimensions: 8192,
            semantic_top_k: 8,
            semantic_min_similarity: 0.08,
            tag_min_jaccard: 0.34,
            corroborate_min_similarity: 0.15,
            max_edges_per_node: 24,
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
        if !self.vault.default_decay_rate.is_finite() || self.vault.default_decay_rate < 0.0 {
            bail!("vault.default_decay_rate must be finite and non-negative");
        }
        if !(0.0..=1.0).contains(&self.evolution.fading_threshold) {
            bail!("evolution.fading_threshold must be between 0 and 1");
        }
        self.graph.validate()?;
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

impl GraphConfig {
    /// Reject settings that would make the graph meaningless rather than let
    /// them silently produce an empty or fully-connected network.
    pub fn validate(&self) -> Result<()> {
        if !(256..=65536).contains(&self.dimensions) {
            bail!("graph.dimensions must be between 256 and 65536");
        }
        if !(1..=64).contains(&self.semantic_top_k) {
            bail!("graph.semantic_top_k must be between 1 and 64");
        }
        if !(0.0..=1.0).contains(&self.semantic_min_similarity) {
            bail!("graph.semantic_min_similarity must be between 0 and 1");
        }
        if !(0.0..=1.0).contains(&self.tag_min_jaccard) {
            bail!("graph.tag_min_jaccard must be between 0 and 1");
        }
        if !(0.0..=1.0).contains(&self.corroborate_min_similarity) {
            bail!("graph.corroborate_min_similarity must be between 0 and 1");
        }
        if !(1..=512).contains(&self.max_edges_per_node) {
            bail!("graph.max_edges_per_node must be between 1 and 512");
        }
        Ok(())
    }
}
