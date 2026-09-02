use relic2077::git::sync;
use relic2077::vault::{EntryPatch, Vault};
use std::path::Path;
use std::process::Command;
use tempfile::tempdir;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {:?} failed: {}",
        dir,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn bare_remote(parent: &Path, name: &str) -> std::path::PathBuf {
    let path = parent.join(name);
    let output = Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success(), "bare init failed");
    path
}

fn init_repo(dir: &Path, remote: Option<&Path>) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.name", "Relic Test"]);
    git(dir, &["config", "user.email", "relic@test.local"]);
    if let Some(remote) = remote {
        git(dir, &["remote", "add", "origin", remote.to_str().unwrap()]);
    }
}

fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", message]);
}

#[test]
fn sync_commits_and_pushes_to_a_new_remote() {
    let work = tempdir().unwrap();
    let vault_dir = work.path().join("vault");
    Vault::init(&vault_dir).unwrap();
    let remote = bare_remote(work.path(), "remote.git");
    init_repo(&vault_dir, Some(&remote));

    let vault = Vault {
        root: vault_dir.clone(),
    };
    vault
        .create(
            "First memory",
            "Local knowledge.",
            "knowledge",
            vec![],
            0.8,
            "test",
        )
        .unwrap();

    let outcome = sync(&vault_dir, "origin", "main", "initial sync").unwrap();
    assert!(outcome.committed);
    assert!(outcome.pushed);
    assert_eq!(outcome.branch, "main");

    // The remote now has the branch.
    git(&vault_dir, &["rev-parse", "--verify", "origin/main"]);
}

#[test]
fn sync_pulls_remote_changes_into_a_clone() {
    let work = tempdir().unwrap();
    let remote = bare_remote(work.path(), "remote.git");

    // Author: a base vault pushed to the remote.
    let author = work.path().join("author");
    Vault::init(&author).unwrap();
    init_repo(&author, Some(&remote));
    commit_all(&author, "init");
    git(&author, &["push", "-u", "origin", "main"]);

    // Peer clones the base (before the author's next change).
    let peer = work.path().join("peer");
    let output = Command::new("git")
        .arg("-C")
        .arg(work.path())
        .args(["clone", remote.to_str().unwrap(), peer.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(&peer, &["config", "user.name", "Relic Peer"]);
    git(&peer, &["config", "user.email", "peer@test.local"]);

    // Author adds an entry and pushes it after the peer cloned.
    let author_vault = Vault {
        root: author.clone(),
    };
    let authored = author_vault
        .create(
            "Shared memory",
            "From the author.",
            "knowledge",
            vec![],
            0.9,
            "author",
        )
        .unwrap();
    commit_all(&author, "add shared memory");
    git(&author, &["push", "origin", "main"]);

    // Peer syncs and receives the author's entry.
    let outcome = sync(&peer, "origin", "main", "pull peer").unwrap();
    assert!(outcome.pulled);
    assert!(outcome.resolved.is_empty());

    let peer_vault = Vault::discover(&peer).unwrap();
    assert!(peer_vault.get(&authored.meta.id).is_ok());
}

#[test]
fn sync_resolves_conflicts_preserving_the_local_version() {
    let work = tempdir().unwrap();
    let remote = bare_remote(work.path(), "remote.git");

    // Author pushes a base vault with one entry.
    let author = work.path().join("author");
    Vault::init(&author).unwrap();
    init_repo(&author, Some(&remote));
    commit_all(&author, "init");
    git(&author, &["push", "-u", "origin", "main"]);
    let author_vault = Vault {
        root: author.clone(),
    };
    let entry = author_vault
        .create(
            "Contested",
            "Base content.",
            "knowledge",
            vec![],
            0.8,
            "author",
        )
        .unwrap();
    let entry_path = entry.path.clone();
    commit_all(&author, "add base entry");
    git(&author, &["push", "origin", "main"]);

    // Peer clones the base.
    let peer = work.path().join("peer");
    let output = Command::new("git")
        .arg("-C")
        .arg(work.path())
        .args(["clone", remote.to_str().unwrap(), peer.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(output.status.success());
    git(&peer, &["config", "user.name", "Relic Peer"]);
    git(&peer, &["config", "user.email", "peer@test.local"]);

    // Author edits the same entry and pushes.
    author_vault
        .update(
            &entry.meta.id,
            EntryPatch {
                content: Some("Author version.".into()),
                ..Default::default()
            },
        )
        .unwrap();
    commit_all(&author, "author edit");
    git(&author, &["push", "origin", "main"]);

    // Peer edits the same entry locally (diverged), then syncs.
    let peer_vault = Vault::discover(&peer).unwrap();
    peer_vault
        .update(
            &entry.meta.id,
            EntryPatch {
                content: Some("Peer version.".into()),
                ..Default::default()
            },
        )
        .unwrap();
    commit_all(&peer, "peer edit");

    let outcome = sync(&peer, "origin", "main", "pull peer").unwrap();
    assert!(outcome.pulled);
    assert_eq!(outcome.resolved.len(), 1);

    // Remote (author) wins as the canonical entry, peer version preserved.
    let canonical = peer_vault.get(&entry.meta.id).unwrap();
    assert!(canonical.body.contains("Author version."));
    let saved = peer.join(".relic").join("conflicts");
    assert!(saved.exists());
    let conflicts = std::fs::read_dir(&saved)
        .unwrap()
        .flat_map(|e| e.ok())
        .collect::<Vec<_>>();
    assert_eq!(conflicts.len(), 1);
    let entry_file_name = entry_path.file_name().unwrap().to_string_lossy();
    let preserved = read_conflict_file(&saved, &entry_file_name);
    assert!(preserved.contains("Peer version."));
}

fn read_conflict_file(conflicts_dir: &Path, entry_file_name: &str) -> String {
    let mut found = String::new();
    for dir in conflicts_dir.read_dir().unwrap().flatten() {
        let result = walk_conflicts(&dir.path(), entry_file_name);
        if let Some(content) = result {
            found = content;
            break;
        }
    }
    found
}

fn walk_conflicts(dir: &Path, entry_file_name: &str) -> Option<String> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(content) = walk_conflicts(&path, entry_file_name) {
                return Some(content);
            }
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(entry_file_name))
        {
            return std::fs::read_to_string(&path).ok();
        }
    }
    None
}
