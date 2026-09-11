use relic2077::codex_memory::{ImportRequest, import, memory_id, parse};
use relic2077::vault::Vault;
use std::fs;
use tempfile::tempdir;

const MEMORY: &str = "
# Task Group: alpha — remote routing

scope: Keep remote operations reproducible.
applies_to: cwd=/repo/alpha; reuse_rule=reuse the routing rule, recheck the branch.

## Task 1: Record the default working directory

### rollout_summary_files

- rollout_summaries/2026-09-09T07-49-03-abc-routing.md (cwd=/repo/alpha, rollout_path=/sessions/x.jsonl, thread_id=01a08524, success)

### keywords

- routing, worktree, cwd

## User preferences

- 用户说“以这个为默认工作目录” -> 后续仓库操作默认使用 `/repo/alpha`。 [Task 1]
- Do not assume a branch is current; verify before consequential operations. [Task 1]

## Reusable knowledge

- Remote repo: `c4g.tun:/CHAOSBJ0/PUMCH/alpha/`
- A wrapped bullet that continues
  on the following line without a marker.

## Failures and how to do differently

- A stale snapshot produced a wrong branch report. Re-read HEAD before reporting. [Task 1]

# Task Group: beta — unrelated work

scope: Beta scope.
applies_to: cwd=/repo/beta.

## User preferences

- Prefer editable deliverables over raster images. [Task 1]

## Keywords

- beta, raster
";

fn write_memory(directory: &std::path::Path, text: &str) -> std::path::PathBuf {
    let path = directory.join("MEMORY.md");
    fs::write(&path, text).unwrap();
    path
}

fn request(path: &std::path::Path) -> ImportRequest {
    ImportRequest {
        source: Some(path.to_owned()),
        update: false,
        dry_run: false,
        exclude_task_groups: vec![],
        confidence: None,
    }
}

#[test]
fn parses_only_memory_sections_and_joins_wrapped_bullets() {
    let points = parse(MEMORY);
    // alpha: 2 preferences + 2 knowledge + 1 failure; beta: 1 preference.
    // keywords and rollout_summary_files are pointers, not knowledge, so their
    // bullets must not become memories.
    assert_eq!(points.len(), 6);
    assert!(
        points
            .iter()
            .all(|point| !point.text.contains("routing, worktree"))
    );
    assert!(
        points
            .iter()
            .all(|point| !point.text.contains("rollout_summaries/2026"))
    );
    assert!(points.iter().any(|point| point.kind == "lesson"));
    let wrapped = points
        .iter()
        .find(|point| point.text.starts_with("A wrapped bullet"))
        .expect("wrapped bullet survives as one point");
    assert_eq!(
        wrapped.text,
        "A wrapped bullet that continues on the following line without a marker."
    );
    // The reuse boundary travels with the point.
    let routing = points
        .iter()
        .find(|point| point.text.contains("Remote repo"))
        .unwrap();
    assert_eq!(routing.task_group, "alpha — remote routing");
    assert!(routing.applies_to.contains("recheck the branch"));
}

#[test]
fn import_is_idempotent_and_deterministic() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let source = write_memory(directory.path(), MEMORY);

    let first = import(&vault, request(&source)).unwrap();
    assert_eq!((first.parsed, first.created, first.skipped), (6, 6, 0));
    assert!(first.errors.is_empty());
    assert_eq!(vault.entries().unwrap().len(), 6);

    let second = import(&vault, request(&source)).unwrap();
    assert_eq!((second.created, second.skipped), (0, 6));
    assert_eq!(vault.entries().unwrap().len(), 6);

    // A dry run reports the same conclusion without touching the vault.
    let mut dry = request(&source);
    dry.dry_run = true;
    let dry = import(&vault, dry).unwrap();
    assert_eq!((dry.created, dry.skipped), (0, 6));
}

#[test]
fn entries_carry_provenance_and_stay_unverified() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let source = write_memory(directory.path(), MEMORY);
    import(&vault, request(&source)).unwrap();

    let entries = vault.entries().unwrap();
    for entry in &entries {
        assert_eq!(entry.meta.source_agents, vec!["codex-memory".to_owned()]);
        assert!(entry.meta.tags.iter().any(|tag| tag == "unverified"));
        assert!(entry.meta.confidence < 0.7, "not in the verified band");
        assert!(entry.meta.id.starts_with("relic-codex-memory-"));
    }
    let routing = entries
        .iter()
        .find(|entry| entry.body.contains("c4g.tun:/CHAOSBJ0/PUMCH/alpha/"))
        .unwrap();
    assert!(routing.body.contains("**Applies to**: cwd=/repo/alpha"));
    assert!(routing.meta.links[0].starts_with("codex-memory:codex:"));
    // Imported memories are searchable through the normal index.
    assert!(
        vault
            .search("CHAOSBJ0", 10)
            .unwrap()
            .iter()
            .any(|hit| hit.id == routing.meta.id)
    );
}

#[test]
fn changed_bullets_create_new_entries_instead_of_rewriting() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let source = write_memory(directory.path(), MEMORY);
    import(&vault, request(&source)).unwrap();

    // Codex rewrites a bullet: the old memory is history, not a silent overwrite.
    let edited = MEMORY.replace(
        "Prefer editable deliverables over raster images. [Task 1]",
        "Prefer editable deliverables over raster images, and ship the file, not a plan. [Task 1]",
    );
    write_memory(directory.path(), &edited);
    let report = import(&vault, request(&source)).unwrap();
    assert_eq!(report.created, 1);
    assert_eq!(report.skipped, 5);
    assert_eq!(vault.entries().unwrap().len(), 7);
}

#[test]
fn update_refreshes_the_matching_entry_without_duplicating() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let source = write_memory(directory.path(), MEMORY);
    import(&vault, request(&source)).unwrap();
    let before = vault.entries().unwrap().len();

    let mut update = request(&source);
    update.update = true;
    let report = import(&vault, update).unwrap();
    assert_eq!(report.updated, 6);
    assert_eq!(vault.entries().unwrap().len(), before);
}

#[test]
fn exclusion_keeps_self_referential_task_groups_out() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let source = write_memory(directory.path(), MEMORY);
    let mut exclude = request(&source);
    exclude.exclude_task_groups = vec!["beta".into()];

    let report = import(&vault, exclude).unwrap();
    assert_eq!((report.parsed, report.excluded, report.created), (5, 1, 5));
    assert!(
        vault
            .entries()
            .unwrap()
            .iter()
            .all(|entry| !entry.meta.title.contains("beta — unrelated work"))
    );
}

#[test]
fn identity_is_stable_and_group_scoped() {
    let points = parse(MEMORY);
    let first = points.first().unwrap();
    let mut echo = first.clone();
    // The same sentence echoed under another task group is a different memory.
    echo.task_group = "gamma — echo".into();
    assert_ne!(memory_id(first), memory_id(&echo));
    assert_eq!(memory_id(first), memory_id(&first.clone()));
}

#[test]
fn missing_source_fails_with_a_readable_error() {
    let directory = tempdir().unwrap();
    let vault = Vault::init(&directory.path().join("vault")).unwrap();
    let missing = directory.path().join("nowhere/MEMORY.md");
    let error = import(&vault, request(&missing)).unwrap_err().to_string();
    assert!(error.contains("Codex memories"), "got: {error}");
}
