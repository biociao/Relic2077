use relic2077::entry::Entry;
use relic2077::vault::{EntryPatch, Vault};
use tempfile::tempdir;

#[test]
fn initializes_creates_and_finds_entries() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let created = vault
        .create(
            "Chunking strategy",
            "Use 512 token chunks for prose.",
            "knowledge",
            vec!["rag".into()],
            0.82,
            "codex",
        )
        .unwrap();
    assert_eq!(
        vault.get(&created.meta.id).unwrap().meta.title,
        "Chunking strategy"
    );
    let hits = vault.search("chunks", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, created.meta.id);
}

#[test]
fn entry_round_trips() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let entry = vault
        .create(
            "Durable memory",
            "Never fade away.",
            "pattern",
            vec![],
            0.9,
            "",
        )
        .unwrap();
    let rendered = entry.render().unwrap();
    let parsed = Entry::parse(&entry.path, &rendered).unwrap();
    assert_eq!(parsed.meta, entry.meta);
    assert!(parsed.body.contains("Never fade away"));
}

#[test]
fn updates_selected_entry_fields() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let entry = vault
        .create("Old title", "Old body", "knowledge", vec![], 0.5, "")
        .unwrap();

    let updated = vault
        .update(
            &entry.meta.id,
            EntryPatch {
                title: Some("New title".into()),
                confidence: Some(0.9),
                tags: Some(vec!["verified".into()]),
                ..EntryPatch::default()
            },
        )
        .unwrap();

    assert_eq!(updated.meta.title, "New title");
    assert_eq!(updated.meta.confidence, 0.9);
    assert_eq!(updated.meta.tags, vec!["verified"]);
    assert_eq!(updated.body, entry.body);
}

#[test]
fn supersedes_entries_without_deleting_history() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    let old = vault
        .create(
            "Old guidance",
            "Do the old thing.",
            "knowledge",
            vec![],
            0.6,
            "",
        )
        .unwrap();
    let new = vault
        .create(
            "New guidance",
            "Do the new thing.",
            "knowledge",
            vec![],
            0.9,
            "",
        )
        .unwrap();

    vault.supersede(&old.meta.id, &new.meta.id).unwrap();

    let old = vault.get(&old.meta.id).unwrap();
    let new = vault.get(&new.meta.id).unwrap();
    assert_eq!(old.meta.status, "superseded");
    assert_eq!(
        old.meta.superseded_by.as_deref(),
        Some(new.meta.id.as_str())
    );
    assert!(new.meta.supersedes.contains(&old.meta.id));
    assert!(old.path.exists());
}
