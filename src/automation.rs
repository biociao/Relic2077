//! Local worker state and unattended synchronization. Never resolves Git conflicts.
use crate::{capture, vault::Vault};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub(crate) fn lock(vault: &Vault, name: &str) -> Result<File> {
    let dir = vault.root.join(".relic/automation");
    fs::create_dir_all(&dir)?;
    capture::atomic_write(&dir.join(".gitignore"), b"*\n")?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(name))?;
    f.lock()?;
    Ok(f)
}
pub(crate) fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    capture::atomic_write(path, &serde_json::to_vec_pretty(value)?)
}
pub(crate) fn read<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid state {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.into()),
    }
}
#[derive(Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerState {
    pub heartbeat: Option<i64>,
    pub last_sync: Option<i64>,
    pub next_sync: i64,
    pub failures: u32,
    pub paused: bool,
    pub last_error: Option<String>,
    pub queue_error: Option<String>,
    pub dirty_since: Option<i64>,
    pub changed_at: Option<i64>,
    pub fingerprint: String,
}
pub fn state(vault: &Vault) -> Result<WorkerState> {
    read(&vault.root.join(".relic/automation/worker.json"))
}

/// Poll every five seconds. Persist debounce and exponential backoff across restarts.
/// An OS lock also serializes UI retry with another worker process.
pub fn tick(vault: &Vault, retry: bool) -> Result<WorkerState> {
    let _lock = lock(vault, "worker.lock")?;
    let mut s = state(vault)?;
    let now = Utc::now().timestamp();
    s.heartbeat = Some(now);
    s.queue_error = match capture::work(vault, 100) {
        Ok(report) if report.failed == 0 => None,
        Ok(report) => Some(report.errors.join("\n")),
        Err(e) => Some(format!("{e:#}")),
    };
    let config = vault.config()?;
    if retry {
        s.paused = false;
        s.next_sync = 0;
    }
    if config.sync.mode == "auto" && !s.paused && (s.failures == 0 || now >= s.next_sync) {
        let result = (|| -> Result<()> {
            let fingerprint = crate::git::automation_fingerprint(&vault.root)?;
            if fingerprint != s.fingerprint {
                s.fingerprint = fingerprint;
                s.changed_at = Some(now);
                s.dirty_since.get_or_insert(now);
            }
            let settled = s.changed_at.is_none_or(|t| now - t >= 15)
                || s.dirty_since.is_some_and(|t| now - t >= 60);
            if (now < s.next_sync && s.dirty_since.is_none()) || (!retry && !settled) {
                return Ok(());
            }
            let remote = config
                .sync
                .remotes
                .first()
                .context("Automatic sync requires a configured sync.remotes entry")?;
            crate::git::sync_unattended(&vault.root, &remote.name, &remote.url)?;
            s.last_sync = Some(now);
            s.next_sync = now + 60;
            s.failures = 0;
            s.last_error = None;
            s.dirty_since = None;
            s.changed_at = None;
            s.fingerprint = crate::git::automation_fingerprint(&vault.root)?;
            Ok(())
        })();
        if let Err(e) = result {
            s.paused = e.downcast_ref::<crate::git::SyncConflict>().is_some();
            s.failures = s.failures.saturating_add(1);
            s.next_sync = now + (15_i64 * 2_i64.pow(s.failures.min(6))).min(900);
            s.last_error = Some(format!("{e:#}"));
        }
    }
    save(&vault.root.join(".relic/automation/worker.json"), &s)?;
    Ok(s)
}
pub fn status(vault: &Vault) -> Result<serde_json::Value> {
    let s = state(vault)?;
    let online = s
        .heartbeat
        .is_some_and(|t| Utc::now().timestamp() - t < 120);
    let queue = capture::list(vault)?
        .into_iter()
        .map(|item| -> Result<serde_json::Value> {
            let (source, _) = capture::get(vault, item.id)?;
            let mut value = serde_json::to_value(item)?;
            value["origin"] =
                serde_json::json!(if source.input.tags.iter().any(|t| t == "manual-history") {
                    "manual_history"
                } else if source.input.tags.iter().any(|t| t == "automatic-capture") {
                    "automatic"
                } else {
                    "structured"
                });
            value["session_id"] = serde_json::json!(source.input.session_id);
            value["source_agent"] = serde_json::json!(source.input.source_agent);
            value["project"] = serde_json::json!(source.input.project);
            Ok(value)
        })
        .collect::<Result<Vec<_>>>()?;
    let events: Vec<serde_json::Value> = read(&vault.root.join(".relic/automation/events.json"))?;
    Ok(
        serde_json::json!({"setup":crate::setup::status(vault,&events)?,"worker":s,"online":online,"mode":vault.config()?.sync.mode,"queue":queue,"events":events}),
    )
}

/// Schedule another attempt; the independent worker performs network I/O.
pub fn retry_sync(vault: &Vault) -> Result<()> {
    let _lock = lock(vault, "worker.lock")?;
    let mut s = state(vault)?;
    s.paused = false;
    s.next_sync = 0;
    s.changed_at = None;
    save(&vault.root.join(".relic/automation/worker.json"), &s)
}

pub fn lease(vault: &Vault) -> Result<File> {
    let dir = vault.root.join(".relic/automation");
    fs::create_dir_all(&dir)?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("daemon.lock"))?;
    f.try_lock()
        .context("A daemon is already running for this vault")?;
    Ok(f)
}
pub fn running(vault: &Vault) -> Result<bool> {
    let path = vault.root.join(".relic/automation/daemon.lock");
    if !path.exists() {
        return Ok(false);
    }
    let f = OpenOptions::new().read(true).write(true).open(path)?;
    match f.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(e) => Err(anyhow::anyhow!(e)),
    }
}
pub fn stop_generation(vault: &Vault) -> Option<String> {
    fs::read_to_string(vault.root.join(".relic/automation/stop")).ok()
}
pub fn stop_requested(vault: &Vault, initial: &Option<String>) -> bool {
    let current = stop_generation(vault);
    current.is_some() && &current != initial
}
pub fn start(vault: &Vault, executable: &Path) -> Result<()> {
    use std::process::{Command, Stdio};
    let _guard = lock(vault, "control.lock")?;
    if running(vault)? {
        return Ok(());
    }
    let _ = fs::remove_file(vault.root.join(".relic/automation/stop"));
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(vault.root.join(".relic/automation/daemon.log"))?;
    let mut cmd = Command::new(executable);
    cmd.args(["daemon", "--vault"])
        .arg(fs::canonicalize(&vault.root)?)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd.spawn()?;
    for _ in 0..30 {
        if running(vault)? {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("Worker exited {status}; inspect .relic/automation/daemon.log");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    anyhow::bail!("Worker startup not confirmed; inspect .relic/automation/daemon.log")
}
pub fn stop(vault: &Vault) -> Result<()> {
    let _guard = lock(vault, "control.lock")?;
    fs::write(
        vault.root.join(".relic/automation/stop"),
        uuid::Uuid::new_v4().to_string(),
    )?;
    Ok(())
}
