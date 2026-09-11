use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntryMeta {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: String,
    pub confidence: f64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source_agents: Vec<String>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
    pub last_verified: DateTime<Utc>,
    #[serde(default)]
    pub expires: Option<DateTime<Utc>>,
    #[serde(default)]
    pub supersedes: Vec<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default = "default_decay_rate")]
    pub decay_rate: f64,
}

fn default_decay_rate() -> f64 {
    0.05
}

const DAYS_PER_YEAR: f64 = 365.25;

impl EntryMeta {
    /// Confidence after continuous annual decay from the last verification.
    pub fn effective_confidence_at(&self, now: DateTime<Utc>) -> f64 {
        if self.expires.is_some_and(|expires| expires <= now) {
            return 0.0;
        }
        let elapsed_seconds = now
            .signed_duration_since(self.last_verified)
            .num_seconds()
            .max(0) as f64;
        let elapsed_years = elapsed_seconds / (DAYS_PER_YEAR * 24.0 * 60.0 * 60.0);
        (self.confidence * (-self.decay_rate * elapsed_years).exp()).clamp(0.0, 1.0)
    }

    pub fn effective_confidence(&self) -> f64 {
        self.effective_confidence_at(Utc::now())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub meta: EntryMeta,
    pub body: String,
    pub path: PathBuf,
}

/// Separates a tag's namespace from its value: `src:codex-memory`,
/// `project:gi03`, `section:failure`, `pool:preconscious`.
pub const TAG_NAMESPACE_SEPARATOR: char = ':';

/// A tag is either a *scope* or a *subject*.
///
/// A namespaced tag scopes a memory — where it came from, which project it
/// belongs to, which pool it sits in. A bare tag names a subject: what the
/// memory is about. Only subjects may take part in tag-derived relations,
/// contradictions and pattern proposals, because a scope shared by hundreds of
/// memories (an import batch, a project) says nothing about what any two of
/// them claim, and pairing them produces contradictions that are pure noise.
///
/// See `tools/relic-tidy.py` for the convention this encodes.
pub fn is_subject_tag(tag: &str) -> bool {
    !tag.contains(TAG_NAMESPACE_SEPARATOR)
}

/// The tags of an entry that name subjects rather than scopes.
pub fn subject_tags(tags: &[String]) -> impl Iterator<Item = &String> {
    tags.iter().filter(|tag| is_subject_tag(tag))
}

impl Entry {
    pub fn parse(path: &Path, input: &str) -> Result<Self> {
        let input = input
            .strip_prefix("---\n")
            .context("missing YAML front matter")?;
        let (yaml, body) = input
            .split_once("\n---\n")
            .context("unterminated YAML front matter")?;
        let meta: EntryMeta = serde_yaml::from_str(yaml).context("invalid entry metadata")?;
        validate_meta(&meta)?;
        Ok(Self {
            meta,
            body: body.trim().to_owned(),
            path: path.to_owned(),
        })
    }

    pub fn render(&self) -> Result<String> {
        let yaml = serde_yaml::to_string(&self.meta)?.trim().to_owned();
        Ok(format!("---\n{yaml}\n---\n\n{}\n", self.body.trim()))
    }

    /// SHA-256 over everything the derived layers read.
    ///
    /// The derived graph and the vector cache are keyed on the fingerprint of
    /// the whole vault, which is built from these digests. Hashing content
    /// rather than trusting `updated` means a hand edit that forgets to bump
    /// the timestamp still invalidates the cache instead of serving stale
    /// neighbours.
    pub fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        let meta = &self.meta;
        hasher.update(meta.id.as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.kind.as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.title.as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.status.as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.confidence.to_le_bytes());
        hasher.update(b"\0");
        hasher.update(meta.tags.join("\u{1f}").as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.source_agents.join("\u{1f}").as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.links.join("\u{1f}").as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.supersedes.join("\u{1f}").as_bytes());
        hasher.update(b"\0");
        hasher.update(meta.superseded_by.clone().unwrap_or_default().as_bytes());
        hasher.update(b"\0");
        hasher.update(self.body.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

pub fn validate_meta(meta: &EntryMeta) -> Result<()> {
    if meta.id.trim().is_empty() || meta.title.trim().is_empty() {
        bail!("id and title are required");
    }
    if !(0.0..=1.0).contains(&meta.confidence) {
        bail!("confidence must be between 0 and 1");
    }
    if meta.decay_rate < 0.0 {
        bail!("decay_rate cannot be negative");
    }
    if !["active", "fading", "superseded", "archived"].contains(&meta.status.as_str()) {
        bail!("unsupported status '{}'", meta.status);
    }
    Ok(())
}

pub fn slugify(value: &str) -> String {
    let slug: String = value
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let parts: Vec<_> = slug.split('-').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        "entry".into()
    } else {
        parts.join("-")
    }
}
