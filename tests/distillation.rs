use relic2077::{
    capture::{self, CaptureInput, Status},
    distillation::{self, Config},
    vault::Vault,
};
use serde_json::json;
use std::fs;
use tempfile::tempdir;

fn source() -> CaptureInput {
    CaptureInput {
        event_id: "e1".into(),
        project: "project".into(),
        session_id: "session".into(),
        source_agent: "test".into(),
        title: "Cache restart".into(),
        context: "Cache lost".into(),
        action: "Rebuild".into(),
        outcome: "Search recovered".into(),
        evidence: vec!["Regression test passed".into()],
        tags: vec!["cache".into()],
    }
}
fn answer() -> serde_json::Value {
    json!({"knowledge":{"title":"Recover lost cache","content":"Rebuild from Markdown when the cache is absent.","kind":"lesson","confidence":0.8,"tags":["cache"]},"recommendation":"new","rationale":"Reusable recovery procedure; evidence still needs review.","evidence_indices":[0],"related_entry_ids":[]})
}
#[test]
fn defaults_to_template_and_rejects_invalid_local_configuration() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    assert!(matches!(
        distillation::config(&vault).unwrap(),
        Config::Template
    ));
    let invalid = Config::Command {
        executable: "relative-command".into(),
        args: vec![],
        timeout_seconds: 1,
    };
    assert!(distillation::configure(&vault, &invalid).is_err());
    // Synced YAML cannot enable arbitrary command execution.
    fs::write(
        dir.path().join(".relic/config.yaml"),
        "version: 1\ndistillation:\n  backend: command\n  executable: /bin/false\n",
    )
    .unwrap();
    let item = capture::capture(&vault, source()).unwrap();
    capture::work(&vault, 1).unwrap();
    assert!(
        capture::get(&vault, item.id)
            .unwrap()
            .1
            .candidate
            .unwrap()
            .distillation
            .is_none()
    );
}

#[cfg(unix)]
fn adapter(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("adapter.sh");
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}
#[cfg(unix)]
fn enable(vault: &Vault, path: std::path::PathBuf) {
    distillation::configure(
        vault,
        &Config::Command {
            executable: path,
            args: vec![],
            timeout_seconds: 5,
        },
    )
    .unwrap();
}
#[cfg(unix)]
fn print_answer(value: &serde_json::Value) -> String {
    format!("cat <<'RELIC_JSON'\n{value}\nRELIC_JSON")
}

#[cfg(unix)]
#[test]
fn command_receives_bounded_context_and_returns_a_review_only_draft() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let related = vault
        .create(
            "Cache notes",
            "An earlier cache finding",
            "knowledge",
            vec!["cache".into()],
            0.7,
            "test",
        )
        .unwrap();
    let request = dir.path().join("observed.json");
    let path = adapter(
        dir.path(),
        &format!("cat > '{}'\n{}", request.display(), print_answer(&answer())),
    );
    enable(&vault, path);
    let item = capture::capture(&vault, source()).unwrap();
    let report = capture::work(&vault, 1).unwrap();
    assert_eq!(report.prepared, 1, "{report:?}");
    let observed: serde_json::Value = serde_json::from_slice(&fs::read(request).unwrap()).unwrap();
    assert_eq!(observed["source"]["input"]["event_id"], "e1");
    assert_eq!(observed["related_knowledge"][0]["id"], related.meta.id);
    let item = capture::get(&vault, item.id).unwrap().1;
    assert_eq!(item.status, Status::NeedsReview);
    let candidate = item.candidate.unwrap();
    assert_eq!(candidate.knowledge.title, "Recover lost cache");
    assert_eq!(candidate.distillation.unwrap().backend, "command-v1");
    assert_eq!(vault.entries().unwrap().len(), 1);
    // Backend config and executable arguments are excluded even in old vaults.
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap()
    };
    assert!(git(&["init"]).status.success());
    assert!(
        git(&["check-ignore", ".relic/automation/distillation.json"])
            .status
            .success()
    );
}

#[cfg(unix)]
#[test]
fn invalid_model_results_fail_without_echoing_response_or_stderr() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, source()).unwrap();
    let mut invalid_evidence = answer();
    invalid_evidence["evidence_indices"] = json!([9]);
    let mut invented_reference = answer();
    invented_reference["related_entry_ids"] = json!(["not-in-request"]);
    let mut invalid_confidence = answer();
    invalid_confidence["knowledge"]["confidence"] = json!(4);
    for body in [
        "echo PRIVATE_PAYLOAD; echo PRIVATE_DIAGNOSTIC >&2".into(),
        print_answer(&invalid_evidence),
        print_answer(&invented_reference),
        print_answer(&invalid_confidence),
        "echo PRIVATE_DIAGNOSTIC >&2; exit 7".into(),
    ] {
        enable(&vault, adapter(dir.path(), &body));
        let report = capture::work(&vault, 1).unwrap();
        assert_eq!(report.failed, 1);
        let state = capture::get(&vault, item.id).unwrap().1;
        assert_eq!(state.status, Status::Failed);
        assert!(!state.last_error.unwrap().contains("PRIVATE"));
        capture::retry(&vault, item.id).unwrap();
    }
    assert!(vault.entries().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn timeout_and_output_limits_leave_recoverable_failure() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, source()).unwrap();
    for body in ["exec sleep 5", "head -c 70000 /dev/zero"] {
        let path = adapter(dir.path(), body);
        distillation::configure(
            &vault,
            &Config::Command {
                executable: path,
                args: vec![],
                timeout_seconds: 1,
            },
        )
        .unwrap();
        let start = std::time::Instant::now();
        assert_eq!(capture::work(&vault, 1).unwrap().failed, 1);
        assert!(start.elapsed() < std::time::Duration::from_secs(3));
        capture::retry(&vault, item.id).unwrap();
    }
    enable(&vault, adapter(dir.path(), &print_answer(&answer())));
    let report = capture::work(&vault, 1).unwrap();
    assert_eq!(report.prepared, 1, "{report:?}");
}

#[cfg(unix)]
#[test]
fn slow_distillation_does_not_block_capture_or_competing_worker() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let marker = dir.path().join("started");
    enable(
        &vault,
        adapter(
            dir.path(),
            &format!(
                "touch '{}'\nsleep 0.7\n{}",
                marker.display(),
                print_answer(&answer())
            ),
        ),
    );
    capture::capture(&vault, source()).unwrap();
    let clone = vault.clone();
    let worker = std::thread::spawn(move || capture::work(&clone, 1).unwrap());
    let start = std::time::Instant::now();
    while !marker.exists() {
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let start = std::time::Instant::now();
    let mut second = source();
    second.event_id = "e2".into();
    let captured = capture::capture(&vault, second).unwrap();
    assert!(capture::work(&vault, 1).unwrap().busy);
    assert!(start.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(captured.status, Status::Pending);
    let report = worker.join().unwrap();
    assert_eq!(report.prepared, 1, "{report:?}");
}

#[cfg(unix)]
#[test]
fn redistill_preserves_old_draft_on_failure_and_never_reopens_reviewed_entries() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, source()).unwrap();
    capture::work(&vault, 1).unwrap();
    let original = capture::get(&vault, item.id)
        .unwrap()
        .1
        .candidate
        .unwrap()
        .knowledge;
    enable(&vault, adapter(dir.path(), "exit 1"));
    capture::redistill(&vault, item.id).unwrap();
    assert_eq!(capture::work(&vault, 1).unwrap().failed, 1);
    assert_eq!(
        capture::get(&vault, item.id)
            .unwrap()
            .1
            .candidate
            .unwrap()
            .knowledge,
        original
    );
    distillation::configure(&vault, &Config::Template).unwrap();
    capture::retry(&vault, item.id).unwrap();
    capture::work(&vault, 1).unwrap();
    capture::review(
        &vault,
        item.id,
        capture::Review {
            decision: capture::Decision::Reject,
            reason: "Transient".into(),
            knowledge: None,
        },
    )
    .unwrap();
    assert!(capture::redistill(&vault, item.id).is_err());
}

#[test]
fn targeted_processing_leaves_other_pending_captures_untouched() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let first = capture::capture(&vault, source()).unwrap();
    let mut second = source();
    second.event_id = "e2".into();
    let second = capture::capture(&vault, second).unwrap();
    assert_eq!(capture::work_one(&vault, second.id).unwrap().prepared, 1);
    assert_eq!(
        capture::get(&vault, first.id).unwrap().1.status,
        Status::Pending
    );
    assert_eq!(
        capture::get(&vault, second.id).unwrap().1.status,
        Status::NeedsReview
    );
}
