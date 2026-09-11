//! The vector layer: a deterministic, dependency-free embedding of every memory.
//!
//! Relic's contract is that Markdown owns the truth and every derived artifact
//! can be deleted and rebuilt. This module honours that contract for *semantic*
//! similarity: instead of calling a hosted embedding model, it projects the
//! token stream of an entry into a hashed feature space with inverse document
//! frequency weighting. That gives a real cosine-similarity space — the same
//! retrieval geometry a sparse TF-IDF model provides — while staying local,
//! offline, reproducible, and inspectable.
//!
//! How it works:
//!
//! 1. [`crate::text::tokenize`] turns an entry into tokens; the title is
//!    repeated and tags are included because both are dense summaries.
//! 2. Sublinear term frequency (`1 + ln tf`) times BM25-style inverse document
//!    frequency (`ln(1 + (N - df + 0.5) / (df + 0.5))`, always positive) weights
//!    each token.
//! 3. [`crate::text::hash_token`] maps a token to a signed dimension, so
//!    collisions partially cancel instead of always reinforcing.
//! 4. The vector is L2-normalised, which makes the inner product the cosine.
//!
//! Because hashing loses the token behind a dimension, the corpus also records
//! up to [`crate::text::REPRESENTATIVE_TERMS_PER_DIMENSION`] tokens per occupied
//! dimension. That is what lets [`EmbeddingIndex::explain`] answer *why* two
//! memories are similar with words rather than only a number.
//!
//! Everything here is a cache. [`EmbeddingIndex::load`] returns `None` for a
//! missing, unreadable, or stale file, and the caller simply rebuilds.

use crate::entry::Entry;
use crate::text::{REPRESENTATIVE_TERMS_PER_DIMENSION, TOKENIZER_VERSION, hash_token, tokenize};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

/// On-disk format version for the vector cache. Bump when the layout changes.
pub const EMBEDDING_VERSION: u32 = 1;

/// Smallest magnitude a dimension keeps. Dropping near-zero weights removes
/// cancellation noise from the inverted index without changing cosine values
/// beyond float noise.
const MINIMUM_WEIGHT: f32 = 1e-4;

const COMPRESSION_LEVEL: i32 = 3;

/// A sparse vector over the hashed feature space. Dimensions are sorted
/// ascending and weights are L2-normalised.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SparseVector {
    pub dims: Vec<u32>,
    pub weights: Vec<f32>,
}

impl SparseVector {
    pub fn is_empty(&self) -> bool {
        self.dims.is_empty()
    }

    /// Inner product of two sparse vectors. Both the query and the corpus
    /// vectors are normalised, so this is the cosine similarity.
    pub fn dot(&self, other: &SparseVector) -> f32 {
        let (mut left, mut right) = (0, 0);
        let mut total = 0.0;
        while left < self.dims.len() && right < other.dims.len() {
            match self.dims[left].cmp(&other.dims[right]) {
                std::cmp::Ordering::Less => left += 1,
                std::cmp::Ordering::Greater => right += 1,
                std::cmp::Ordering::Equal => {
                    total += self.weights[left] * other.weights[right];
                    left += 1;
                    right += 1;
                }
            }
        }
        total
    }
}

/// One memory that the vector layer considers close to a query vector.
#[derive(Debug, Clone, Serialize)]
pub struct SimilarityHit {
    pub id: String,
    pub similarity: f64,
    /// Representative terms of the dimensions that carried the similarity.
    pub shared_terms: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredVector {
    id: String,
    dims: Vec<u32>,
    weights: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredIndex {
    version: u32,
    tokenizer_version: u32,
    dimensions: usize,
    fingerprint: String,
    document_count: usize,
    /// Token to document frequency, sorted by token.
    vocabulary: Vec<(String, u32)>,
    /// Occupied dimension to its most frequent tokens, sorted by dimension.
    terms: Vec<(u32, Vec<String>)>,
    documents: Vec<StoredVector>,
}

/// The in-memory vector layer for one vault revision.
#[derive(Debug, Clone)]
pub struct EmbeddingIndex {
    dimensions: usize,
    document_count: usize,
    fingerprint: String,
    /// Token to document frequency. Inverse document frequency is derived from
    /// it on demand, so the cache round-trips exactly.
    vocabulary: HashMap<String, u32>,
    terms: HashMap<u32, Vec<String>>,
    ids: Vec<String>,
    positions: HashMap<String, usize>,
    vectors: Vec<SparseVector>,
    /// Dimension to `(document position, weight)` postings. `nearest` only
    /// visits dimensions the query actually occupies, which is what keeps the
    /// all-pairs semantic edge build affordable.
    inverted: Vec<Vec<(u32, f32)>>,
}

impl EmbeddingIndex {
    /// Project every entry into the hashed feature space.
    ///
    /// Entries are processed in stable ID order so that the document frequency
    /// table — and therefore every vector — is identical on every rebuild.
    pub fn build(entries: &[Entry], dimensions: usize, fingerprint: &str) -> Result<Self> {
        if dimensions == 0 {
            bail!("embedding dimensions must be greater than zero");
        }
        let mut ordered: Vec<&Entry> = entries.iter().collect();
        ordered.sort_by(|left, right| left.meta.id.cmp(&right.meta.id));

        let mut frequencies = Vec::with_capacity(ordered.len());
        let mut document_frequency: HashMap<String, u32> = HashMap::new();
        for entry in &ordered {
            let counts = term_counts(&embedding_text(entry));
            for token in counts.keys() {
                *document_frequency.entry(token.clone()).or_insert(0) += 1;
            }
            frequencies.push(counts);
        }

        let document_count = ordered.len();
        let ids: Vec<String> = ordered.iter().map(|entry| entry.meta.id.clone()).collect();
        let vectors: Vec<SparseVector> = frequencies
            .iter()
            .map(|counts| {
                vectorize(
                    counts,
                    &document_frequency,
                    document_count,
                    dimensions,
                    false,
                )
            })
            .collect();
        let terms = representative_terms(&document_frequency, dimensions);

        Ok(Self::assemble(
            dimensions,
            document_count,
            fingerprint.to_owned(),
            document_frequency,
            terms,
            ids,
            vectors,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        dimensions: usize,
        document_count: usize,
        fingerprint: String,
        vocabulary: HashMap<String, u32>,
        terms: HashMap<u32, Vec<String>>,
        ids: Vec<String>,
        vectors: Vec<SparseVector>,
    ) -> Self {
        let positions = ids
            .iter()
            .enumerate()
            .map(|(position, id)| (id.clone(), position))
            .collect();
        let mut inverted = vec![Vec::new(); dimensions];
        for (position, vector) in vectors.iter().enumerate() {
            for (dim, weight) in vector.dims.iter().zip(vector.weights.iter()) {
                inverted[*dim as usize].push((position as u32, *weight));
            }
        }
        Self {
            dimensions,
            document_count,
            fingerprint,
            vocabulary,
            terms,
            ids,
            positions,
            vectors,
            inverted,
        }
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }

    pub fn position_of(&self, id: &str) -> Option<usize> {
        self.positions.get(id).copied()
    }

    pub fn vector(&self, id: &str) -> Option<&SparseVector> {
        self.positions
            .get(id)
            .map(|position| &self.vectors[*position])
    }

    /// Cosine similarity between two stored memories.
    pub fn cosine(&self, left: &str, right: &str) -> Option<f64> {
        let left = self.vector(left)?;
        let right = self.vector(right)?;
        Some(f64::from(left.dot(right)))
    }

    /// Project arbitrary text into the same space so it can be compared with
    /// stored memories.
    ///
    /// Tokens the corpus never saw are dropped rather than given a synthetic
    /// weight. An unknown token cannot legitimately match any document, so
    /// keeping it would only add hash-collision noise to the score.
    pub fn encode(&self, text: &str) -> SparseVector {
        vectorize(
            &term_counts(text),
            &self.vocabulary,
            self.document_count,
            self.dimensions,
            true,
        )
    }

    /// Rank stored memories against an arbitrary vector.
    pub fn nearest(
        &self,
        query: &SparseVector,
        limit: usize,
        minimum_similarity: f64,
        exclude: Option<usize>,
    ) -> Vec<SimilarityHit> {
        if query.is_empty() || limit == 0 || self.ids.is_empty() {
            return Vec::new();
        }
        let mut scores = vec![0f32; self.ids.len()];
        for (dim, weight) in query.dims.iter().zip(query.weights.iter()) {
            let Some(postings) = self.inverted.get(*dim as usize) else {
                continue;
            };
            for (document, value) in postings {
                scores[*document as usize] += weight * value;
            }
        }
        let minimum = minimum_similarity as f32;
        let mut ranked: Vec<(usize, f32)> = scores
            .into_iter()
            .enumerate()
            .filter(|(position, score)| {
                Some(*position) != exclude && *score > minimum && *score > 0.0
            })
            .collect();
        if ranked.len() > limit {
            ranked.select_nth_unstable_by(limit, |left, right| right.1.total_cmp(&left.1));
            ranked.truncate(limit);
        }
        ranked.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| self.ids[left.0].cmp(&self.ids[right.0]))
        });
        ranked
            .into_iter()
            .map(|(position, score)| SimilarityHit {
                id: self.ids[position].clone(),
                similarity: f64::from(score),
                shared_terms: self.explain_vectors(
                    query,
                    &self.vectors[position],
                    REPRESENTATIVE_TERMS_PER_DIMENSION + 2,
                ),
            })
            .collect()
    }

    /// Rank stored memories against a stored memory.
    pub fn nearest_from(
        &self,
        position: usize,
        limit: usize,
        minimum_similarity: f64,
    ) -> Vec<SimilarityHit> {
        if position >= self.vectors.len() {
            return Vec::new();
        }
        self.nearest(
            &self.vectors[position],
            limit,
            minimum_similarity,
            Some(position),
        )
    }

    /// Name the dimensions two stored memories share, most influential first.
    pub fn explain_pair(&self, left: &str, right: &str, limit: usize) -> Vec<String> {
        let (Some(left), Some(right)) = (self.vector(left), self.vector(right)) else {
            return Vec::new();
        };
        self.explain_vectors(left, right, limit)
    }

    /// Name the dimensions two vectors share, most influential first.
    ///
    /// This is the explanation attached to every derived semantic edge: the
    /// vocabulary that carried the similarity, so a reader can judge it.
    pub fn explain_vectors(
        &self,
        left: &SparseVector,
        right: &SparseVector,
        limit: usize,
    ) -> Vec<String> {
        let (mut a, mut b) = (0, 0);
        let mut contributions: Vec<(f32, String)> = Vec::new();
        while a < left.dims.len() && b < right.dims.len() {
            match left.dims[a].cmp(&right.dims[b]) {
                std::cmp::Ordering::Less => a += 1,
                std::cmp::Ordering::Greater => b += 1,
                std::cmp::Ordering::Equal => {
                    let contribution = (left.weights[a] * right.weights[b]).abs();
                    if let Some(term) = self
                        .terms
                        .get(&left.dims[a])
                        .and_then(|terms| terms.first())
                    {
                        contributions.push((contribution, term.clone()));
                    }
                    a += 1;
                    b += 1;
                }
            }
        }
        contributions.sort_by(|left, right| {
            right
                .0
                .total_cmp(&left.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        contributions.dedup_by(|left, right| left.1 == right.1);
        contributions
            .into_iter()
            .take(limit)
            .map(|(_, term)| term)
            .collect()
    }

    /// Persist the vector layer, compressed, next to the other derived state.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut vocabulary: Vec<(String, u32)> = self
            .vocabulary
            .iter()
            .map(|(token, frequency)| (token.clone(), *frequency))
            .collect();
        vocabulary.sort();
        let mut terms: Vec<(u32, Vec<String>)> = self
            .terms
            .iter()
            .map(|(dim, tokens)| (*dim, tokens.clone()))
            .collect();
        terms.sort_by_key(|(dim, _)| *dim);
        let documents = self
            .ids
            .iter()
            .zip(self.vectors.iter())
            .map(|(id, vector)| StoredVector {
                id: id.clone(),
                dims: vector.dims.clone(),
                weights: vector.weights.clone(),
            })
            .collect();
        let stored = StoredIndex {
            version: EMBEDDING_VERSION,
            tokenizer_version: TOKENIZER_VERSION,
            dimensions: self.dimensions,
            fingerprint: self.fingerprint.clone(),
            document_count: self.document_count,
            vocabulary,
            terms,
            documents,
        };
        let json = serde_json::to_vec(&stored).context("could not encode the vector layer")?;
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), COMPRESSION_LEVEL)?;
        encoder.write_all(&json)?;
        let compressed = encoder.finish()?;
        fs::write(path, compressed)?;
        Ok(())
    }

    /// Load the vector layer for `fingerprint`.
    ///
    /// Returns `Ok(None)` when the cache is absent, unreadable, truncated, or
    /// describes a different vault revision. Every one of those cases has the
    /// same correct response — rebuild from Markdown — so none of them is an
    /// error.
    pub fn load(path: &Path, fingerprint: &str, dimensions: usize) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let Ok(bytes) = fs::read(path) else {
            return Ok(None);
        };
        let Ok(mut decoder) = zstd::stream::read::Decoder::new(bytes.as_slice()) else {
            return Ok(None);
        };
        let mut json = Vec::new();
        if decoder.read_to_end(&mut json).is_err() {
            return Ok(None);
        }
        let Ok(stored) = serde_json::from_slice::<StoredIndex>(&json) else {
            return Ok(None);
        };
        if stored.version != EMBEDDING_VERSION
            || stored.tokenizer_version != TOKENIZER_VERSION
            || stored.dimensions != dimensions
            || stored.fingerprint != fingerprint
        {
            return Ok(None);
        }
        let vocabulary: HashMap<String, u32> = stored.vocabulary.iter().cloned().collect();
        let terms: HashMap<u32, Vec<String>> = stored.terms.iter().cloned().collect();
        let ids: Vec<String> = stored
            .documents
            .iter()
            .map(|document| document.id.clone())
            .collect();
        let vectors: Vec<SparseVector> = stored
            .documents
            .into_iter()
            .map(|document| SparseVector {
                dims: document.dims,
                weights: document.weights,
            })
            .collect();
        Ok(Some(Self::assemble(
            stored.dimensions,
            stored.document_count,
            stored.fingerprint,
            vocabulary,
            terms,
            ids,
            vectors,
        )))
    }
}

/// The text an entry contributes to the vector space. The title is repeated and
/// tags are included because both compress the entry; a long body would
/// otherwise drown its own subject line.
fn embedding_text(entry: &Entry) -> String {
    let mut text = String::with_capacity(entry.body.len() + 128);
    text.push_str(&entry.meta.title);
    text.push('\n');
    text.push_str(&entry.meta.title);
    text.push('\n');
    text.push_str(&entry.meta.tags.join(" "));
    text.push('\n');
    text.push_str(&entry.body);
    text
}

fn term_counts(text: &str) -> BTreeMap<String, u32> {
    let mut counts = BTreeMap::new();
    for token in tokenize(text) {
        *counts.entry(token).or_insert(0) += 1;
    }
    counts
}

/// BM25-style inverse document frequency, shifted so every weight is positive.
/// A negative weight would make a shared token *reduce* similarity.
fn inverse_document_frequency(document_count: usize, document_frequency: u32) -> f32 {
    let count = document_count as f32;
    let frequency = document_frequency as f32;
    (1.0 + (count - frequency + 0.5) / (frequency + 0.5)).ln()
}

fn vectorize(
    counts: &BTreeMap<String, u32>,
    vocabulary: &HashMap<String, u32>,
    document_count: usize,
    dimensions: usize,
    drop_unknown: bool,
) -> SparseVector {
    // A BTreeMap keyed by dimension keeps the accumulation order deterministic;
    // summing in hash order would make the last float bits depend on the run.
    let mut accumulated: BTreeMap<u32, f32> = BTreeMap::new();
    for (token, count) in counts {
        let weight = match vocabulary.get(token) {
            Some(frequency) => inverse_document_frequency(document_count, *frequency),
            None if drop_unknown => continue,
            None => 0.0,
        };
        let (dim, sign) = hash_token(token, dimensions);
        *accumulated.entry(dim).or_insert(0.0) += sign * (1.0 + (*count as f32).ln()) * weight;
    }
    let mut dims = Vec::with_capacity(accumulated.len());
    let mut weights = Vec::with_capacity(accumulated.len());
    let mut squares = 0.0f32;
    for (dim, weight) in accumulated {
        if weight.abs() < MINIMUM_WEIGHT {
            continue;
        }
        squares += weight * weight;
        dims.push(dim);
        weights.push(weight);
    }
    let norm = squares.sqrt();
    if norm > 0.0 {
        for weight in weights.iter_mut() {
            *weight /= norm;
        }
    }
    SparseVector { dims, weights }
}

/// Record which tokens live in which hashed dimension so similarity can be
/// explained. Only the most frequent tokens per dimension are kept.
fn representative_terms(
    document_frequency: &HashMap<String, u32>,
    dimensions: usize,
) -> HashMap<u32, Vec<String>> {
    let mut by_dimension: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    for (token, frequency) in document_frequency {
        let (dim, _) = hash_token(token, dimensions);
        by_dimension
            .entry(dim)
            .or_default()
            .push((*frequency, token.clone()));
    }
    by_dimension
        .into_iter()
        .map(|(dimension, mut tokens)| {
            tokens.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
            tokens.truncate(REPRESENTATIVE_TERMS_PER_DIMENSION);
            (
                dimension,
                tokens.into_iter().map(|(_, token)| token).collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{Entry, EntryMeta};
    use chrono::Utc;
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
                source_agents: vec!["test".into()],
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

    fn corpus() -> Vec<Entry> {
        vec![
            entry(
                "a",
                "Chunking strategy",
                "Use 512 token chunks for prose and validate against the corpus.",
                &["rag"],
            ),
            entry(
                "b",
                "Retrieval chunk size",
                "Chunk prose at 512 tokens so retrieval quality stays stable.",
                &["rag"],
            ),
            entry(
                "c",
                "Git conflict resolution",
                "Remote wins, local copy is preserved under the conflict folder.",
                &["git"],
            ),
        ]
    }

    #[test]
    fn related_memories_outrank_unrelated_ones() {
        let index = EmbeddingIndex::build(&corpus(), 8192, "test").unwrap();
        let hits = index.nearest_from(index.position_of("a").unwrap(), 3, 0.0);
        assert_eq!(hits[0].id, "b", "{hits:?}");
        // The absolute value depends on corpus size, so the meaningful property
        // is the separation: the related memory must dominate the unrelated one.
        let unrelated = hits
            .iter()
            .find(|hit| hit.id == "c")
            .map(|hit| hit.similarity)
            .unwrap_or(0.0);
        assert!(
            hits[0].similarity > unrelated + 0.05,
            "{} vs {unrelated}",
            hits[0].similarity
        );
    }

    #[test]
    fn similarity_is_explained_with_vocabulary() {
        let index = EmbeddingIndex::build(&corpus(), 8192, "test").unwrap();
        let hits = index.nearest_from(index.position_of("a").unwrap(), 1, 0.0);
        assert!(
            hits[0].shared_terms.contains(&"chunk".to_string()),
            "plural folding should surface the shared stem: {:?}",
            hits[0].shared_terms
        );
        assert!(
            index.explain_pair("a", "c", 5).is_empty(),
            "unrelated memories share no explaining vocabulary"
        );
    }

    #[test]
    fn query_text_lands_in_the_same_space() {
        let index = EmbeddingIndex::build(&corpus(), 8192, "test").unwrap();
        let query = index.encode("chunk prose tokens");
        let hits = index.nearest(&query, 3, 0.0, None);
        assert!(!hits.is_empty());
        assert_ne!(hits[0].id, "c");
    }

    #[test]
    fn unknown_query_tokens_are_dropped() {
        let index = EmbeddingIndex::build(&corpus(), 8192, "test").unwrap();
        assert!(index.encode("zzzzqqqq").is_empty());
    }

    #[test]
    fn rebuilds_are_byte_identical() {
        let first = EmbeddingIndex::build(&corpus(), 8192, "test").unwrap();
        let second = EmbeddingIndex::build(&corpus(), 8192, "test").unwrap();
        assert_eq!(first.ids(), second.ids());
        for id in first.ids() {
            assert_eq!(first.vector(id), second.vector(id));
        }
    }

    #[test]
    fn round_trips_through_the_cache() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.bin");
        let index = EmbeddingIndex::build(&corpus(), 8192, "fingerprint").unwrap();
        index.save(&path).unwrap();
        let loaded = EmbeddingIndex::load(&path, "fingerprint", 8192)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.ids(), index.ids());
        for id in index.ids() {
            assert_eq!(loaded.vector(id), index.vector(id), "{id}");
        }
        assert_eq!(loaded.nearest_from(0, 1, 0.0)[0].id, "b");
    }

    #[test]
    fn stale_or_missing_cache_is_not_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.bin");
        assert!(
            EmbeddingIndex::load(&path, "fingerprint", 8192)
                .unwrap()
                .is_none()
        );
        let index = EmbeddingIndex::build(&corpus(), 8192, "fingerprint").unwrap();
        index.save(&path).unwrap();
        assert!(
            EmbeddingIndex::load(&path, "other", 8192)
                .unwrap()
                .is_none()
        );
        assert!(
            EmbeddingIndex::load(&path, "fingerprint", 4096)
                .unwrap()
                .is_none()
        );
        std::fs::write(&path, b"not a store").unwrap();
        assert!(
            EmbeddingIndex::load(&path, "fingerprint", 8192)
                .unwrap()
                .is_none()
        );
    }
}
