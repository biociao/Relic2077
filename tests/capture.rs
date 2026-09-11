use relic2077::capture::{self, CaptureInput, Decision, Review, Status};
use relic2077::vault::Vault;
use std::fs;
use tempfile::tempdir;

fn input() -> CaptureInput {
    CaptureInput {
        event_id: "fix-1".into(),
        project: "example".into(),
        session_id: "session-1".into(),
        source_agent: "test-agent".into(),
        title: "Cache recovery".into(),
        context: "Cache was missing after a restart".into(),
        action: "Rebuilt cache from Markdown".into(),
        outcome: "Search worked after restart".into(),
        evidence: vec!["Restart regression test passed".into()],
        tags: vec!["cache".into()],
    }
}
fn accept() -> Review {
    Review {
        decision: Decision::Accept,
        reason: "Reviewed the restart test and applicable environment".into(),
        knowledge: None,
    }
}
fn state_path(vault: &Vault, id: uuid::Uuid) -> std::path::PathBuf {
    vault.root.join(format!(".relic/queue/{id}.json"))
}

#[test]
fn capture_survives_restart_and_retries_without_duplicates() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, input()).unwrap();
    assert_eq!(item.status, Status::Pending);
    let reopened = Vault::discover(dir.path()).unwrap();
    assert_eq!(capture::capture(&reopened, input()).unwrap().id, item.id);
    let mut changed = input();
    changed.outcome = "Different outcome".into();
    assert!(capture::capture(&reopened, changed).is_err());
    let mut another_session = input();
    another_session.session_id = "session-2".into();
    assert_ne!(
        capture::capture(&reopened, another_session).unwrap().id,
        item.id
    );
    assert_eq!(capture::list(&reopened).unwrap().len(), 2);
    assert!(reopened.entries().unwrap().is_empty());
}

#[test]
fn review_publishes_once_and_preserves_source_and_reason() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let related = vault
        .create(
            "Cache notes",
            "Earlier cache knowledge",
            "knowledge",
            vec!["cache".into()],
            0.8,
            "other",
        )
        .unwrap();
    let item = capture::capture(&vault, input()).unwrap();
    assert!(capture::review(&vault, item.id, accept()).is_err());
    assert_eq!(capture::work(&vault, 100).unwrap().prepared, 1);
    let (source, prepared) = capture::get(&vault, item.id).unwrap();
    assert_eq!(source.input, input());
    assert_eq!(
        prepared.candidate.unwrap().related_entries,
        vec![related.meta.id]
    );
    assert!(vault.search("restart", 10).unwrap().is_empty());
    let accepted = capture::review(&vault, item.id, accept()).unwrap();
    assert_eq!(accepted.status, Status::Accepted);
    let entry = vault.get(accepted.entry_id.as_ref().unwrap()).unwrap();
    assert_eq!(entry.meta.source_agents, vec!["test-agent"]);
    assert!(entry.body.contains("fix-1"));
    assert!(entry.body.contains(&accept().reason));
    assert!(entry.body.contains("not independently verified"));
    assert!(vault.root.join(&entry.meta.links[0]).exists());
    assert_eq!(vault.search("restart", 10).unwrap().len(), 1);
    assert_eq!(
        capture::review(&vault, item.id, accept()).unwrap().entry_id,
        accepted.entry_id
    );
    assert_eq!(capture::work(&vault, 100).unwrap().prepared, 0);
    assert_eq!(vault.entries().unwrap().len(), 2);
    let conflicting = Review {
        reason: "Changed after publication".into(),
        ..accept()
    };
    assert!(capture::review(&vault, item.id, conflicting).is_err());
}

#[test]
fn worker_recovers_missing_state_and_interrupted_processing() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let first = capture::capture(&vault, input()).unwrap();
    fs::remove_file(state_path(&vault, first.id)).unwrap();
    assert_eq!(capture::work(&vault, 1).unwrap().prepared, 1);
    let (_, mut item) = capture::get(&vault, first.id).unwrap();
    item.status = Status::Processing;
    fs::write(
        state_path(&vault, first.id),
        serde_json::to_vec(&item).unwrap(),
    )
    .unwrap();
    let report = capture::work(&Vault::discover(dir.path()).unwrap(), 1).unwrap();
    assert_eq!(report.recovered, 1);
    assert_eq!(
        capture::get(&vault, first.id).unwrap().1.status,
        Status::NeedsReview
    );
}

#[test]
fn interrupted_review_recovers_before_or_after_markdown_publication() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, input()).unwrap();
    capture::work(&vault, 1).unwrap();
    let (_, mut prepared) = capture::get(&vault, item.id).unwrap();
    prepared.review = Some(accept());
    // Simulates a crash after decision persistence but before Markdown write.
    fs::write(
        state_path(&vault, item.id),
        serde_json::to_vec(&prepared).unwrap(),
    )
    .unwrap();
    assert_eq!(capture::work(&vault, 1).unwrap().recovered, 1);
    assert_eq!(vault.entries().unwrap().len(), 1);
    // Simulates a crash after Markdown publication but before status update.
    fs::write(
        state_path(&vault, item.id),
        serde_json::to_vec(&prepared).unwrap(),
    )
    .unwrap();
    assert_eq!(capture::work(&vault, 1).unwrap().recovered, 1);
    assert_eq!(vault.entries().unwrap().len(), 1);
    assert_eq!(
        capture::get(&vault, item.id).unwrap().1.status,
        Status::Accepted
    );
    // Portable source + accepted Markdown reconstruct accepted state on another device.
    fs::remove_file(state_path(&vault, item.id)).unwrap();
    assert_eq!(capture::work(&vault, 1).unwrap().recovered, 1);
    assert_eq!(
        capture::get(&vault, item.id).unwrap().1.status,
        Status::Accepted
    );
}

#[test]
fn failed_source_is_isolated_and_retries_explicitly() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let bad = capture::capture(&vault, input()).unwrap();
    let mut second = input();
    second.event_id = "fix-2".into();
    let good = capture::capture(&vault, second).unwrap();
    let path = vault.root.join(format!("sources/captures/{}.json", bad.id));
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"invalid json").unwrap();
    let report = capture::work(&vault, 100).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.prepared, 1);
    assert_eq!(
        capture::get(&vault, good.id).unwrap().1.status,
        Status::NeedsReview
    );
    assert_eq!(capture::work(&vault, 100).unwrap().failed, 0);
    fs::write(path, original).unwrap();
    capture::retry(&vault, bad.id).unwrap();
    assert_eq!(capture::work(&vault, 100).unwrap().prepared, 1);
    assert_eq!(capture::get(&vault, bad.id).unwrap().1.attempts, 2);
}

#[test]
fn rejected_experiences_stay_out_of_knowledge_and_watch_prepares_drafts() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, input()).unwrap();
    assert_eq!(vault.maintain("weekly", 5).unwrap().captures.prepared, 1);
    let review = Review {
        decision: Decision::Reject,
        reason: "Duplicate of an existing lesson".into(),
        knowledge: None,
    };
    assert_eq!(
        capture::review(&vault, item.id, review.clone())
            .unwrap()
            .status,
        Status::Rejected
    );
    assert_eq!(
        capture::review(&vault, item.id, review).unwrap().status,
        Status::Rejected
    );
    assert!(capture::retry(&vault, item.id).is_err());
    assert!(capture::review(&vault, item.id, accept()).is_err());
    assert!(vault.entries().unwrap().is_empty());
    assert_eq!(capture::list(&vault).unwrap().len(), 1);
}

#[test]
fn concurrent_capture_and_review_are_idempotent() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let vault = vault.clone();
            std::thread::spawn(move || capture::capture(&vault, input()).unwrap().id)
        })
        .collect();
    let ids: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    capture::work(&vault, 100).unwrap();
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let vault = vault.clone();
            let id = ids[0];
            std::thread::spawn(move || capture::review(&vault, id, accept()).unwrap())
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap().status, Status::Accepted);
    }
    assert_eq!(vault.entries().unwrap().len(), 1);
}

#[test]
fn invalid_input_and_review_cannot_publish() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let mut invalid = input();
    invalid.event_id.clear();
    assert!(capture::capture(&vault, invalid).is_err());
    assert!(capture::work(&vault, 0).is_err());
    let item = capture::capture(&vault, input()).unwrap();
    capture::work(&vault, 1).unwrap();
    let review = Review {
        reason: " ".into(),
        ..accept()
    };
    assert!(capture::review(&vault, item.id, review).is_err());
    assert!(vault.entries().unwrap().is_empty());
}

#[test]
fn existing_vault_gitignore_excludes_local_state_but_includes_sources() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    fs::write(dir.path().join(".gitignore"), "custom-ignore\n").unwrap();
    let item = capture::capture(&vault, input()).unwrap();
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
        git(&["check-ignore", &format!(".relic/queue/{}.json", item.id)])
            .status
            .success()
    );
    assert!(
        git(&["check-ignore", ".relic/queue/worker.lock"])
            .status
            .success()
    );
    assert!(
        !git(&[
            "check-ignore",
            &format!("sources/captures/{}.json", item.id)
        ])
        .status
        .success()
    );
    assert_eq!(
        fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
        "custom-ignore\n"
    );
}

#[test]
fn index_failure_after_publication_recovers_without_overwriting_knowledge() {
    let dir = tempdir().unwrap();
    let vault = Vault::init(dir.path()).unwrap();
    let item = capture::capture(&vault, input()).unwrap();
    capture::work(&vault, 1).unwrap();
    let index = vault.root.join(".relic/index.sqlite");
    fs::remove_file(&index).unwrap();
    fs::create_dir(&index).unwrap();
    assert!(capture::review(&vault, item.id, accept()).is_err());
    let entry = vault.entries().unwrap().pop().unwrap();
    let before = fs::read(&entry.path).unwrap();
    assert_eq!(
        capture::get(&vault, item.id).unwrap().1.status,
        Status::NeedsReview
    );
    fs::remove_dir(&index).unwrap();
    assert_eq!(capture::work(&vault, 1).unwrap().recovered, 1);
    assert_eq!(fs::read(&entry.path).unwrap(), before);
    assert_eq!(vault.entries().unwrap().len(), 1);
    assert_eq!(vault.search("restart", 10).unwrap().len(), 1);
}
