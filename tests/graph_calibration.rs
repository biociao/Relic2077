//! Guards the calibrated similarity thresholds.
//!
//! The defaults in `GraphConfig` are not guesses: they were measured over
//! memories that share a subject versus memories that do not. This test keeps
//! that separation honest, so a change to the tokenizer, the weighting, or the
//! hashing that destroys it fails here with numbers instead of quietly
//! producing a graph full of unrelated edges — or an empty one.

use chrono::Utc;
use relic2077::config::GraphConfig;
use relic2077::embedding::EmbeddingIndex;
use relic2077::entry::{Entry, EntryMeta};
use relic2077::graph::{Derivation, EdgeKind, Graph};
use std::path::PathBuf;

fn entry(id: &str, title: &str, body: &str, tags: &[&str]) -> Entry {
    let now = Utc::now();
    Entry {
        meta: EntryMeta {
            id: id.into(),
            kind: "knowledge".into(),
            title: title.into(),
            status: "active".into(),
            confidence: 0.8,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            source_agents: vec!["codex".into()],
            created: now,
            updated: now,
            last_verified: now,
            expires: None,
            supersedes: vec![],
            superseded_by: None,
            links: vec![],
            decay_rate: 0.05,
        },
        body: body.into(),
        path: PathBuf::from(format!("{id}.md")),
    }
}

/// Six memories in three subjects: two pairs that genuinely belong together and
/// two singletons that share nothing with the rest.
fn corpus() -> Vec<Entry> {
    vec![
        entry(
            "chunk-a",
            "Chunking strategy for retrieval",
            "Use 512 token chunks for prose documents and validate the split against the corpus. \
             Overlapping chunks of about twenty percent preserve context across a boundary. \
             Recursive splitting on headings works better than a fixed character budget for \
             technical documentation with code blocks.",
            &["rag", "chunking"],
        ),
        entry(
            "chunk-b",
            "Retrieval chunk size selection",
            "Chunk prose at 512 tokens so retrieval quality stays stable across queries. \
             Selecting the chunk size matters more than the embedding model for short documents. \
             Recursive splitting by heading keeps code blocks intact and avoids mid sentence cuts.",
            &["rag", "chunking"],
        ),
        entry(
            "git-a",
            "Git conflict resolution policy",
            "When a merge conflicts the remote version becomes canonical and the local version is \
             preserved under a conflict folder. Nothing is silently dropped. The search index is \
             rebuilt after every merge regardless of the outcome.",
            &["git", "sync"],
        ),
        entry(
            "git-b",
            "Merge conflict handling in sync",
            "A conflicting merge keeps the remote file as canonical and stores the local file in a \
             timestamped conflict directory. The index is rebuilt after the merge finishes.",
            &["git", "sync"],
        ),
        entry(
            "travel",
            "Alpine travel packing list",
            "Bring layered clothing, a waterproof shell, and broken in boots. Pack light because \
             the hut provides blankets and meals. Waterproof gloves matter more than a spare sweater.",
            &["life"],
        ),
        entry(
            "decay",
            "Confidence decays from last verification",
            "Effective confidence multiplies the stored value by an exponential decay from the last \
             verification date. Expired entries report zero. Updating confidence refreshes the \
             verification timestamp, which is what keeps a memory trustworthy over time.",
            &["memory", "confidence"],
        ),
    ]
}

/// Pair every memory with every other, split into the ones that share a subject
/// and the ones that do not.
fn partitioned_scores(entries: &[Entry]) -> (Vec<f64>, Vec<f64>) {
    let index = EmbeddingIndex::build(entries, GraphConfig::default().dimensions, "test").unwrap();
    let mut related = Vec::new();
    let mut unrelated = Vec::new();
    for left in 0..entries.len() {
        for right in (left + 1)..entries.len() {
            let score = index
                .cosine(&entries[left].meta.id, &entries[right].meta.id)
                .unwrap();
            let shares = entries[left]
                .meta
                .tags
                .iter()
                .any(|tag| entries[right].meta.tags.contains(tag));
            if shares {
                related.push(score);
            } else {
                unrelated.push(score);
            }
        }
    }
    (related, unrelated)
}

#[test]
fn the_similarity_floor_sits_in_the_gap() {
    let entries = corpus();
    let config = GraphConfig::default();
    let (related, unrelated) = partitioned_scores(&entries);

    let weakest_related = related.iter().copied().fold(f64::INFINITY, f64::min);
    let strongest_unrelated = unrelated.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let strongest_related = related.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    assert!(
        weakest_related > strongest_unrelated,
        "subjects do not separate: related min {weakest_related:.3}, unrelated max {strongest_unrelated:.3}"
    );
    assert!(
        strongest_unrelated < config.semantic_min_similarity,
        "the floor of {} admits an unrelated pair scoring {strongest_unrelated:.3}",
        config.semantic_min_similarity
    );
    assert!(
        weakest_related > config.semantic_min_similarity,
        "the floor of {} rejects a related pair scoring {weakest_related:.3}",
        config.semantic_min_similarity
    );
    // A floor that sits at the edge of the gap would flip on the next content
    // change; require a real margin on both sides.
    assert!(
        weakest_related > config.semantic_min_similarity * 2.0,
        "{weakest_related:.3} is too close to the floor"
    );
    assert!(
        strongest_unrelated * 2.0 < config.semantic_min_similarity,
        "{strongest_unrelated:.3} is too close to the floor"
    );
    println!(
        "related {weakest_related:.3}..{strongest_related:.3}; unrelated <= {strongest_unrelated:.3}; floor {:.3}",
        config.semantic_min_similarity
    );
}

#[test]
fn subjects_produce_edges_and_singletons_do_not() {
    let entries = corpus();
    let index = EmbeddingIndex::build(&entries, GraphConfig::default().dimensions, "test").unwrap();
    let graph = Graph::build(&entries, &index, &GraphConfig::default(), "test");

    for pair in [("chunk-a", "chunk-b"), ("git-a", "git-b")] {
        assert!(
            graph
                .explain(pair.0, pair.1)
                .iter()
                .any(|edge| edge.kind == EdgeKind::Semantic),
            "{} and {} share a subject but got no semantic edge: {:?}",
            pair.0,
            pair.1,
            graph.explain(pair.0, pair.1)
        );
    }
    for pair in [
        ("chunk-a", "travel"),
        ("chunk-b", "decay"),
        ("travel", "decay"),
    ] {
        assert!(
            !graph
                .explain(pair.0, pair.1)
                .iter()
                .any(|edge| matches!(edge.kind, EdgeKind::Semantic | EdgeKind::Tag)),
            "{} and {} are unrelated but were joined",
            pair.0,
            pair.1
        );
    }
}

#[test]
fn every_semantic_edge_carries_vocabulary_and_a_derivation() {
    let entries = corpus();
    let index = EmbeddingIndex::build(&entries, GraphConfig::default().dimensions, "test").unwrap();
    let graph = Graph::build(&entries, &index, &GraphConfig::default(), "test");

    let semantic: Vec<_> = graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Semantic)
        .collect();
    assert!(!semantic.is_empty());
    for edge in semantic {
        assert_eq!(edge.derivation, Derivation::Statistical);
        assert!(
            edge.evidence.contains("nearest vocabulary:"),
            "an inferred edge must say what carried it: {}",
            edge.evidence
        );
        assert!(
            !edge.evidence.ends_with("none"),
            "a semantic edge with no explaining vocabulary is unexplained: {}",
            edge.evidence
        );
    }
}

#[test]
fn the_graph_represents_a_vault_smaller_than_its_thresholds() {
    // A brand new vault must still build, and must say why it looks sparse.
    let entries = vec![
        entry(
            "a",
            "First memory",
            "The first thing worth remembering.",
            &["start"],
        ),
        entry(
            "b",
            "Second memory",
            "The second thing worth remembering.",
            &["start"],
        ),
    ];
    let index = EmbeddingIndex::build(&entries, GraphConfig::default().dimensions, "test").unwrap();
    let graph = Graph::build(&entries, &index, &GraphConfig::default(), "test");
    assert_eq!(graph.nodes.len(), 2);
    assert!(!graph.notes.is_empty());
    // The shared subject tag still produces an edge even when the corpus is too
    // small for the vector layer to say anything.
    assert!(
        graph
            .explain("a", "b")
            .iter()
            .any(|edge| edge.kind == EdgeKind::Tag),
        "{:?}",
        graph.edges
    );
}
