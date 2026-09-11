//! Durable capture inbox. Source events are portable; queue state is local and
//! must be backed up (unlike the disposable search index). No model is invoked.
use crate::entry::{Entry, EntryMeta};
use crate::vault::Vault;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaptureInput {
    pub event_id: String,
    pub project: String,
    pub session_id: String,
    #[serde(default)]
    pub source_agent: String,
    pub title: String,
    pub context: String,
    pub action: String,
    pub outcome: String,
    #[serde(default)]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl CaptureInput {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("event_id", &self.event_id),
            ("project", &self.project),
            ("session_id", &self.session_id),
            ("source_agent", &self.source_agent),
            ("title", &self.title),
            ("context", &self.context),
            ("action", &self.action),
            ("outcome", &self.outcome),
        ] {
            ensure!(!value.trim().is_empty(), "{name} must not be empty");
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= 256 * 1024,
            "capture exceeds 256 KiB"
        );
        Ok(())
    }

    fn same_key(&self, other: &Self) -> bool {
        self.event_id == other.event_id
            && self.project == other.project
            && self.session_id == other.session_id
            && self.source_agent == other.source_agent
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceEvent {
    pub version: u32,
    pub id: Uuid,
    pub captured_at: DateTime<Utc>,
    pub input: CaptureInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeDraft {
    pub title: String,
    pub content: String,
    pub kind: String,
    pub confidence: f64,
    pub tags: Vec<String>,
}

impl KnowledgeDraft {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            !self.title.trim().is_empty() && !self.content.trim().is_empty(),
            "title and content must not be empty"
        );
        ensure!(
            ["knowledge", "lesson", "decision", "pattern"].contains(&self.kind.as_str()),
            "invalid knowledge kind"
        );
        ensure!(
            (0.0..=1.0).contains(&self.confidence),
            "confidence must be between 0 and 1"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Processing,
    NeedsReview,
    Accepted,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub knowledge: KnowledgeDraft,
    /// Suggestions only; tag/title overlap does not prove duplication.
    pub related_entries: Vec<String>,
    #[serde(default)]
    pub distillation: Option<crate::distillation::Trace>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub decision: Decision,
    pub reason: String,
    /// Optional fully reviewed replacement for the template draft.
    pub knowledge: Option<KnowledgeDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub version: u32,
    pub id: Uuid,
    pub status: Status,
    pub attempts: u32,
    pub updated_at: DateTime<Utc>,
    pub candidate: Option<Candidate>,
    pub review: Option<Review>,
    pub entry_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct WorkReport {
    pub prepared: usize,
    pub busy: bool,
    pub recovered: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}

fn queue_dir(vault: &Vault) -> PathBuf {
    vault.root.join(".relic/queue")
}
fn source_path(vault: &Vault, id: Uuid) -> PathBuf {
    vault.root.join(format!("sources/captures/{id}.json"))
}
fn state_path(vault: &Vault, id: Uuid) -> PathBuf {
    queue_dir(vault).join(format!("{id}.json"))
}
fn entry_id(id: Uuid) -> String {
    format!("relic-capture-{id}")
}

/// OS lock is released on process exit, including crashes. Keep the inode;
/// unlinking a lock file could let two processes lock different inodes.
fn lock(vault: &Vault) -> Result<File> {
    let dir = queue_dir(vault);
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    File::open(vault.root.join(".relic"))?.sync_all()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("worker.lock"))?;
    file.lock()?;
    // Also protects pre-existing vaults whose root .gitignore predates capture.
    atomic_write(&dir.join(".gitignore"), b"*\n")?;
    fs::create_dir_all(vault.root.join("sources/captures"))?;
    #[cfg(unix)]
    File::open(vault.root.join("sources"))?.sync_all()?;
    atomic_write(&vault.root.join("sources/captures/.gitignore"), b".*.tmp\n")?;
    Ok(file)
}

/// Acknowledgements follow file fsync + rename + parent directory fsync.
pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent directory")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn save(vault: &Vault, item: &mut QueueItem) -> Result<()> {
    item.updated_at = Utc::now();
    atomic_write(
        &state_path(vault, item.id),
        &serde_json::to_vec_pretty(item)?,
    )
}

fn read_source(vault: &Vault, id: Uuid) -> Result<SourceEvent> {
    let event: SourceEvent = serde_json::from_slice(&fs::read(source_path(vault, id))?)?;
    ensure!(
        event.version == 1 && event.id == id,
        "invalid source version or ID"
    );
    event.input.validate()?;
    Ok(event)
}

fn read_state(vault: &Vault, id: Uuid) -> Result<QueueItem> {
    let path = state_path(vault, id);
    if path.exists() {
        let item: QueueItem = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(
            item.version == 1 && item.id == id,
            "invalid queue version or ID"
        );
        return Ok(item);
    }
    // Covers a crash after source persistence but before queue persistence.
    read_source(vault, id)?;
    Ok(QueueItem {
        version: 1,
        id,
        status: Status::Pending,
        attempts: 0,
        updated_at: Utc::now(),
        candidate: None,
        review: None,
        entry_id: None,
        last_error: None,
    })
}

fn source_ids(vault: &Vault) -> Result<Vec<Uuid>> {
    let directory = vault.root.join("sources/captures");
    if !directory.exists() {
        return Ok(vec![]);
    }
    let mut ids = Vec::new();
    for file in fs::read_dir(directory)? {
        let path = file?.path();
        if path.extension().is_some_and(|e| e == "json") {
            ids.push(Uuid::parse_str(
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .context("invalid source filename")?,
            )?);
        }
    }
    ids.sort();
    Ok(ids)
}

pub fn capture(vault: &Vault, input: CaptureInput) -> Result<QueueItem> {
    input.validate()?;
    let _lock = lock(vault)?;
    capture_locked(vault, input)
}

fn capture_locked(vault: &Vault, input: CaptureInput) -> Result<QueueItem> {
    for id in source_ids(vault)? {
        let event = read_source(vault, id)?;
        // A late Stop for a turn already imported manually must not re-capture it.
        if input.tags.iter().any(|t| t == "automatic-capture")
            && event.input.tags.iter().any(|t| t == "manual-history")
            && event.input.source_agent == input.source_agent
            && event.input.session_id == input.session_id
            && (event
                .input
                .evidence
                .contains(&format!("history-turn:{}", input.event_id))
                || event
                    .input
                    .evidence
                    .contains(&crate::history::pair_key(&input.context, &input.outcome)))
        {
            return read_state(vault, id);
        }
        if event.input.same_key(&input) {
            ensure!(
                event.input == input,
                "event key already exists with different content; use a new event_id"
            );
            let mut item = read_state(vault, id)?;
            save(vault, &mut item)?;
            return Ok(item);
        }
    }
    let event = SourceEvent {
        version: 1,
        id: Uuid::new_v4(),
        captured_at: Utc::now(),
        input,
    };
    atomic_write(
        &source_path(vault, event.id),
        &serde_json::to_vec_pretty(&event)?,
    )?;
    let mut item = read_state(vault, event.id)?;
    save(vault, &mut item)?;
    Ok(item)
}

/// Session-level exclusion and insertion share the live-capture lock.
/// This covers pending, rejected and failed sources as well as accepted ones.
pub fn capture_history(vault: &Vault, input: CaptureInput) -> Result<QueueItem> {
    input.validate()?;
    ensure!(
        input.tags.iter().any(|t| t == "manual-history"),
        "not a history capture"
    );
    let _lock = lock(vault)?;
    for id in source_ids(vault)? {
        let source = read_source(vault, id)?;
        if source.input.source_agent == input.source_agent
            && source.input.session_id == input.session_id
        {
            // Same snapshot retry is safe even if the first response was lost.
            if source.input == input {
                return read_state(vault, id);
            }
            bail!("此会话已有采集记录；已阻止重复导入，请刷新列表");
        }
    }
    ensure!(
        !vault
            .entries()?
            .iter()
            .any(|e| crate::history::has_session(&e.body, &input.session_id)),
        "此会话已关联正式记忆，不能重复导入"
    );
    capture_locked(vault, input)
}

pub fn get(vault: &Vault, id: Uuid) -> Result<(SourceEvent, QueueItem)> {
    Ok((read_source(vault, id)?, read_state(vault, id)?))
}

pub fn list(vault: &Vault) -> Result<Vec<QueueItem>> {
    source_ids(vault)?
        .into_iter()
        .map(|id| read_state(vault, id))
        .collect()
}

fn prepare(vault: &Vault, event: &SourceEvent) -> Result<Candidate> {
    let input = &event.input;
    let related_entries = vault
        .entries()?
        .into_iter()
        .filter(|entry| entry.meta.status != "superseded" && entry.meta.status != "archived")
        .filter(|entry| {
            entry.meta.title.to_lowercase() == input.title.to_lowercase()
                || input.tags.iter().any(|tag| entry.meta.tags.contains(tag))
        })
        .take(20)
        .map(|entry| entry.meta.id)
        .collect();
    let evidence = if input.tags.iter().any(|t| t == "manual-history") {
        "- Historical transcript only; no independently verified evidence supplied.".into()
    } else if input.evidence.is_empty() {
        "- No evidence supplied.".into()
    } else {
        input
            .evidence
            .iter()
            .map(|e| format!("- {e}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(Candidate {
        knowledge: KnowledgeDraft {
            title: input.title.clone(),
            content: format!(
                "## Context\n{}\n\n## Action\n{}\n\n## Outcome\n{}\n\n## Reported evidence (not independently verified)\n{}",
                input.context, input.action, input.outcome, evidence
            ),
            kind: "lesson".into(),
            confidence: 0.5,
            tags: input.tags.clone(),
        },
        related_entries,
        distillation: None,
    })
}

/// One bounded pass. Failed events require explicit retry; a broken event must
/// not prevent unrelated captures from progressing. Interrupted work retries
/// under the process lock. Accepted/rejected work is never automatically redone.
pub fn work(vault: &Vault, limit: usize) -> Result<WorkReport> {
    work_selected(vault, limit, None)
}

/// Process one named capture without touching unrelated backlog.
pub fn work_one(vault: &Vault, id: Uuid) -> Result<WorkReport> {
    read_source(vault, id)?;
    work_selected(vault, 1, Some(id))
}

fn work_selected(vault: &Vault, limit: usize, selected: Option<Uuid>) -> Result<WorkReport> {
    ensure!(
        (1..=1000).contains(&limit),
        "limit must be between 1 and 1000"
    );
    // Serialize processors, but release the short queue lock during model work.
    // OS release on exit makes Processing recoverable without a stale lease timer.
    let processor = {
        let _queue = lock(vault)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(queue_dir(vault).join("processor.lock"))?
    };
    match processor.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Ok(WorkReport {
                busy: true,
                ..Default::default()
            });
        }
        Err(error) => return Err(error.into()),
    }
    let mut report = WorkReport::default();
    let started = std::time::Instant::now();
    let mut processed = 0;
    let ids = match selected {
        Some(id) => vec![id],
        None => source_ids(vault)?,
    };
    for id in ids {
        if started.elapsed() >= std::time::Duration::from_secs(30) {
            break;
        }
        let queue = lock(vault)?;
        let mut item = match read_state(vault, id) {
            Ok(item) => item,
            Err(error) => {
                report.failed += 1;
                report.errors.push(format!("{id}: {error}"));
                continue;
            }
        };
        let unfinished_review = item.status == Status::NeedsReview && item.review.is_some();
        if !matches!(item.status, Status::Pending | Status::Processing) && !unfinished_review {
            continue;
        }
        if processed >= limit {
            break;
        }
        processed += 1;
        let interrupted = item.status == Status::Processing || unfinished_review;
        item.status = Status::Processing;
        item.attempts = item.attempts.saturating_add(1);
        save(vault, &mut item)?;
        drop(queue);
        let result = (|| -> Result<()> {
            let event = read_source(vault, id)?;
            if let Some(review) = &item.review {
                if review.decision == Decision::Accept {
                    publish(vault, &event, &item, review)?;
                    item.entry_id = Some(entry_id(id));
                    item.status = Status::Accepted;
                } else {
                    item.status = Status::Rejected;
                }
                report.recovered += 1;
            } else if let Some(existing) = vault
                .entries()?
                .iter()
                .find(|entry| entry.meta.id == entry_id(id))
            {
                ensure!(
                    existing
                        .meta
                        .links
                        .contains(&format!("sources/captures/{id}.json")),
                    "existing capture entry is missing matching provenance"
                );
                // An accepted source may arrive via sync without local queue state.
                vault.reindex()?;
                item.entry_id = Some(entry_id(id));
                item.status = Status::Accepted;
                report.recovered += 1;
            } else {
                let mut candidate = prepare(vault, &event)?;
                if let Some((knowledge, trace)) =
                    crate::distillation::run(vault, &event, &candidate.related_entries)?
                {
                    candidate.knowledge = knowledge;
                    candidate.distillation = Some(trace);
                }
                item.candidate = Some(candidate);
                item.status = Status::NeedsReview;
                report.prepared += 1;
                if interrupted {
                    report.recovered += 1;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => item.last_error = None,
            Err(error) => {
                item.status = Status::Failed;
                item.last_error = Some(error.to_string());
                report.failed += 1;
                report.errors.push(format!("{id}: {error}"));
            }
        }
        let _queue = lock(vault)?;
        save(vault, &mut item)?;
    }
    Ok(report)
}

/// Explicitly re-distill an unreviewed candidate after changing the backend.
pub fn redistill(vault: &Vault, id: Uuid) -> Result<QueueItem> {
    let _lock = lock(vault)?;
    let mut item = read_state(vault, id)?;
    ensure!(
        item.status == Status::NeedsReview && item.review.is_none(),
        "only unreviewed candidates can be redistilled"
    );
    item.status = Status::Pending;
    item.last_error = None;
    save(vault, &mut item)?;
    Ok(item)
}

pub fn retry(vault: &Vault, id: Uuid) -> Result<QueueItem> {
    let _lock = lock(vault)?;
    let mut item = read_state(vault, id)?;
    ensure!(
        matches!(item.status, Status::Failed | Status::Pending),
        "only failed captures can be retried"
    );
    item.status = Status::Pending;
    item.last_error = None;
    save(vault, &mut item)?;
    Ok(item)
}

pub fn review(vault: &Vault, id: Uuid, review: Review) -> Result<QueueItem> {
    ensure!(
        !review.reason.trim().is_empty(),
        "review reason must not be empty"
    );
    if let Some(knowledge) = &review.knowledge {
        knowledge.validate()?;
    }
    let _lock = lock(vault)?;
    let mut item = read_state(vault, id)?;
    // Persist the decision before publishing. A retry after a crash must resume
    // that exact decision instead of overwriting already published knowledge.
    if let Some(previous) = &item.review {
        ensure!(
            *previous == review,
            "capture already has a different review"
        );
    }
    if matches!(item.status, Status::Accepted | Status::Rejected) {
        ensure!(
            (item.status == Status::Accepted) == (review.decision == Decision::Accept),
            "capture already has a different decision"
        );
        if item.status == Status::Accepted {
            vault.get(&entry_id(id))?;
            vault.reindex()?;
        }
        return Ok(item);
    }
    ensure!(
        item.status == Status::NeedsReview,
        "capture must be processed before review"
    );
    let event = read_source(vault, id)?;
    item.review = Some(review.clone());
    save(vault, &mut item)?;
    if review.decision == Decision::Reject {
        item.status = Status::Rejected;
    } else {
        publish(vault, &event, &item, &review)?;
        item.entry_id = Some(entry_id(id));
        item.status = Status::Accepted;
    }
    save(vault, &mut item)?;
    Ok(item)
}

fn publish(vault: &Vault, event: &SourceEvent, item: &QueueItem, review: &Review) -> Result<()> {
    let id = entry_id(event.id);
    let existing = vault
        .entries()?
        .into_iter()
        .find(|entry| entry.meta.id == id);
    if let Some(existing) = &existing {
        ensure!(
            existing
                .meta
                .links
                .contains(&format!("sources/captures/{}.json", event.id)),
            "existing capture entry is missing matching provenance"
        );
    }
    if existing.is_none() {
        let draft = review
            .knowledge
            .as_ref()
            .or_else(|| item.candidate.as_ref().map(|c| &c.knowledge))
            .context("missing candidate knowledge")?;
        draft.validate()?;
        let folder = match draft.kind.as_str() {
            "decision" => "decisions",
            "pattern" => "patterns",
            _ => "entries/inbox",
        };
        let source = format!("sources/captures/{}.json", event.id);
        let now = Utc::now();
        let entry = Entry {
            meta: EntryMeta {
                id,
                kind: draft.kind.clone(),
                title: draft.title.clone(),
                status: "active".into(),
                confidence: draft.confidence,
                tags: draft.tags.clone(),
                source_agents: vec![event.input.source_agent.clone()],
                created: now,
                updated: now,
                last_verified: now,
                expires: None,
                supersedes: vec![],
                superseded_by: None,
                links: vec![source.clone()],
                decay_rate: 0.05,
            },
            body: format!(
                "# {}\n\n{}\n\n## Capture provenance\n- Source: {}\n- Project: {}\n- Session: {}\n- Event: {}\n- Review reason: {}",
                draft.title,
                draft.content,
                source,
                event.input.project,
                event.input.session_id,
                event.input.event_id,
                review.reason
            ),
            path: vault.root.join(format!("{folder}/capture-{}.md", event.id)),
        };
        if entry.path.exists() {
            bail!("capture destination already exists with a different entry ID");
        }
        atomic_write(&entry.path, entry.render()?.as_bytes())?;
    }
    // If indexing fails, the same deterministic entry is reused on retry.
    vault.reindex()?;
    Ok(())
}
