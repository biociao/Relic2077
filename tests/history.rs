use relic2077::{
    capture::{self, CaptureInput, Decision, Review, Status},
    history::{self, ImportRequest, ScanRequest},
    vault::Vault,
};
use serde_json::{Value, json};
use std::{fs, path::Path};
use tempfile::tempdir;
fn transcript(id: &str) -> Vec<Value> {
    vec![
        json!({"type":"session_meta","payload":{"id":id,"cwd":"/other/project"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Fix cache"}]}}),
        json!({"type":"response_item","payload":{"type":"reasoning","summary":"SECRET REASONING"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","output":"RAW TOOL LOG"}}),
        json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"INTERMEDIATE"}]}}),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1","last_agent_message":"Cache fixed"}}),
    ]
}
fn write(path: &Path, lines: &[Value]) {
    fs::write(
        path,
        lines.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
}
fn scan(v: &Vault, root: &Path, host: &str) -> Value {
    history::scan(
        v,
        ScanRequest {
            host: host.into(),
            root: root.into(),
        },
    )
    .unwrap()
}
fn request(root: &Path, row: &Value, host: &str) -> ImportRequest {
    ImportRequest {
        host: host.into(),
        root: root.into(),
        file: row["file"].as_str().unwrap().into(),
        fingerprint: row["fingerprint"].as_str().unwrap().into(),
    }
}
fn live() -> CaptureInput {
    CaptureInput {
        event_id: "turn:turn-1".into(),
        project: "/other/project".into(),
        session_id: "session-1".into(),
        source_agent: "codex".into(),
        title: "live".into(),
        context: "Fix cache".into(),
        action: "Automatic".into(),
        outcome: "Cache fixed".into(),
        tags: vec!["automatic-capture".into()],
        evidence: vec![],
    }
}
#[test]
fn manual_scan_is_read_only_import_is_idempotent_and_review_is_required() {
    let dir = tempdir().unwrap();
    let v = Vault::init(&dir.path().join("vault")).unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir(&root).unwrap();
    write(&root.join("session.jsonl"), &transcript("session-1"));
    let data = scan(&v, &root, "codex");
    let row = &data["sessions"][0];
    assert_eq!(row["status"], "available");
    assert!(capture::list(&v).unwrap().is_empty());
    assert!(!v.root.join("sources/captures").exists());
    let item = history::import(&v, request(&root, row, "codex")).unwrap();
    assert_eq!(item.status, Status::Pending);
    let (source, _) = capture::get(&v, item.id).unwrap();
    assert!(source.input.outcome.contains("Cache fixed"));
    for excluded in ["SECRET REASONING", "RAW TOOL LOG", "INTERMEDIATE"] {
        assert!(!source.input.outcome.contains(excluded));
    }
    assert_eq!(
        history::import(&v, request(&root, row, "codex"))
            .unwrap()
            .id,
        item.id
    );
    assert!(v.entries().unwrap().is_empty());
    assert_eq!(
        scan(&v, &root, "codex")["sessions"][0]["status"],
        "captured"
    );
    assert_eq!(capture::capture(&v, live()).unwrap().id, item.id);
    let mut delayed = live();
    delayed.event_id = "turn:fallback-generated-uuid".into();
    assert_eq!(capture::capture(&v, delayed).unwrap().id, item.id);
    let mut new_turn = live();
    new_turn.event_id = "turn:later".into();
    new_turn.context = "New work".into();
    new_turn.outcome = "New result".into();
    capture::capture(&v, new_turn).unwrap();
    assert_eq!(capture::list(&v).unwrap().len(), 2);
    capture::work_one(&v, item.id).unwrap();
    capture::review(
        &v,
        item.id,
        Review {
            decision: Decision::Accept,
            reason: "Checked".into(),
            knowledge: None,
        },
    )
    .unwrap();
    assert_eq!(
        scan(&v, &root, "codex")["sessions"][0]["status"],
        "recorded"
    );
}
#[test]
fn existing_live_rejected_or_failed_sessions_cannot_be_reimported() {
    let dir = tempdir().unwrap();
    let v = Vault::init(&dir.path().join("vault")).unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir(&root).unwrap();
    write(&root.join("s.jsonl"), &transcript("session-1"));
    let data = scan(&v, &root, "codex");
    let row = &data["sessions"][0];
    let item = capture::capture(&v, live()).unwrap();
    assert!(history::import(&v, request(&root, row, "codex")).is_err());
    capture::work_one(&v, item.id).unwrap();
    capture::review(
        &v,
        item.id,
        Review {
            decision: Decision::Reject,
            reason: "Duplicate".into(),
            knowledge: None,
        },
    )
    .unwrap();
    assert_eq!(
        scan(&v, &root, "codex")["sessions"][0]["status"],
        "captured"
    );
    assert!(history::import(&v, request(&root, row, "codex")).is_err());
}
#[test]
fn stale_snapshots_path_escape_incomplete_and_legacy_writes_fail_closed() {
    let dir = tempdir().unwrap();
    let v = Vault::init(&dir.path().join("vault")).unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir(&root).unwrap();
    let path = root.join("s.jsonl");
    write(&path, &transcript("session-1"));
    let data = scan(&v, &root, "codex");
    let row = &data["sessions"][0];
    let mut t = transcript("session-1");
    t.push(json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"new"}}));
    write(&path, &t);
    assert!(history::import(&v, request(&root, row, "codex")).is_err());
    assert_eq!(scan(&v, &root, "codex")["sessions"][0]["status"], "active");
    let mut req = request(&root, row, "codex");
    req.file = "../outside.jsonl".into();
    assert!(history::import(&v, req).is_err());
    t = transcript("session-1");
    t.push(json!({"type":"response_item","payload":{"type":"custom_tool_call","input":"tools.mcp__relic__relic_create_entry(...)"}}));
    write(&path, &t);
    assert_eq!(
        scan(&v, &root, "codex")["sessions"][0]["status"],
        "check_required"
    );
    assert!(capture::list(&v).unwrap().is_empty());
    fs::write(&path, "{\"unfinished\"").unwrap();
    let data = scan(&v, &root, "codex");
    assert!(data["sessions"].as_array().unwrap().is_empty());
    assert_eq!(data["errors"].as_array().unwrap().len(), 1);
}
#[test]
fn concurrent_history_import_and_live_capture_never_duplicate_covered_turn() {
    let dir = tempdir().unwrap();
    let v = Vault::init(&dir.path().join("vault")).unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir(&root).unwrap();
    write(&root.join("s.jsonl"), &transcript("session-1"));
    let data = scan(&v, &root, "codex");
    let request = request(&root, &data["sessions"][0], "codex");
    std::thread::scope(|s| {
        let v = &v;
        s.spawn(move || {
            let _ = history::import(v, request);
        });
        s.spawn(move || {
            capture::capture(v, live()).unwrap();
        });
    });
    assert_eq!(capture::list(&v).unwrap().len(), 1);
}
#[test]
fn dsh_zstd_uses_completed_final_messages_and_excludes_aborted_turns() {
    let dir = tempdir().unwrap();
    let v = Vault::init(&dir.path().join("vault")).unwrap();
    let root = dir.path().join("sessions");
    fs::create_dir(&root).unwrap();
    let t = [
        json!({"type":"session","id":"dsh-1","cwd":"/other/project"}),
        json!({"type":"turn/start","data":{"turn":1}}),
        json!({"type":"user/message","data":{"content":[{"type":"text","text":"Question"}]}}),
        json!({"type":"assistant/message","data":{"turn":1,"message":{"content":[{"type":"text","text":"ABORTED"}]}}}),
        json!({"type":"turn/end","data":{"turn":1,"reason":{"kind":"aborted"}}}),
        json!({"type":"turn/start","data":{"turn":2}}),
        json!({"type":"assistant/message","data":{"turn":2,"message":{"content":[{"type":"text","text":"FINAL"}]}}}),
        json!({"type":"turn/end","data":{"turn":2,"reason":{"kind":"completed"}}}),
    ];
    let bytes = t.iter().map(|x| format!("{x}\n")).collect::<String>();
    fs::write(
        root.join("session.jsonl.zstd"),
        zstd::stream::encode_all(bytes.as_bytes(), 1).unwrap(),
    )
    .unwrap();
    let data = scan(&v, &root, "dsh");
    let row = &data["sessions"][0];
    assert_eq!(row["turns"], 1);
    assert_eq!(row["status"], "available");
    let q = history::import(&v, request(&root, row, "dsh")).unwrap();
    let (s, _) = capture::get(&v, q.id).unwrap();
    assert!(s.input.outcome.contains("FINAL"));
    assert!(!s.input.outcome.contains("ABORTED"));
}
