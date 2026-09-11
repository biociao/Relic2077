use serde_json::{Value, json};
use std::fs;
use tempfile::tempdir;

#[test]
fn cli_capture_watch_review_and_search() {
    let dir = tempdir().unwrap();
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_relic"))
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(&["init"]);
    fs::write(
        dir.path().join("capture.json"),
        json!({
            "event_id":"one","project":"project","session_id":"session","title":"Restart recovery",
            "context":"Index missing","action":"Reindex","outcome":"Search restored"
        })
        .to_string(),
    )
    .unwrap();
    let captured: Value = serde_json::from_str(&run(&["capture", "capture.json"])).unwrap();
    let id = captured["id"].as_str().unwrap();
    run(&["watch", "--once"]);
    let item: Value = serde_json::from_str(&run(&["queue", "get", id])).unwrap();
    assert_eq!(item["item"]["status"], "needs_review");
    assert!(run(&["search", "Restart"]).is_empty());
    fs::write(
        dir.path().join("review.json"),
        json!({"decision":"accept","reason":"Verified in regression test"}).to_string(),
    )
    .unwrap();
    let accepted: Value =
        serde_json::from_str(&run(&["queue", "review", id, "review.json"])).unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert!(run(&["search", "Restart"]).contains("Restart recovery"));
}
