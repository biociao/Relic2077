use crate::entry::{Entry, subject_tags};
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

/// Note the absence of "against". In technical prose it is almost always a
/// preposition — "validate against the corpus", "reconcile against the ledger" —
/// so it produced a negative verdict for sentences that state no verdict at all.
/// An explicit negation such as "not work" or "does not" carries that meaning
/// without the false positives.
const NEGATIVE_MARKERS: &[&str] = &[
    "fail",
    "reject",
    "invalid",
    "false",
    "ineffective",
    "unreliable",
    "avoid",
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

/// How many more markers one direction must carry than the other before the
/// text counts as *stating* a claim rather than merely containing such a word.
///
/// One incidental marker is not a verdict: "Recursive splitting avoids mid
/// sentence cuts" is a positive engineering statement that happens to contain
/// "avoids". Requiring a margin of two means a false contradiction is much less
/// likely, at the cost of missing some subtle disagreements. That trade is
/// deliberate — a wrong conflict edge corrupts the graph and every reflection
/// built on it, while a missed one merely leaves two memories unlinked.
pub const CLAIM_MINIMUM_STRENGTH: usize = 2;

/// The claim markers a text states, counted by direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Claim {
    pub positive: usize,
    pub negative: usize,
}

impl Claim {
    /// The direction of the claim: +1 positive, -1 negative, 0 undecided.
    pub fn net(&self) -> i8 {
        match self.negative.cmp(&self.positive) {
            Ordering::Greater => -1,
            Ordering::Less => 1,
            Ordering::Equal => 0,
        }
    }

    /// How one-sided the text is: the margin between the two directions.
    pub fn strength(&self) -> usize {
        self.positive.abs_diff(self.negative)
    }

    /// True when the text states a claim firmly enough to act on.
    pub fn is_decisive(&self, minimum: usize) -> bool {
        self.net() != 0 && self.strength() >= minimum
    }
}

/// Count the claim markers a text states.
pub fn claim(text: &str) -> Claim {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    Claim {
        positive: POSITIVE_MARKERS
            .iter()
            .filter(|marker| matches_marker(&words, &lower, marker))
            .count(),
        negative: NEGATIVE_MARKERS
            .iter()
            .filter(|marker| matches_marker(&words, &lower, marker))
            .count(),
    }
}

/// Classify a text as net positive (+1), net negative (-1), or undecided (0).
///
/// This is the direction only. Use [`Claim::is_decisive`] before treating the
/// direction as something the writer actually claimed.
pub fn polarity(text: &str) -> i8 {
    claim(text).net()
}

/// True when `text` states `marker`.
///
/// A marker containing a space or a punctuation mark is a phrase such as
/// "not work" or "doesn't". Those are distinctive enough that a substring test
/// cannot fire on an unrelated word, so they are matched directly.
fn matches_marker(words: &[&str], lower: &str, marker: &str) -> bool {
    if !marker.chars().all(|character| character.is_alphanumeric()) {
        return lower.contains(marker);
    }
    words.iter().any(|word| inflects_from(word, marker))
}

/// True when `word` is `marker` carrying a regular English inflection.
///
/// Deliberately a closed suffix list rather than a stemmer: the point is to
/// accept "fails" for "fail" while refusing "validate" for "valid", which a
/// shared-prefix test would wrongly accept.
fn inflects_from(word: &str, marker: &str) -> bool {
    if word == marker {
        return true;
    }
    match word.strip_prefix(marker) {
        Some(rest) => matches!(rest, "s" | "es" | "ed" | "d" | "ing"),
        None => false,
    }
}

/// Find pairs of active entries that share at least one subject tag and carry
/// opposite *decisive* claims. The detected pair is a signal for the writer to
/// review; it is not a judgement about which entry is right.
///
/// Only subject tags are compared: a namespaced scope tag (`src:`, `project:`,
/// `section:`, `pool:`) groups memories by where they came from, and pairing
/// everything inside such a group claims contradictions that do not exist.
pub fn detect_contradictions(entries: &[Entry]) -> Vec<Contradiction> {
    let mut by_subject: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for entry in entries {
        if entry.meta.status == "superseded" || entry.meta.status == "archived" {
            continue;
        }
        for tag in subject_tags(&entry.meta.tags) {
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
                let a_claim = claim(&format!("{} {}", a.meta.title, a.body));
                let b_claim = claim(&format!("{} {}", b.meta.title, b.body));
                if !a_claim.is_decisive(CLAIM_MINIMUM_STRENGTH)
                    || !b_claim.is_decisive(CLAIM_MINIMUM_STRENGTH)
                {
                    continue;
                }
                let (a_polarity, b_polarity) = (a_claim.net(), b_claim.net());
                if a_polarity != b_polarity {
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
        for tag in subject_tags(&entry.meta.tags) {
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
