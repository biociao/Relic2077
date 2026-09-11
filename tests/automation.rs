use relic2077::{automation, capture, git, hooks, vault::Vault};
use serde_json::json;
use std::{fs, path::Path, process::Command};
use tempfile::tempdir;
fn run(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn identity(path: &Path) {
    run(path, &["config", "user.name", "Test"]);
    run(path, &["config", "user.email", "test@example.invalid"]);
}
#[test]
fn hooks_retrieve_capture_deduplicate_and_wait_for_review() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    vault
        .create(
            "Atomic writes",
            "Persist review before publication",
            "decision",
            vec![],
            0.9,
            "test",
        )
        .unwrap();
    let prompt = json!({"hook_event_name":"UserPromptSubmit","session_id":"s1","cwd":"/project","prompt":"atomic writes OR \" : ()","turn_id":"t1"});
    let result = hooks::handle(&vault, "codex", prompt).unwrap();
    assert!(
        result["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap()
            .contains("Atomic writes")
    );
    let stop = json!({"hook_event_name":"Stop","session_id":"s1","cwd":"/project","turn_id":"t1","last_assistant_message":"Verified recovery"});
    assert_eq!(
        hooks::handle(&vault, "codex", stop.clone()).unwrap(),
        json!({})
    );
    hooks::handle(&vault, "codex", stop).unwrap();
    assert_eq!(capture::list(&vault).unwrap().len(), 1);
    let state = automation::tick(&vault, false).unwrap();
    assert!(state.last_sync.is_none());
    assert_eq!(
        capture::list(&vault).unwrap()[0].status,
        capture::Status::NeedsReview
    );
    assert_eq!(vault.entries().unwrap().len(), 1);
    hooks::handle(&vault,"dsh",json!({"hook_event_name":"Stop","session_id":"s2","cwd":"/project","turn_id":"t2","last_assistant_message":null})).unwrap();
    assert_eq!(capture::list(&vault).unwrap().len(), 1);
    let status = automation::status(&vault).unwrap();
    assert_eq!(
        status["events"].as_array().unwrap().last().unwrap()["status"],
        "skipped_no_response"
    );
}
#[test]
fn installer_preserves_hooks_and_is_idempotent() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(&dir.path().join("vault space")).unwrap();
    let path = dir.path().join("hooks.json");
    fs::write(&path,r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo custom"}]}]},"extra":true}"#).unwrap();
    let before = fs::read(&path).unwrap();
    hooks::install(&path, &vault.root, Path::new("/bin/relic's tool"), true).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    let first = hooks::install(&path, &vault.root, Path::new("/bin/relic"), false).unwrap();
    let second = hooks::install(&path, &vault.root, Path::new("/bin/relic"), false).unwrap();
    assert_eq!(first, second);
    assert_eq!(second["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(second["extra"], true);
}
#[test]
fn worker_syncs_and_pauses_on_conflict_without_losing_either_version() {
    let dir = tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    run(&remote, &["init", "--bare"]);
    let vault = Vault::init(&dir.path().join("vault")).unwrap();
    run(&vault.root, &["init", "-b", "main"]);
    identity(&vault.root);
    let mut config = vault.config().unwrap();
    config.sync.mode = "auto".into();
    config.sync.remotes.push(relic2077::config::Remote {
        name: "origin".into(),
        url: remote.to_str().unwrap().into(),
    });
    fs::write(
        vault.root.join(".relic/config.yaml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    let entry = vault
        .create("Shared", "base", "knowledge", vec![], 0.7, "test")
        .unwrap();
    let first = automation::tick(&vault, false).unwrap();
    assert!(first.last_sync.is_none()); // debounce
    assert!(automation::tick(&vault, true).unwrap().last_sync.is_some());
    assert!(run(&vault.root, &["status", "--porcelain"]).is_empty());
    let other = dir.path().join("other");
    run(
        dir.path(),
        &[
            "clone",
            "--branch",
            "main",
            remote.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    identity(&other);
    let relative = entry.path.strip_prefix(&vault.root).unwrap();
    fs::write(
        other.join(relative),
        fs::read_to_string(other.join(relative))
            .unwrap()
            .replace("base", "remote version"),
    )
    .unwrap();
    run(&other, &["add", "-A"]);
    run(&other, &["commit", "-m", "remote"]);
    run(&other, &["push"]);
    fs::write(
        &entry.path,
        fs::read_to_string(&entry.path)
            .unwrap()
            .replace("base", "local version"),
    )
    .unwrap();
    let conflict = automation::tick(&vault, true).unwrap();
    assert!(conflict.paused, "{conflict:?}");
    assert!(fs::read_to_string(&entry.path).unwrap().contains("<<<<<<<"));
    let resumed = automation::tick(&vault, false).unwrap();
    assert!(resumed.paused);
    assert_eq!(resumed.failures, conflict.failures);
    let local = run(
        &vault.root,
        &["show", &format!("HEAD:{}", relative.display())],
    );
    assert!(local.contains("local version"));
    let remote_text = run(
        &vault.root,
        &["show", &format!("origin/main:{}", relative.display())],
    );
    assert!(remote_text.contains("remote version"));
}
#[test]
fn unattended_sync_rejects_parent_repository() {
    let dir = tempdir().unwrap();
    run(dir.path(), &["init"]);
    let vault = Vault::init(&dir.path().join("child")).unwrap();
    assert!(
        git::automation_fingerprint(&vault.root)
            .unwrap_err()
            .to_string()
            .contains("own Git")
    );
}

#[test]
fn failed_sync_backs_off_across_worker_runs_and_retry_resets_schedule() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    run(&vault.root, &["init", "-b", "main"]);
    identity(&vault.root);
    let mut config = vault.config().unwrap();
    config.sync.mode = "auto".into();
    fs::write(
        vault.root.join(".relic/config.yaml"),
        serde_yaml::to_string(&config).unwrap(),
    )
    .unwrap();
    let failed = automation::tick(&vault, true).unwrap();
    assert_eq!(failed.failures, 1);
    assert!(failed.last_error.unwrap().contains("configured"));
    let next = automation::tick(&vault, false).unwrap();
    assert_eq!(next.failures, 1);
    assert_eq!(next.next_sync, failed.next_sync);
    automation::retry_sync(&vault).unwrap();
    assert_eq!(automation::state(&vault).unwrap().next_sync, 0);
    assert_eq!(automation::tick(&vault, false).unwrap().failures, 2);
}
