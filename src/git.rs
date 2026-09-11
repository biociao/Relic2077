use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::fs;
use std::path::{Path, PathBuf};

use crate::vault::Vault;

/// Run `git` against the vault directory and return the trimmed standard
/// output. A non-zero exit is reported as an error carrying the stderr.
fn git(root: &Path, args: &[&str]) -> Result<String> {
    git_result(root, args).context(format!("git {} failed", args.join(" ")))
}

/// Run `git`, returning the trimmed stdout on success or the error envelope
/// (with stderr) on failure. Callers that need to distinguish an expected
/// non-zero exit (e.g. a conflicted merge) can inspect the returned error.
fn git_result(root: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| format!("failed to invoke git (args: {})", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        bail!(
            "git {} exited with {}: {}{}",
            args.join(" "),
            output.status,
            stderr,
            if stdout.is_empty() {
                String::new()
            } else {
                format!(" ({stdout})")
            }
        );
    }
}

/// Ensure the vault root is a git repository, initialising one if needed.
pub fn ensure_repository(root: &Path) -> Result<()> {
    if git_result(root, &["rev-parse", "--git-dir"]).is_err() {
        git(root, &["init"])?;
    }
    Ok(())
}

/// Add a remote by name and URL if it is not already configured.
pub fn ensure_remote(root: &Path, name: &str, url: &str) -> Result<()> {
    if git_result(root, &["remote", "get-url", name]).is_err() {
        git(root, &["remote", "add", name, url])?;
    }
    Ok(())
}

/// The currently checked-out branch name.
pub fn current_branch(root: &Path) -> Result<String> {
    git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
}

/// Make an initial commit when the repository has files but no commits yet, so
/// a freshly initialised vault can be pushed immediately.
pub fn ensure_initial_commit(root: &Path, message: &str) -> Result<bool> {
    let has_head = git_result(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    if has_head {
        return Ok(false);
    }
    git(root, &["add", "-A"])?;
    let staged = dirty(root)?;
    if !staged {
        return Ok(false);
    }
    git(root, &["commit", "-m", message])?;
    Ok(true)
}

/// True when the working tree has staged or unstaged changes (including
/// untracked, non-ignored files).
pub fn dirty(root: &Path) -> Result<bool> {
    Ok(!git(root, &["status", "--porcelain"])?.is_empty())
}

/// Stage every change and, if anything changed, create a commit. Returns the
/// working-tree state as a short snapshot string for reporting.
pub fn commit_all(root: &Path, message: &str) -> Result<()> {
    if !dirty(root)? {
        return Ok(());
    }
    git(root, &["add", "-A"])?;
    git(root, &["commit", "-m", message])?;
    Ok(())
}

/// Push the current branch to `remote`, creating the upstream tracking if this
/// is the first push.
pub fn push(root: &Path, remote: &str, branch: &str) -> Result<()> {
    git(root, &["push", "--set-upstream", remote, branch])?;
    Ok(())
}

/// True when the fetched remote branch exists locally.
fn remote_branch_exists(root: &Path, remote: &str, branch: &str) -> Result<bool> {
    Ok(git_result(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{remote}/{branch}"),
        ],
    )
    .is_ok())
}

fn unmerged_paths(root: &Path) -> Result<Vec<String>> {
    let output = git(root, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(output.lines().map(str::to_owned).collect())
}

/// Resolve merge conflicts deterministically: the remote (theirs) version
/// becomes the canonical file, while the local (ours) version is preserved
/// under `.relic/conflicts/<timestamp>/` so no knowledge is lost.
fn resolve_conflicts(root: &Path, paths: &[String]) -> Result<Vec<String>> {
    let conflicts = root
        .join(".relic")
        .join("conflicts")
        .join(Utc::now().format("%Y%m%d-%H%M%S").to_string());
    let mut preserved = Vec::new();
    for path in paths {
        let destination = conflict_path(&conflicts, path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let ours = git(root, &["show", &format!(":2:{path}")])?;
        fs::write(&destination, ours)?;
        // Take the remote version and clear the conflict marker.
        git(root, &["checkout", "--theirs", path])?;
        git(root, &["add", path])?;
        preserved.push(path.clone());
    }
    Ok(preserved)
}

fn conflict_path(conflicts: &Path, path: &str) -> PathBuf {
    let mut destination = conflicts.join(path);
    destination.set_extension(destination.extension().map_or_else(
        || "local".into(),
        |ext| format!("{}.local", ext.to_string_lossy()),
    ));
    destination
}

/// A record of one `relic sync` run.
#[derive(Debug, serde::Serialize)]
pub struct SyncOutcome {
    pub committed: bool,
    pub pulled: bool,
    pub resolved: Vec<String>,
    pub pushed: bool,
    pub head: String,
    pub remote: String,
    pub branch: String,
    pub message: String,
}

/// Commit local edits, pull the configured remote (resolving any conflicts in
/// favour of the remote while preserving local versions), then push. This is the
/// entry point for the `relic sync` subcommand.
pub fn sync(root: &Path, remote: &str, branch: &str, message: &str) -> Result<SyncOutcome> {
    let _lock = crate::automation::lock(
        &Vault {
            root: root.to_owned(),
        },
        "git.lock",
    )?;
    ensure_repository(root)?;
    let initial = ensure_initial_commit(root, message)?;
    let was_dirty = dirty(root)?;
    if was_dirty {
        commit_all(root, message)?;
    }
    let committed = initial || was_dirty;

    git(root, &["fetch", remote])?;
    let mut pulled = false;
    let mut resolved = Vec::new();
    if remote_branch_exists(root, remote, branch)? {
        let head_before = git(root, &["rev-parse", "--short", "HEAD"])?;
        let merge = git_result(root, &["merge", "--no-edit", &format!("{remote}/{branch}")]);
        match merge {
            Ok(_) => {}
            Err(merge_error) => {
                let conflicts = unmerged_paths(root)?;
                if conflicts.is_empty() {
                    return Err(merge_error);
                }
                resolved = resolve_conflicts(root, &conflicts)?;
                git(root, &["commit", "--no-edit"])?;
            }
        }
        let head_after = git(root, &["rev-parse", "--short", "HEAD"])?;
        pulled = head_after != head_before;
    }

    Vault {
        root: root.to_path_buf(),
    }
    .reindex()?;

    push(root, remote, branch)?;
    let head = git(root, &["rev-parse", "--short", "HEAD"])?;
    Ok(SyncOutcome {
        committed,
        pulled,
        resolved,
        pushed: true,
        head,
        remote: remote.to_string(),
        branch: branch.to_string(),
        message: message.to_string(),
    })
}

#[derive(Debug)]
pub struct SyncConflict;
impl std::fmt::Display for SyncConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Automatic sync paused: resolve the vault's Git conflict, commit the resolution, then retry"
        )
    }
}
impl std::error::Error for SyncConflict {}

fn automation_repository(root: &Path) -> Result<()> {
    ensure_repository(root)?;
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    anyhow::ensure!(
        fs::canonicalize(top)? == fs::canonicalize(root)?,
        "Vault must be its own Git repository for automatic sync"
    );
    Ok(())
}

/// Include file contents, not just Git's M marker, so continuous edits debounce.
pub fn automation_fingerprint(root: &Path) -> Result<String> {
    use std::hash::{Hash, Hasher};
    use std::io::Read;
    automation_repository(root)?;
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-c", "-o", "--exclude-standard", "-z"])
        .output()?;
    anyhow::ensure!(output.status.success(), "Cannot enumerate vault files");
    let mut paths: Vec<_> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    paths.sort();
    paths.dedup();
    let mut hash = std::hash::DefaultHasher::new();
    for bytes in paths {
        let path = std::str::from_utf8(bytes).context("Vault paths must be UTF-8")?;
        bytes.hash(&mut hash);
        let full = root.join(path);
        if !full.exists() {
            continue;
        }
        if full.symlink_metadata()?.file_type().is_symlink() {
            fs::read_link(full)?.hash(&mut hash);
        } else if full.is_file() {
            let mut file = fs::File::open(full)?;
            let mut buffer = [0u8; 16384];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.write(&buffer[..n]);
            }
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}

/// No prompts and a bounded child lifetime. File-backed output avoids pipe deadlocks.
fn unattended_git(root: &Path, args: &[&str]) -> Result<String> {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let dir = root.join(".relic/automation");
    fs::create_dir_all(&dir)?;
    let out = dir.join("git.stdout");
    let err = dir.join("git.stderr");
    let mut child = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=10")
        .stdin(Stdio::null())
        .stdout(fs::File::create(&out)?)
        .stderr(fs::File::create(&err)?)
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = fs::read_to_string(&out)?;
            let stderr = fs::read_to_string(&err)?;
            let _ = fs::remove_file(&out);
            let _ = fs::remove_file(&err);
            anyhow::ensure!(
                status.success(),
                "git {} failed: {}",
                args.first().unwrap_or(&""),
                stderr.trim()
            );
            return Ok(stdout.trim().to_owned());
        }
        if start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&out);
            let _ = fs::remove_file(&err);
            bail!("git operation timed out after 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Preserve conflicts in Git for explicit resolution; never choose ours/theirs.
pub fn sync_unattended(root: &Path, remote: &str, url: &str) -> Result<()> {
    let vault = Vault {
        root: root.to_owned(),
    };
    let _lock = crate::automation::lock(&vault, "git.lock")?;
    automation_repository(root)?;
    if !unmerged_paths(root)?.is_empty()
        || git_result(root, &["rev-parse", "--verify", "MERGE_HEAD"]).is_ok()
    {
        return Err(SyncConflict.into());
    }
    anyhow::ensure!(!remote.starts_with('-'), "Invalid remote name");
    ensure_remote(root, remote, url)?;
    anyhow::ensure!(
        git(root, &["remote", "get-url", remote])? == url,
        "Configured remote URL differs from Git; update it explicitly before automatic sync"
    );
    // Validate knowledge before publication; capture candidates remain review-only.
    vault.entries()?;
    unattended_git(root, &["add", "-A"])?;
    if dirty(root)? {
        unattended_git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "relic: automatic memory sync",
            ],
        )?;
    }
    let branch = current_branch(root)?;
    anyhow::ensure!(branch != "HEAD", "Automatic sync requires a named branch");
    unattended_git(root, &["fetch", remote])?;
    if remote_branch_exists(root, remote, &branch)?
        && let Err(e) = unattended_git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "merge",
                "--no-edit",
                &format!("{remote}/{branch}"),
            ],
        )
    {
        if !unmerged_paths(root)?.is_empty() {
            return Err(SyncConflict.into());
        }
        return Err(e);
    }
    vault.reindex()?;
    unattended_git(root, &["push", "--set-upstream", remote, &branch])?;
    Ok(())
}
