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
