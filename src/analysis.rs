use crate::entry::Entry;
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// A pair of knowledge entries that share a subject and reach opposite
/// conclusions, detected by claim polarity over their titles and bodies.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Contradiction {
    pub a: String,
    pub b: String,
    pub subjects: Vec<String>,
    pub a_polarity: i8,
    pub b_polarity: i8,
    pub reason: String,
}

/// A reusable pattern proposal extracted from a group of related entries.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PatternProposal {
    pub tag: String,
    pub members: Vec<String>,
    pub members_ids: Vec<String>,
    pub title: String,
    pub body: String,
}

const POSITIVE_MARKERS: &[&str] = &[
    "works",
    "support",
    "valid",
    "true",
    "effective",
    "reliable",
    "recommend",
    "benefit",
    "improve",
    "confirm",
    "proven",
    "success",
    "use",
    "usable",
    "best",
];

const NEGATIVE_MARKERS: &[&str] = &[
    "fail",
    "reject",
    "invalid",
    "false",
    "ineffective",
    "unreliable",
    "avoid",
    "against",
    "not work",
    "never",
    "wrong",
    "no evidence",
    "does not",
    "doesn't",
    "cannot",
    "can't",
    "broken",
    "bad",
];

/// Classify a text as containing a net positive (+1), net negative (-1), or no
/// consistent claim (0) based on the presence of known polarity markers.
fn polarity(text: &str) -> i8 {
    let lower = text.to_lowercase();
    let positive = POSITIVE_MARKERS
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count();
    let negative = NEGATIVE_MARKERS
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count();
    match negative.cmp(&positive) {
        Ordering::Greater => -1,
        Ordering::Less => 1,
        Ordering::Equal => 0,
    }
}

/// Find pairs of active entries that share at least one subject tag and carry
/// opposite claim polarity. The detected pair is a signal for the writer to
/// review; it is not a judgement about which entry is right.
pub fn detect_contradictions(entries: &[Entry]) -> Vec<Contradiction> {
    let mut by_subject: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        if entry.meta.status == "superseded" || entry.meta.status == "archived" {
            continue;
        }
        for tag in &entry.meta.tags {
            by_subject.entry(tag.clone()).or_default().push(entry);
        }
    }
    let mut results = Vec::new();
    for (subject, group) in by_subject {
        for i in 0..group.len() {
            for j in (i + 1)..group.len() {
                let a = group[i];
                let b = group[j];
                if a.meta.id == b.meta.id {
                    continue;
                }
                let a_polarity = polarity(&format!("{} {}", a.meta.title, a.body));
                let b_polarity = polarity(&format!("{} {}", b.meta.title, b.body));
                if a_polarity != 0 && b_polarity != 0 && a_polarity != b_polarity {
                    results.push(Contradiction {
                        a: a.meta.id.clone(),
                        b: b.meta.id.clone(),
                        subjects: vec![subject.clone()],
                        a_polarity,
                        b_polarity,
                        reason: format!(
                            "entries '{}' and '{}' both concern '{}' but state opposite claims",
                            a.meta.title, b.meta.title, subject
                        ),
                    });
                }
            }
        }
    }
    results
}

/// Group active entries by subject tag and propose a reusable pattern for any
/// group with at least `min_members` members. The proposal is a draft; the
/// `write_patterns` command optionally materialises it as a new entry.
pub fn extract_patterns(entries: &[Entry], min_members: usize) -> Vec<PatternProposal> {
    let mut by_subject: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        if entry.meta.status == "superseded" || entry.meta.status == "archived" {
            continue;
        }
        for tag in &entry.meta.tags {
            by_subject.entry(tag.clone()).or_default().push(entry);
        }
    }
    let mut results = Vec::new();
    for (tag, group) in by_subject {
        if group.len() < min_members {
            continue;
        }
        let mut titles = group
            .iter()
            .map(|entry| entry.meta.title.as_str())
            .collect::<Vec<_>>();
        titles.sort();
        let body = format!(
            "## Recurring theme: {tag}\n\nMembers ({n}):\n{members}\n\n## Consensus\n- Validate each member before applying this pattern broadly.",
            n = group.len(),
            members = titles
                .iter()
                .map(|title| format!("- {title}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        results.push(PatternProposal {
            tag: tag.clone(),
            members: titles.into_iter().map(str::to_owned).collect(),
            members_ids: group.iter().map(|entry| entry.meta.id.clone()).collect(),
            title: format!("{tag} pattern"),
            body,
        });
    }
    results
}
