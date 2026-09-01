use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
