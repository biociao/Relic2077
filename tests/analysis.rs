use relic2077::analysis::{
    CLAIM_MINIMUM_STRENGTH, claim, detect_contradictions, extract_patterns, polarity,
};
use relic2077::vault::Vault;
use std::fs;
use tempfile::tempdir;

fn vault_with(entries: &[(&str, &str, &[&str])]) -> (tempfile::TempDir, Vault) {
    let directory = tempdir().unwrap();
    let vault = Vault::init(directory.path()).unwrap();
    for (title, content, tags) in entries {
        vault
            .create(
                title,
                content,
                "knowledge",
                tags.iter().map(|t| t.to_string()).collect(),
                0.8,
                "test",
            )
            .unwrap();
    }
    (directory, vault)
}

#[test]
fn detects_opposing_claims_that_share_a_subject() {
    let (_dir, vault) = vault_with(&[
        (
            "RAG chunking works",
            "RAG works and is effective.",
            &["rag"],
        ),
        (
            "RAG chunking fails",
            "RAG fails and is unreliable.",
            &["rag"],
        ),
        ("RAG setup notes", "A note about the setup.", &["rag"]),
    ]);
    let contradictions = detect_contradictions(&vault.entries().unwrap());
    // The "works" and "fails" pair oppose each other; the neutral "setup notes"
    // entry carries no claim polarity and is excluded. Only one pair flips.
    assert_eq!(contradictions.len(), 1);
    assert_eq!(contradictions[0].subjects, vec!["rag"]);
    assert_ne!(contradictions[0].a_polarity, contradictions[0].b_polarity);
}

#[test]
fn does_not_flag_entries_with_match_polarity_or_disjoint_subjects() {
    let (_dir, vault) = vault_with(&[
        ("A works", "A is effective.", &["alpha"]),
        ("B works", "B is reliable.", &["alpha"]),
        ("C fails", "C is unreliable.", &["beta"]),
    ]);
    let contradictions = detect_contradictions(&vault.entries().unwrap());
    // A and B share "alpha" but both are positive; C contradicts none because
    // it belongs to a disjoint subject tag ("beta").
    assert!(contradictions.is_empty());
}

#[test]
fn polarity_matches_words_not_substrings() {
    // Containment is not a claim: "validate" contains "valid", and "against" in
    // "against the corpus" is a preposition rather than a negative verdict.
    assert_eq!(polarity("Validate the split against the corpus."), 0);
    assert_eq!(polarity("The index is rebuilt after every merge."), 0);
    assert_eq!(polarity("Nothing is silently dropped."), 0);

    // Inflections of a marker still count.
    assert_eq!(polarity("The approach works."), 1);
    assert_eq!(polarity("The approach failed."), -1);
    assert_eq!(polarity("The strategy is ineffective and broken."), -1);
    assert_eq!(polarity("The strategy is effective and reliable."), 1);
}

#[test]
fn one_incidental_marker_is_not_a_claim() {
    // "avoids" is a negative word, but one incidental marker is not a verdict.
    let weak = claim("Recursive splitting avoids mid sentence cuts.");
    assert_eq!(weak.net(), -1);
    assert_eq!(weak.strength(), 1);
    assert!(!weak.is_decisive(CLAIM_MINIMUM_STRENGTH));

    let decisive = claim("The strategy is ineffective and broken.");
    assert_eq!(decisive.net(), -1);
    assert!(decisive.is_decisive(CLAIM_MINIMUM_STRENGTH));
}

#[test]
fn compatible_memories_about_one_subject_are_not_contradictions() {
    // Two memories describing the same technique in the same direction. One of
    // them contains "avoids"; the other contains "use" and "works". Counting
    // any marker as a claim made this pair look like a contradiction.
    let (_dir, vault) = vault_with(&[
        (
            "Chunking strategy for retrieval",
            "Use 512 token chunks for prose documents and validate the split against the corpus. Recursive splitting works better than a fixed character budget.",
            &["rag"],
        ),
        (
            "Retrieval chunk size selection",
            "Chunk prose at 512 tokens so retrieval quality stays stable. Recursive splitting by heading avoids mid sentence cuts and keeps code blocks intact.",
            &["rag"],
        ),
    ]);
    assert!(
        detect_contradictions(&vault.entries().unwrap()).is_empty(),
        "agreeing memories were reported as contradicting"
    );
}

#[test]
fn extracts_patterns_only_from_sufficient_groups() {
    let (_dir, vault) = vault_with(&[
        ("P1", "Content one.", &["pattern"]),
        ("P2", "Content two.", &["pattern"]),
        ("P3", "Content three.", &["pattern"]),
        ("Solo", "Alone.", &["solo"]),
    ]);
    let patterns = extract_patterns(&vault.entries().unwrap(), 2);
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].tag, "pattern");
    assert_eq!(patterns[0].members.len(), 3);
    assert!(extract_patterns(&vault.entries().unwrap(), 4).is_empty());
}

#[test]
fn reflection_embeds_detected_contradictions_and_patterns() {
    let (directory, vault) = vault_with(&[
        (
            "RAG chunking works",
            "RAG works and is effective.",
            &["rag"],
        ),
        (
            "RAG chunking fails",
            "RAG fails and is unreliable.",
            &["rag"],
        ),
        ("P1", "One.", &["pattern"]),
        ("P2", "Two.", &["pattern"]),
        ("P3", "Three.", &["pattern"]),
    ]);
    let path = vault.create_reflection("daily").unwrap();
    assert!(path.exists());
    // The same-period reflection already exists.
    assert!(vault.reflection_path("daily").unwrap().is_some());
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("## Contradictions"));
    assert!(text.contains("RAG chunking works"));
    assert!(text.contains("## Patterns worth extracting"));
    assert!(text.contains("pattern"));
    let _ = directory;
}

#[test]
fn write_pattern_materialises_a_pattern_entry() {
    let (_dir, vault) = vault_with(&[
        ("P1", "One.", &["pattern"]),
        ("P2", "Two.", &["pattern"]),
        ("P3", "Three.", &["pattern"]),
    ]);
    let entry = vault.write_pattern("pattern").unwrap();
    assert_eq!(entry.meta.kind, "pattern");
    assert!(entry.body.contains("Recurring theme"));
    // Writing a pattern leaves the source entries intact.
    assert!(
        vault
            .entries()
            .unwrap()
            .iter()
            .any(|e| e.meta.kind == "knowledge")
    );
}

#[test]
fn reflection_auto_trigger_skips_when_below_threshold() {
    let (directory, vault) = vault_with(&[("One", "Only one.", &["x"])]);
    // One entry is below a ">= 5" threshold, so automatic reflection is blocked.
    assert!(!vault.should_reflect("weekly", 5).unwrap());
    // A threshold of one entry allows it.
    assert!(vault.should_reflect("weekly", 1).unwrap());
    let _ = directory;
}

#[test]
fn reflection_auto_trigger_skips_when_already_present() {
    let (directory, vault) = vault_with(&[
        ("One", "One.", &["x"]),
        ("Two", "Two.", &["y"]),
        ("Three", "Three.", &["z"]),
        ("Four", "Four.", &["w"]),
        ("Five", "Five.", &["q"]),
    ]);
    assert!(vault.should_reflect("daily", 5).unwrap());
    vault.create_reflection("daily").unwrap();
    // Once the reflection exists, the same-period trigger is a no-op.
    assert!(!vault.should_reflect("daily", 5).unwrap());
    let _ = directory;
}

#[test]
fn namespaced_scope_tags_never_produce_contradictions_or_patterns() {
    // A provenance tag shared by an import batch, or a project tag shared by a
    // whole repository, is a *scope*, not a subject. Pairing everything inside
    // it would claim a contradiction between two memories that were merely
    // imported together — the failure mode of the 182-entry Codex import.
    let (_dir, vault) = vault_with(&[
        (
            "Batch entry one",
            "The pipeline works and is reliable.",
            &["src:codex-memory", "project:gi03", "pool:preconscious"],
        ),
        (
            "Batch entry two",
            "The pipeline fails and is unreliable.",
            &["src:codex-memory", "project:gi03", "pool:preconscious"],
        ),
        (
            "Batch entry three",
            "The pipeline works and is effective.",
            &["src:codex-memory", "project:gi03", "pool:preconscious"],
        ),
    ]);
    let entries = vault.entries().unwrap();
    assert!(detect_contradictions(&entries).is_empty());
    assert!(extract_patterns(&entries, 2).is_empty());

    // The same entries *do* relate once they carry a real subject tag.
    let (_dir, vault) = vault_with(&[
        (
            "Subject entry one",
            "The pipeline works and is reliable.",
            &["src:codex-memory", "pipeline"],
        ),
        (
            "Subject entry two",
            "The pipeline fails and is unreliable.",
            &["src:codex-memory", "pipeline"],
        ),
    ]);
    let entries = vault.entries().unwrap();
    assert_eq!(detect_contradictions(&entries).len(), 1);
    assert_eq!(extract_patterns(&entries, 2).len(), 1);
}
