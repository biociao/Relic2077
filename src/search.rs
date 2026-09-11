//! Retrieval across the two layers: keyword precision and vector recall.
//!
//! Relic has two ways to find a memory, and they fail in opposite directions.
//! FTS5 keyword search is precise but literal: it cannot find "chunk size" when
//! the memory says "chunking strategy", and it treats a Chinese query as an
//! exact contiguous run. The vector layer ([`crate::embedding`]) is the reverse:
//! it finds related memories regardless of wording but cannot express "this
//! exact identifier".
//!
//! [`Mode::Hybrid`] runs both and merges the two rankings with Reciprocal Rank
//! Fusion, `score = Σ 1 / (k + rank)`. RRF is used deliberately: it needs only
//! the *order* each method produced, so the two layers' incomparable scores —
//! BM25, which is unbounded, and cosine, which is not — never have to be
//! normalised against each other. A hit ranked well by both methods rises; a hit
//! only one method can see still appears.

use crate::entry::Entry;
use crate::index::SearchHit;
use crate::vault::Vault;
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// The RRF damping constant. The value from the original TREC work; large
/// enough that the top few ranks of either method carry similar weight, which is
/// what makes the fusion robust when one method is much more confident.
pub const RRF_K: f64 = 60.0;

/// How many candidates each layer contributes before fusion. Wider than the
/// requested limit so a memory ranked modestly by both layers can still surface.
const CANDIDATE_DEPTH: usize = 40;

/// Characters of an entry body used as an excerpt when the memory was found by
/// the vector layer, which does not produce a keyword snippet.
const EXCERPT_CHARACTERS: usize = 220;

/// Which retrieval layers to consult.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// FTS5 keyword search only. Precise, literal, and unchanged from earlier
    /// releases, which is why it stays the default for existing callers.
    Keyword,
    /// The vector layer only. Finds related memories regardless of wording.
    Semantic,
    /// Both layers, fused by reciprocal rank.
    Hybrid,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "keyword" => Ok(Self::Keyword),
            "semantic" => Ok(Self::Semantic),
            "hybrid" => Ok(Self::Hybrid),
            other => bail!("unknown search mode '{other}' (expected keyword, semantic, or hybrid)"),
        }
    }
}

/// One memory found by one or both layers.
#[derive(Debug, Clone, Serialize)]
pub struct FusedHit {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub confidence: f64,
    pub status: String,
    pub path: String,
    /// The fusion score. Only meaningful relative to other hits in this result.
    pub score: f64,
    /// 1-based position in the keyword ranking, when that layer found it.
    pub keyword_rank: Option<usize>,
    /// 1-based position in the vector ranking, when that layer found it.
    pub semantic_rank: Option<usize>,
    /// Cosine similarity, when the vector layer found it.
    pub similarity: Option<f64>,
    /// The vocabulary that carried the similarity, for a reader to judge it.
    pub shared_terms: Vec<String>,
}

/// A memory related to another memory, with the vocabulary that relates them.
#[derive(Debug, Clone, Serialize)]
pub struct RelatedMemory {
    pub id: String,
    pub title: String,
    pub path: String,
    pub tags: Vec<String>,
    pub confidence: f64,
    pub similarity: f64,
    pub shared_terms: Vec<String>,
}

/// Search the vault in the requested mode.
pub fn search(vault: &Vault, query: &str, limit: usize, mode: Mode) -> Result<Vec<FusedHit>> {
    let entries = vault.entries()?;
    let by_id: HashMap<&str, &Entry> = entries
        .iter()
        .map(|entry| (entry.meta.id.as_str(), entry))
        .collect();
    let depth = limit.max(1).saturating_mul(4).clamp(1, CANDIDATE_DEPTH);

    let mut fused: BTreeMap<String, FusedHit> = BTreeMap::new();
    if matches!(mode, Mode::Keyword | Mode::Hybrid) {
        for (position, hit) in vault.search(query, depth)?.into_iter().enumerate() {
            let rank = position + 1;
            let slot = fused
                .entry(hit.id.clone())
                .or_insert_with(|| FusedHit::from_search_hit(&hit));
            slot.score += reciprocal_rank(rank);
            slot.keyword_rank = Some(rank);
        }
    }
    if matches!(mode, Mode::Semantic | Mode::Hybrid) {
        for (position, hit) in vault
            .semantic_queries(query, depth)?
            .into_iter()
            .enumerate()
        {
            let rank = position + 1;
            let slot = fused.entry(hit.id.clone()).or_insert_with(|| {
                FusedHit::from_related(&hit, by_id.get(hit.id.as_str()).copied())
            });
            slot.score += reciprocal_rank(rank);
            slot.semantic_rank = Some(rank);
            slot.similarity = Some(hit.similarity);
            slot.shared_terms = hit.shared_terms;
        }
    }

    let mut hits: Vec<FusedHit> = fused.into_values().collect();
    hits.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.id.cmp(&right.id))
    });
    hits.truncate(limit);
    Ok(hits)
}

impl FusedHit {
    fn from_search_hit(hit: &SearchHit) -> Self {
        Self {
            id: hit.id.clone(),
            title: hit.title.clone(),
            excerpt: hit.excerpt.replace('\n', " "),
            confidence: hit.confidence,
            status: hit.status.clone(),
            path: hit.path.clone(),
            score: 0.0,
            keyword_rank: None,
            semantic_rank: None,
            similarity: None,
            shared_terms: Vec::new(),
        }
    }

    fn from_related(hit: &RelatedMemory, entry: Option<&Entry>) -> Self {
        let excerpt = entry
            .map(|entry| excerpt_of(&entry.body, EXCERPT_CHARACTERS))
            .unwrap_or_default();
        Self {
            id: hit.id.clone(),
            title: hit.title.clone(),
            excerpt,
            confidence: hit.confidence,
            status: entry
                .map(|entry| entry.meta.status.clone())
                .unwrap_or_default(),
            path: hit.path.clone(),
            score: 0.0,
            keyword_rank: None,
            semantic_rank: None,
            similarity: Some(hit.similarity),
            shared_terms: hit.shared_terms.clone(),
        }
    }
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

/// Collapse a body into a single-line excerpt, cut on a character boundary.
///
/// Entries created by Relic begin with the title as a Markdown heading, and a
/// caller printing a hit already shows the title, so that heading is dropped
/// rather than repeated in every excerpt.
pub fn excerpt_of(body: &str, limit: usize) -> String {
    let body = strip_leading_heading(body);
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let mut excerpt: String = collapsed.chars().take(limit).collect();
    excerpt.push('…');
    excerpt
}

fn strip_leading_heading(body: &str) -> &str {
    let trimmed = body.trim_start();
    let Some(first_line) = trimmed.lines().next() else {
        return trimmed;
    };
    if first_line.trim_start().starts_with('#') {
        return trimmed[first_line.len()..].trim_start();
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, title: &str, body: &str) -> Entry {
        let now = chrono::Utc::now();
        Entry {
            meta: crate::entry::EntryMeta {
                id: id.into(),
                kind: "knowledge".into(),
                title: title.into(),
                status: "active".into(),
                confidence: 0.7,
                tags: vec![],
                source_agents: vec![],
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
            path: std::path::PathBuf::from(format!("{id}.md")),
        }
    }

    #[test]
    fn keyword_only_mode_is_the_literal_search() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::init(directory.path()).unwrap();
        vault
            .create(
                "Chunking",
                "Use 512 token chunks.",
                "knowledge",
                vec![],
                0.8,
                "test",
            )
            .unwrap();
        let hits = search(&vault, "chunks", 10, Mode::Keyword).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].semantic_rank.is_none());
        assert!(hits[0].keyword_rank.is_some());
    }

    #[test]
    fn hybrid_finds_a_memory_the_keyword_layer_cannot() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::init(directory.path()).unwrap();
        for index in 0..6 {
            vault
                .create(
                    &format!("Filler {index}"),
                    "Unrelated note about alpine travel packing and waterproof boots for the hut.",
                    "knowledge",
                    vec!["life".into()],
                    0.8,
                    "test",
                )
                .unwrap();
        }
        let target = vault
            .create(
                "Chunking strategy for retrieval",
                "Use 512 token chunks for prose documents and validate the split against the corpus.",
                "knowledge",
                vec!["rag".into()],
                0.8,
                "test",
            )
            .unwrap();

        // No single memory contains every word of this query, so the keyword
        // layer's implicit AND finds nothing at all — a common experience when
        // a reader types what they mean rather than the wording that was used.
        let query = "chunks prose tokens alpine";
        let keyword = search(&vault, query, 10, Mode::Keyword).unwrap();
        assert!(
            keyword.is_empty(),
            "the keyword layer unexpectedly matched: {keyword:?}"
        );

        let hybrid = search(&vault, query, 10, Mode::Hybrid).unwrap();
        let found = hybrid
            .iter()
            .find(|hit| hit.id == target.meta.id)
            .expect("hybrid should surface the related memory");
        assert!(found.semantic_rank.is_some());
        assert!(found.similarity.unwrap() > 0.08);
        assert!(
            !found.shared_terms.is_empty(),
            "a semantic hit must explain itself"
        );
    }

    #[test]
    fn fusion_rewards_a_hit_both_layers_agree_on() {
        // A hit ranked first by both layers must beat one ranked first by only
        // one of them, which is the whole point of fusing.
        let both = reciprocal_rank(1) + reciprocal_rank(1);
        let single = reciprocal_rank(1);
        assert!(both > single);
    }

    #[test]
    fn modes_are_parsed_and_validated() {
        assert_eq!(Mode::parse("hybrid").unwrap(), Mode::Hybrid);
        assert_eq!(Mode::parse(" SEMANTIC ").unwrap(), Mode::Semantic);
        assert!(Mode::parse("fuzzy").is_err());
    }

    #[test]
    fn excerpts_collapse_and_truncate_on_character_boundaries() {
        let entry = entry("a", "A", "line one\nline two   line three");
        assert_eq!(excerpt_of(&entry.body, 220), "line one line two line three");
        // A leading Markdown heading repeats the title a caller already shows.
        assert_eq!(
            excerpt_of("# The title\n\nThe body follows.", 220),
            "The body follows."
        );
        assert_eq!(excerpt_of("# Only a heading", 220), "");
        let chinese = "知识图谱架构设计需要建立逻辑矢量关系网络并且保持一致";
        let excerpt = excerpt_of(chinese, 6);
        assert_eq!(excerpt.chars().count(), 7);
        assert!(excerpt.ends_with('…'));
    }
}
