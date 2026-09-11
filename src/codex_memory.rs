//! Import memories that a host agent already distilled for itself.
//!
//! Codex maintains its own two-stage memory product under `$CODEX_HOME/memories`
//! (`MEMORY.md`, `raw_memories.md`, `rollout_summaries/`, `memories_1.sqlite`).
//! `MEMORY.md` is the phase-2 synthesis: task groups carrying `scope` /
//! `applies_to`, each holding bullet points grouped by user preference, reusable
//! knowledge and failure lessons. Those bullets are the distilled memory points
//! that Relic's own template backend cannot produce.
//!
//! This importer never calls a model. It transports sentences a model has
//! already written, so entries are tagged `unverified` and kept below the
//! verified confidence band. Every import is explicit: nothing here is reached
//! from a hook, the daemon, a timer, or a page load.

use crate::{
    entry::{Entry, EntryMeta},
    vault::Vault,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

/// DSH has no equivalent store, so this importer is Codex-only by construction.
const HOST: &str = "codex";
const SOURCE_AGENT: &str = "codex-memory";
const SOURCE_TAG: &str = "codex-memory";
const UNVERIFIED_TAG: &str = "unverified";
/// Model-written, cross-session synthesis nobody has re-checked: importable and
/// searchable, but deliberately short of the verified confidence band.
const DEFAULT_CONFIDENCE: f64 = 0.6;
const MAX_TITLE_BYTES: usize = 240;
/// Titles carry the memory point itself, then stop: a 240-byte title is
/// unreadable in a list, and the full text always stays in the body.
const TITLE_TEXT_BYTES: usize = 110;
const MAX_BULLETS: usize = 5000;
const MAX_BULLET_BYTES: usize = 16 * 1024;
const MAX_MEMORY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// Path to `MEMORY.md`. Omitted means `$CODEX_HOME/memories/MEMORY.md`.
    #[serde(default)]
    pub source: Option<PathBuf>,
    /// Refresh stored text when a source bullet changed but the vault has stale
    /// content for the same identity. Off by default: imports are append-only in
    /// spirit, and re-running after a Codex rewrite creates new entries instead.
    #[serde(default)]
    pub update: bool,
    /// Report what would be imported without writing anything.
    #[serde(default)]
    pub dry_run: bool,
    /// Skip every task group whose name contains one of these strings. Codex
    /// also distils this project's own history, so importing it back would
    /// duplicate knowledge the vault already holds.
    #[serde(default)]
    pub exclude_task_groups: Vec<String>,
    pub confidence: Option<f64>,
}

/// One memory point: a single bullet under a task-group section.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct MemoryPoint {
    pub task_group: String,
    pub scope: String,
    pub applies_to: String,
    pub task: String,
    pub section: String,
    pub text: String,
    pub kind: String,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub source: String,
    pub confidence: f64,
    pub parsed: usize,
    pub excluded: usize,
    pub created: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub entries: Vec<String>,
    pub sample: Vec<MemoryPoint>,
}

/// Codex writes English section headings, sometimes with a leading noun
/// ("User preferences", "General Tips"), so the subject is matched anywhere in
/// the heading. `rollout_summary_files` and `keywords` are pointers and
/// metadata, not reusable knowledge, and stay out.
fn importable_section(heading: &str) -> bool {
    let heading = heading.trim().to_ascii_lowercase();
    [
        "preference",
        "reusable knowledge",
        "failure",
        "general tip",
        "key step",
        "lesson",
    ]
    .iter()
    .any(|name| heading.contains(name))
}

/// Failure lessons are the one group that maps onto a distinct Relic kind.
fn kind_for(section: &str) -> &'static str {
    if section.trim().to_ascii_lowercase().starts_with("failure") {
        "lesson"
    } else {
        "knowledge"
    }
}

/// Task-group and section context carried alongside each bullet.
#[derive(Default, Clone)]
struct Section {
    group: String,
    scope: String,
    applies_to: String,
    task: String,
    section: String,
}

struct Collector {
    context: Section,
    seen: BTreeSet<(String, String, String, String)>,
    points: Vec<MemoryPoint>,
}

impl Collector {
    /// Close the pending bullet. Codex wraps long bullets, so the lines are
    /// rejoined before the point is measured, deduplicated and stored.
    fn flush(&mut self, bullet: &mut Option<String>) {
        let Some(text) = bullet.take() else { return };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let context = &self.context;
        if text.is_empty() || context.group.is_empty() || context.section.is_empty() {
            return;
        }
        if text.len() > MAX_BULLET_BYTES || self.points.len() >= MAX_BULLETS {
            return;
        }
        let key = (
            context.group.clone(),
            context.task.clone(),
            context.section.clone(),
            text.clone(),
        );
        if !self.seen.insert(key) {
            return;
        }
        self.points.push(MemoryPoint {
            task_group: context.group.clone(),
            scope: context.scope.clone(),
            applies_to: context.applies_to.clone(),
            task: context.task.clone(),
            section: context.section.clone(),
            kind: kind_for(&context.section).to_owned(),
            text,
        });
    }

    fn heading(&mut self, line: &str, bullet: &mut Option<String>) {
        self.flush(bullet);
        let heading = line.trim_start_matches('#').trim();
        if line.starts_with("# ") {
            // A top-level heading starts a new task group.
            self.context = Section {
                group: heading
                    .strip_prefix("Task Group:")
                    .unwrap_or(heading)
                    .trim()
                    .to_owned(),
                ..Section::default()
            };
        } else if line.starts_with("## ") {
            if let Some(name) = heading.strip_prefix("Task ") {
                // `## Task 2: ...` opens a task and resets section context.
                self.context.task = name.trim().to_owned();
                self.context.section.clear();
            } else {
                // `## User preferences` and friends are task-group sections.
                self.context.section = heading.to_owned();
                self.context.task.clear();
            }
        } else {
            // `### rollout_summary_files`, `### keywords`, `### References`.
            self.context.section = heading.to_owned();
        }
    }
}

/// Walk `MEMORY.md` and return every importable bullet with the context needed
/// to render a self-contained entry. Task-group state is captured because
/// `applies_to` carries the reuse boundary that makes a memory trustworthy.
pub fn parse(input: &str) -> Vec<MemoryPoint> {
    let mut collector = Collector {
        context: Section::default(),
        seen: BTreeSet::new(),
        points: Vec::new(),
    };
    let mut bullet: Option<String> = None;
    for raw in input.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('#') {
            collector.heading(trimmed, &mut bullet);
            continue;
        }
        if collector.context.group.is_empty() {
            continue;
        }
        if bullet.is_none() {
            if let Some(value) = trimmed.strip_prefix("scope:") {
                collector.context.scope = value.trim().to_owned();
                continue;
            }
            if let Some(value) = trimmed.strip_prefix("applies_to:") {
                collector.context.applies_to = value.trim().to_owned();
                continue;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            collector.flush(&mut bullet);
            if importable_section(&collector.context.section) {
                bullet = Some(rest.trim().to_owned());
            }
            continue;
        }
        // Continuation of the previous bullet: nested detail or wrapped text.
        if let Some(text) = &mut bullet
            && !trimmed.is_empty()
        {
            text.push(' ');
            text.push_str(trimmed);
        }
    }
    collector.flush(&mut bullet);
    collector.points
}

/// Deterministic identity: re-importing unchanged memories is a no-op, while
/// identical text under two task groups still yields two distinct entries.
pub fn memory_id(point: &MemoryPoint) -> String {
    let bytes = serde_json::to_vec(&(
        HOST,
        &point.task_group,
        &point.task,
        &point.section,
        &point.text,
    ))
    .expect("strings serialize");
    format!("relic-codex-memory-{:x}", Sha256::digest(bytes))
}

/// Truncate on a character boundary so a multi-byte title never splits a glyph.
fn truncate_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

fn title(point: &MemoryPoint) -> String {
    let text = point.text.trim().replace('`', "");
    let mut title = format!("{} · {}", point.task_group, point.section);
    if !text.is_empty() {
        title.push_str(" · ");
        title.push_str(&truncate_bytes(&text, TITLE_TEXT_BYTES));
        if text.len() > TITLE_TEXT_BYTES {
            title.push('…');
        }
    }
    truncate_bytes(&title, MAX_TITLE_BYTES)
}

fn render_body(point: &MemoryPoint) -> String {
    let mut body = String::new();
    if !point.scope.is_empty() {
        body.push_str(&format!("**Scope**: {}\n\n", point.scope));
    }
    if !point.applies_to.is_empty() {
        body.push_str(&format!("**Applies to**: {}\n\n", point.applies_to));
    }
    if !point.task.is_empty() {
        body.push_str(&format!("**Source task**: {}\n\n", point.task));
    }
    body.push_str(&point.text);
    body
}

fn entry_for(
    vault: &Vault,
    point: &MemoryPoint,
    id: &str,
    confidence: f64,
    existing: Option<&Entry>,
) -> Result<Entry> {
    let now = chrono::Utc::now();
    let path = vault
        .root
        .join("entries/inbox")
        .join(format!("codex-memory-{}.md", &id[id.len() - 12..]));
    let meta = match existing {
        Some(entry) => EntryMeta {
            confidence,
            tags: vec![SOURCE_TAG.into(), UNVERIFIED_TAG.into()],
            updated: now,
            last_verified: now,
            ..entry.meta.clone()
        },
        None => EntryMeta {
            id: id.into(),
            kind: point.kind.clone(),
            title: title(point),
            status: "active".into(),
            confidence,
            tags: vec![SOURCE_TAG.into(), UNVERIFIED_TAG.into()],
            source_agents: vec![SOURCE_AGENT.into()],
            created: now,
            updated: now,
            last_verified: now,
            expires: None,
            supersedes: vec![],
            superseded_by: None,
            links: vec![format!("codex-memory:{HOST}:{}", point.task_group)],
            decay_rate: vault.config()?.vault.default_decay_rate,
        },
    };
    Ok(Entry {
        meta,
        body: render_body(point),
        path: existing.map_or(path, |entry| entry.path.clone()),
    })
}

/// Case-insensitive substring match against the task-group name.
fn excluded_group(group: &str, patterns: &[String]) -> bool {
    let group = group.to_lowercase();
    patterns
        .iter()
        .any(|pattern| !pattern.trim().is_empty() && group.contains(&pattern.trim().to_lowercase()))
}

/// Resolve the default source: Codex's phase-2 synthesis for this device.
pub fn default_source() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("memories")
        .join("MEMORY.md")
}

pub fn import(vault: &Vault, request: ImportRequest) -> Result<ImportReport> {
    let source = request.source.clone().unwrap_or_else(default_source);
    ensure!(
        source.is_absolute(),
        "codex memory source must be an absolute path"
    );
    let metadata = fs::metadata(&source).with_context(|| {
        format!(
            "could not read Codex memories at {} (is Codex's memory feature enabled?)",
            source.display()
        )
    })?;
    ensure!(metadata.is_file(), "codex memory source is not a file");
    ensure!(
        metadata.len() <= MAX_MEMORY_BYTES,
        "codex memory file exceeds 64 MiB"
    );
    let confidence = request.confidence.unwrap_or(DEFAULT_CONFIDENCE);
    ensure!(
        (0.0..=1.0).contains(&confidence),
        "confidence must be between 0 and 1"
    );
    let raw = fs::read_to_string(&source).context("codex memory file is not valid UTF-8")?;
    let points = parse(&raw);
    let excluded: Vec<&MemoryPoint> = points
        .iter()
        .filter(|point| excluded_group(&point.task_group, &request.exclude_task_groups))
        .collect();
    let points: Vec<&MemoryPoint> = points
        .iter()
        .filter(|point| !excluded_group(&point.task_group, &request.exclude_task_groups))
        .collect();
    // Identity comes from the bullet text, not from Codex's grouping: a task
    // group disappears from MEMORY.md once superseded, and one bullet may be
    // echoed in several groups.
    let by_id: BTreeMap<String, Entry> = vault
        .entries()?
        .into_iter()
        .map(|entry| (entry.meta.id.clone(), entry))
        .collect();
    let mut report = ImportReport {
        source: source.to_string_lossy().into_owned(),
        confidence,
        parsed: points.len(),
        excluded: excluded.len(),
        ..Default::default()
    };
    for point in &points {
        if report.sample.len() < 10 {
            report.sample.push((*point).clone());
        }
        let id = memory_id(point);
        let existing = by_id.get(&id);
        match (existing, request.dry_run) {
            (Some(_), true) => report.skipped += 1,
            (None, true) => {
                report.created += 1;
                report.entries.push(id);
            }
            (Some(_), false) if !request.update => report.skipped += 1,
            (existing, false) => {
                let outcome =
                    entry_for(vault, point, &id, confidence, existing).and_then(|entry| {
                        // Never write through a path this importer does not own:
                        // a deterministic ID colliding with a foreign file stops
                        // the point instead of overwriting it.
                        if existing.is_none() && entry.path.exists() {
                            anyhow::bail!(
                                "destination {} already exists; refusing to overwrite",
                                entry.path.display()
                            );
                        }
                        crate::capture::atomic_write(&entry.path, entry.render()?.as_bytes())
                    });
                match outcome {
                    Ok(()) => {
                        if existing.is_some() {
                            report.updated += 1;
                        } else {
                            report.created += 1;
                        }
                        report.entries.push(id);
                    }
                    Err(error) => report.errors.push(format!("{id}: {error}")),
                }
            }
        }
    }
    if !request.dry_run && (report.created > 0 || report.updated > 0) {
        vault.reindex()?;
    }
    Ok(report)
}

impl ImportReport {
    pub fn value(&self) -> Value {
        json!({
            "source": self.source,
            "confidence": self.confidence,
            "parsed": self.parsed,
            "excluded": self.excluded,
            "created": self.created,
            "updated": self.updated,
            "skipped": self.skipped,
            "errors": self.errors,
            "entries": self.entries,
            "sample": self.sample,
        })
    }
}
