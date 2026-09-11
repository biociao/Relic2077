//! The logic layer: a typed, evidence-carrying relation network over memories.
//!
//! The vector layer ([`crate::embedding`]) answers "what is *near* this memory".
//! This module answers the questions that need *named* relations: what
//! supersedes what, what contradicts what, what shares a subject, and how two
//! memories are connected through a chain of such relations.
//!
//! Every edge records three things a reader needs in order to trust it:
//!
//! - `kind` — what the relation means;
//! - `derivation` — whether it was `explicit` in the front matter,
//!   `statistical` (set overlap or vector proximity), or `logical` (a conflict
//!   or agreement between claims);
//! - `evidence` — the concrete reason, in words, including the vocabulary that
//!   carried a semantic similarity.
//!
//! Inferred edges are never presented as facts. A contradiction is a signal for
//! the writer to review, not a verdict about which memory is right.
//!
//! # Deliberate omissions
//!
//! Provenance (`source_agents`) is *not* an edge kind. In a single-agent vault
//! every memory shares an agent, so a provenance edge would join everything to
//! everything and destroy the structure the graph is supposed to show. Agent
//! membership stays a filter over nodes, not a fabricated relation.
//!
//! # Cost
//!
//! Node and explicit-edge construction is linear. Tag, corroboration, and
//! contradiction edges enumerate pairs *within a subject tag group*, so their
//! cost is quadratic in the largest group rather than in the vault. Semantic
//! edges come from the inverted index in the vector layer, which visits only the
//! dimensions a memory occupies. A subject group larger than
//! [`MAX_TAG_GROUP_FOR_PAIRS`] is reported in [`Graph::notes`] and left
//! unexpanded rather than silently truncated.

use crate::config::GraphConfig;
use crate::embedding::EmbeddingIndex;
use crate::entry::{Entry, subject_tags};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;

/// On-disk format version for the derived graph.
pub const GRAPH_VERSION: u32 = 1;

/// Above this many memories in one subject group, pairwise enumeration is
/// skipped and recorded in [`Graph::notes`] instead of quietly changing shape.
pub const MAX_TAG_GROUP_FOR_PAIRS: usize = 2048;

/// Below this many memories, inverse document frequency has nothing to compare
/// against and cosine similarities collapse toward zero for every pair. The
/// build still runs; it records a note explaining why semantic edges are sparse
/// rather than leaving the reader to guess.
pub const MINIMUM_CORPUS_FOR_SEMANTICS: usize = 5;

const HUBS_REPORTED: usize = 10;
const STRONGEST_REPORTED: usize = 10;

/// A detected conflict is a heuristic signal, so it weighs less than a relation
/// the writer declared in the front matter.
const CONTRADICTION_WEIGHT: f64 = 0.8;

/// What a relation means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// An explicit `links` reference in the front matter.
    Link,
    /// Explicit version history: the newer memory replaced the older one.
    Supersedes,
    /// Two memories share subject tags, weighted by Jaccard overlap.
    Tag,
    /// Vector-space neighbours above the configured cosine floor.
    Semantic,
    /// Opposite claims about a shared subject.
    Contradicts,
    /// Matching claims about a shared subject, close in the vector space.
    Corroborates,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Link => "link",
            Self::Supersedes => "supersedes",
            Self::Tag => "tag",
            Self::Semantic => "semantic",
            Self::Contradicts => "contradicts",
            Self::Corroborates => "corroborates",
        }
    }

    pub fn derivation(self) -> Derivation {
        match self {
            Self::Link | Self::Supersedes => Derivation::Explicit,
            Self::Tag | Self::Semantic => Derivation::Statistical,
            Self::Contradicts | Self::Corroborates => Derivation::Logical,
        }
    }

    /// Relations that state a fact about the vault rather than an inference.
    /// These are never pruned by the per-node edge budget.
    pub fn is_load_bearing(self) -> bool {
        matches!(self, Self::Link | Self::Supersedes | Self::Contradicts)
    }

    /// Symmetric relations are stored once, with the lexicographically smaller
    /// endpoint first, so a pair never appears twice in mirrored form.
    pub fn is_symmetric(self) -> bool {
        matches!(
            self,
            Self::Tag | Self::Semantic | Self::Contradicts | Self::Corroborates
        )
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "link" => Some(Self::Link),
            "supersedes" => Some(Self::Supersedes),
            "tag" => Some(Self::Tag),
            "semantic" => Some(Self::Semantic),
            "contradicts" => Some(Self::Contradicts),
            "corroborates" => Some(Self::Corroborates),
            _ => None,
        }
    }

    pub const ALL: [EdgeKind; 6] = [
        Self::Link,
        Self::Supersedes,
        Self::Tag,
        Self::Semantic,
        Self::Contradicts,
        Self::Corroborates,
    ];
}

/// How much inference an edge relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Derivation {
    /// Declared by the writer in the Markdown front matter.
    Explicit,
    /// Derived from set overlap or vector proximity.
    Statistical,
    /// Derived from comparing what two memories claim about one subject.
    Logical,
}

impl Derivation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Statistical => "statistical",
            Self::Logical => "logical",
        }
    }
}

/// One relation between two memories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub derivation: Derivation,
    /// Strength in `(0, 1]`. Edge weights multiply along a traversal, so a
    /// weaker hop always lowers the score of the whole path.
    pub weight: f64,
    pub evidence: String,
}

/// One memory in the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub confidence: f64,
    pub tags: Vec<String>,
    /// Number of incident edges.
    pub degree: usize,
}

/// The derived relation network for one vault revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    pub version: u32,
    pub built_at: DateTime<Utc>,
    /// Hash of the Markdown the graph was derived from. A mismatch means the
    /// graph is stale and must be rebuilt.
    pub fingerprint: String,
    pub dimensions: usize,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Honest record of any bounded or skipped derivation.
    pub notes: Vec<String>,
}

impl Graph {
    /// Derive the graph from entries and their vector space.
    ///
    /// Deterministic: entries are processed in stable ID order, symmetric edges
    /// are canonicalised, and the final edge list is sorted, so two builds over
    /// identical Markdown produce identical output.
    pub fn build(
        entries: &[Entry],
        embeddings: &EmbeddingIndex,
        config: &GraphConfig,
        fingerprint: &str,
    ) -> Graph {
        let mut ordered: Vec<&Entry> = entries.iter().collect();
        ordered.sort_by(|left, right| left.meta.id.cmp(&right.meta.id));
        let known: HashSet<&str> = ordered.iter().map(|entry| entry.meta.id.as_str()).collect();

        let mut edges = Vec::new();
        let mut notes = Vec::new();

        if ordered.len() < MINIMUM_CORPUS_FOR_SEMANTICS {
            notes.push(format!(
                "only {} memories: inverse document frequency needs a larger corpus to separate subjects, so few semantic edges are expected until the vault grows",
                ordered.len()
            ));
        }

        collect_explicit_edges(&ordered, &known, &mut edges);
        collect_tag_edges(&ordered, config, &mut edges, &mut notes);
        collect_semantic_edges(&ordered, embeddings, config, &mut edges);
        collect_claim_edges(&ordered, embeddings, config, &mut edges);
        deduplicate(&mut edges);
        prune_to_budget(&mut edges, config.max_edges_per_node, &mut notes);

        let mut nodes = build_nodes(&ordered, &edges);
        nodes.sort_by(|left, right| left.id.cmp(&right.id));
        edges.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.from.cmp(&right.from))
                .then_with(|| left.to.cmp(&right.to))
        });

        Graph {
            version: GRAPH_VERSION,
            built_at: Utc::now(),
            fingerprint: fingerprint.to_owned(),
            dimensions: embeddings.dimensions(),
            nodes,
            edges,
            notes,
        }
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn has_node(&self, id: &str) -> bool {
        self.node(id).is_some()
    }

    /// Every edge that directly joins two memories, in either direction.
    pub fn explain(&self, from: &str, to: &str) -> Vec<Edge> {
        self.edges
            .iter()
            .filter(|edge| {
                (edge.from == from && edge.to == to) || (edge.from == to && edge.to == from)
            })
            .cloned()
            .collect()
    }

    fn adjacency(&self) -> HashMap<&str, Vec<(&Edge, &str)>> {
        let mut adjacency: HashMap<&str, Vec<(&Edge, &str)>> = HashMap::new();
        for edge in &self.edges {
            adjacency
                .entry(edge.from.as_str())
                .or_default()
                .push((edge, edge.to.as_str()));
            adjacency
                .entry(edge.to.as_str())
                .or_default()
                .push((edge, edge.from.as_str()));
        }
        adjacency
    }

    /// Everything reachable from one memory within `depth` hops.
    ///
    /// A node's score is the product of edge weights along its best path, so a
    /// chain of weak relations ranks below a single strong one. `minimum_weight`
    /// drops hops below a floor, which is how a caller asks for the backbone
    /// rather than the full hairball.
    pub fn neighborhood(
        &self,
        from: &str,
        depth: usize,
        minimum_weight: f64,
        kinds: Option<&[EdgeKind]>,
    ) -> Result<Vec<NeighborHit>> {
        if !self.has_node(from) {
            bail!("entry '{from}' is not in the knowledge graph");
        }
        let adjacency = self.adjacency();
        // Best (score, depth, predecessor, edge) per reached node. Relaxation
        // continues while a better multiplicative path is found; because every
        // weight is at most 1 the score strictly decreases, so this terminates.
        let mut best: HashMap<&str, (f64, usize, &str, &Edge)> = HashMap::new();
        let mut queue: VecDeque<(&str, f64, usize)> = VecDeque::new();
        queue.push_back((from, 1.0, 0));

        while let Some((current, score, current_depth)) = queue.pop_front() {
            if current_depth >= depth {
                continue;
            }
            let Some(neighbors) = adjacency.get(current) else {
                continue;
            };
            for (edge, other) in neighbors {
                if edge.weight < minimum_weight {
                    continue;
                }
                if kinds.is_some_and(|allowed| !allowed.contains(&edge.kind)) {
                    continue;
                }
                if *other == from || *other == current {
                    continue;
                }
                let candidate = score * edge.weight;
                let next_depth = current_depth + 1;
                let improves = match best.get(other) {
                    Some((existing, _, _, _)) => candidate > *existing,
                    None => true,
                };
                if improves {
                    best.insert(other, (candidate, next_depth, current, edge));
                    queue.push_back((other, candidate, next_depth));
                }
            }
        }

        let mut hits: Vec<NeighborHit> = best
            .into_iter()
            .map(|(id, (score, depth, via, edge))| NeighborHit {
                id: id.to_owned(),
                title: self
                    .node(id)
                    .map(|node| node.title.clone())
                    .unwrap_or_default(),
                depth,
                score,
                via: via.to_owned(),
                kind: edge.kind,
                weight: edge.weight,
                evidence: edge.evidence.clone(),
            })
            .collect();
        hits.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| right.score.total_cmp(&left.score))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(hits)
    }

    /// The strongest chain between two memories.
    ///
    /// "Strongest" is the widest path: the route that maximises the *weakest*
    /// hop, so one confidently unrelated step cannot be hidden behind several
    /// strong ones. Edges below `minimum_weight` are not traversed.
    pub fn path(&self, from: &str, to: &str, minimum_weight: f64) -> Result<Option<GraphPath>> {
        if !self.has_node(from) {
            bail!("entry '{from}' is not in the knowledge graph");
        }
        if !self.has_node(to) {
            bail!("entry '{to}' is not in the knowledge graph");
        }
        if from == to {
            return Ok(Some(GraphPath {
                nodes: vec![from.to_owned()],
                edges: Vec::new(),
                bottleneck: 1.0,
            }));
        }
        let adjacency = self.adjacency();
        let mut bottleneck: HashMap<&str, f64> = HashMap::new();
        let mut previous: HashMap<String, (&Edge, String)> = HashMap::new();
        let mut heap: BinaryHeap<Candidate> = BinaryHeap::new();
        bottleneck.insert(from, 1.0);
        heap.push(Candidate {
            score: 1.0,
            id: from.to_owned(),
        });

        while let Some(Candidate { score, id }) = heap.pop() {
            if id == to {
                break;
            }
            if score < bottleneck.get(id.as_str()).copied().unwrap_or(0.0) {
                continue;
            }
            let Some(neighbors) = adjacency.get(id.as_str()) else {
                continue;
            };
            for (edge, other) in neighbors {
                if edge.weight < minimum_weight {
                    continue;
                }
                let candidate = score.min(edge.weight);
                if candidate > bottleneck.get(other).copied().unwrap_or(0.0) {
                    bottleneck.insert(other, candidate);
                    previous.insert((*other).to_owned(), (*edge, id.clone()));
                    heap.push(Candidate {
                        score: candidate,
                        id: (*other).to_owned(),
                    });
                }
            }
        }

        if !bottleneck.contains_key(to) {
            return Ok(None);
        }
        let mut edges: Vec<Edge> = Vec::new();
        let mut nodes: Vec<String> = vec![to.to_owned()];
        let mut cursor = to;
        while let Some((edge, parent)) = previous.get(cursor) {
            edges.push((*edge).clone());
            nodes.push(parent.clone());
            cursor = parent;
        }
        edges.reverse();
        nodes.reverse();
        Ok(Some(GraphPath {
            nodes,
            edges,
            bottleneck: bottleneck.get(to).copied().unwrap_or(0.0),
        }))
    }

    /// Connected components, ignoring edges weaker than `minimum_weight`.
    ///
    /// A high floor reveals the articulation of the vault; the default reveals
    /// how many genuinely separate bodies of knowledge it holds.
    pub fn components(&self, minimum_weight: f64) -> Vec<Vec<String>> {
        let position: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        let mut sets = DisjointSet::new(self.nodes.len());
        for edge in &self.edges {
            if edge.weight < minimum_weight {
                continue;
            }
            if let (Some(left), Some(right)) = (
                position.get(edge.from.as_str()),
                position.get(edge.to.as_str()),
            ) {
                sets.union(*left, *right);
            }
        }
        let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for (index, node) in self.nodes.iter().enumerate() {
            grouped
                .entry(sets.find(index))
                .or_default()
                .push(node.id.clone());
        }
        let mut components: Vec<Vec<String>> = grouped.into_values().collect();
        components.sort_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left.first().cmp(&right.first()))
        });
        components
    }

    pub fn stats(&self, minimum_weight: f64) -> GraphStats {
        let mut edges_by_kind: BTreeMap<String, usize> = BTreeMap::new();
        let mut edges_by_derivation: BTreeMap<String, usize> = BTreeMap::new();
        let mut weighted_degree: BTreeMap<&str, f64> = BTreeMap::new();
        for edge in &self.edges {
            *edges_by_kind
                .entry(edge.kind.as_str().to_owned())
                .or_insert(0) += 1;
            *edges_by_derivation
                .entry(edge.derivation.as_str().to_owned())
                .or_insert(0) += 1;
            *weighted_degree.entry(edge.from.as_str()).or_insert(0.0) += edge.weight;
            *weighted_degree.entry(edge.to.as_str()).or_insert(0.0) += edge.weight;
        }
        let components = self.components(minimum_weight);
        let nodes = self.nodes.len();
        let mut hubs: Vec<Hub> = self
            .nodes
            .iter()
            .map(|node| Hub {
                id: node.id.clone(),
                title: node.title.clone(),
                degree: node.degree,
                weighted_degree: weighted_degree
                    .get(node.id.as_str())
                    .copied()
                    .unwrap_or(0.0),
            })
            .filter(|hub| hub.degree > 0)
            .collect();
        hubs.sort_by(|left, right| {
            right
                .degree
                .cmp(&left.degree)
                .then_with(|| right.weighted_degree.total_cmp(&left.weighted_degree))
                .then_with(|| left.id.cmp(&right.id))
        });
        hubs.truncate(HUBS_REPORTED);

        let mut strongest = self.edges.clone();
        strongest.sort_by(|left, right| {
            right
                .weight
                .total_cmp(&left.weight)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.from.cmp(&right.from))
                .then_with(|| left.to.cmp(&right.to))
        });
        strongest.truncate(STRONGEST_REPORTED);

        let orphans: Vec<String> = self
            .nodes
            .iter()
            .filter(|node| node.degree == 0)
            .map(|node| node.id.clone())
            .collect();

        GraphStats {
            nodes,
            edges: self.edges.len(),
            dimensions: self.dimensions,
            density: if nodes < 2 {
                0.0
            } else {
                self.edges.len() as f64 / (nodes as f64 * (nodes as f64 - 1.0) / 2.0)
            },
            mean_degree: if nodes == 0 {
                0.0
            } else {
                self.nodes.iter().map(|node| node.degree).sum::<usize>() as f64 / nodes as f64
            },
            components: components.len(),
            largest_component: components.first().map(Vec::len).unwrap_or(0),
            isolated: orphans.len(),
            edges_by_kind,
            edges_by_derivation,
            hubs,
            orphans,
            strongest,
            notes: self.notes.clone(),
        }
    }

    /// Graphviz output. Edge labels carry the relation kind and weight so the
    /// picture states what each line means.
    pub fn to_dot(&self, minimum_weight: f64) -> String {
        let mut output = String::from("digraph relic {\n  rankdir=LR;\n  node [shape=box];\n");
        for node in &self.nodes {
            output.push_str(&format!(
                "  \"{}\" [label=\"{}\\n({})\"];\n",
                escape(&node.id),
                escape(&node.title),
                escape(&node.kind)
            ));
        }
        for edge in &self.edges {
            if edge.weight < minimum_weight {
                continue;
            }
            output.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{} {:.2}\"];\n",
                escape(&edge.from),
                escape(&edge.to),
                edge.kind.as_str(),
                edge.weight
            ));
        }
        output.push_str("}\n");
        output
    }

    /// Mermaid output, which renders in GitHub, Obsidian, and most editors.
    pub fn to_mermaid(&self, minimum_weight: f64) -> String {
        let mut output = String::from("graph LR\n");
        for (index, node) in self.nodes.iter().enumerate() {
            output.push_str(&format!(
                "  n{index}[\"{}\"]\n",
                escape(&format!("{} ({})", node.title, node.kind))
            ));
        }
        let positions: HashMap<&str, usize> = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id.as_str(), index))
            .collect();
        for edge in &self.edges {
            if edge.weight < minimum_weight {
                continue;
            }
            if let (Some(left), Some(right)) = (
                positions.get(edge.from.as_str()),
                positions.get(edge.to.as_str()),
            ) {
                output.push_str(&format!(
                    "  n{left} -- \"{} {:.2}\" --> n{right}\n",
                    edge.kind.as_str(),
                    edge.weight
                ));
            }
        }
        output
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Load a graph for `fingerprint`.
    ///
    /// Returns `Ok(None)` when the file is absent, unreadable, or describes a
    /// different vault revision: every one of those cases means "rebuild", which
    /// is not an error for a derived artifact.
    pub fn load(path: &Path, fingerprint: &str) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let Ok(content) = fs::read_to_string(path) else {
            return Ok(None);
        };
        let Ok(graph) = serde_json::from_str::<Graph>(&content) else {
            return Ok(None);
        };
        if graph.version != GRAPH_VERSION || graph.fingerprint != fingerprint {
            return Ok(None);
        }
        Ok(Some(graph))
    }
}

/// Content hash of everything in the vault that the derived layers depend on.
///
/// Deliberately covers whole entries rather than only their timestamps: a hand
/// edit that forgets to bump `updated` must still invalidate the cache.
pub fn fingerprint(entries: &[Entry]) -> String {
    let mut digests: Vec<(String, String)> = entries
        .iter()
        .map(|entry| (entry.meta.id.clone(), entry.content_hash()))
        .collect();
    digests.sort();
    let mut hasher = Sha256::new();
    for (id, digest) in digests {
        hasher.update(id.as_bytes());
        hasher.update(b"\0");
        hasher.update(digest.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn collect_explicit_edges(entries: &[&Entry], known: &HashSet<&str>, edges: &mut Vec<Edge>) {
    for entry in entries {
        let from = entry.meta.id.clone();
        for target in &entry.meta.links {
            if target != &from && known.contains(target.as_str()) {
                edges.push(Edge {
                    from: from.clone(),
                    to: target.clone(),
                    kind: EdgeKind::Link,
                    derivation: Derivation::Explicit,
                    weight: 1.0,
                    evidence: "declared in the entry's links list".into(),
                });
            }
        }
        for target in &entry.meta.supersedes {
            if target != &from && known.contains(target.as_str()) {
                edges.push(Edge {
                    from: from.clone(),
                    to: target.clone(),
                    kind: EdgeKind::Supersedes,
                    derivation: Derivation::Explicit,
                    weight: 1.0,
                    evidence: "declared version history: this memory replaces the older one".into(),
                });
            }
        }
        if let Some(target) = &entry.meta.superseded_by
            && target != &from
            && known.contains(target.as_str())
        {
            edges.push(Edge {
                from: from.clone(),
                to: target.clone(),
                kind: EdgeKind::Supersedes,
                derivation: Derivation::Explicit,
                weight: 1.0,
                evidence: "declared version history: this memory was replaced by the newer one"
                    .into(),
            });
        }
    }
}

fn collect_tag_edges(
    entries: &[&Entry],
    config: &GraphConfig,
    edges: &mut Vec<Edge>,
    notes: &mut Vec<String>,
) {
    let sets: Vec<BTreeSet<&str>> = entries
        .iter()
        .map(|entry| subject_tags(&entry.meta.tags).map(String::as_str).collect())
        .collect();
    let mut by_tag: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, set) in sets.iter().enumerate() {
        for tag in set {
            by_tag.entry(tag).or_default().push(index);
        }
    }
    for (tag, group) in by_tag {
        if group.len() < 2 {
            continue;
        }
        if group.len() > MAX_TAG_GROUP_FOR_PAIRS {
            notes.push(format!(
                "subject '{tag}' has {} members, above the {MAX_TAG_GROUP_FOR_PAIRS} pair limit; tag edges were not expanded for it",
                group.len()
            ));
            continue;
        }
        for left in 0..group.len() {
            for right in (left + 1)..group.len() {
                let (a, b) = (group[left], group[right]);
                let (overlap, shared) = jaccard(&sets[a], &sets[b]);
                if overlap < config.tag_min_jaccard {
                    continue;
                }
                let union = sets[a].union(&sets[b]).count();
                edges.push(Edge {
                    from: entries[a].meta.id.clone(),
                    to: entries[b].meta.id.clone(),
                    kind: EdgeKind::Tag,
                    derivation: Derivation::Statistical,
                    weight: overlap,
                    evidence: format!(
                        "shares {} of {union} subject tags: {}",
                        shared.len(),
                        shared.join(", ")
                    ),
                });
            }
        }
    }
}

fn collect_semantic_edges(
    entries: &[&Entry],
    embeddings: &EmbeddingIndex,
    config: &GraphConfig,
    edges: &mut Vec<Edge>,
) {
    // Canonicalise by unordered pair so the two directions of a mutual nearest
    // neighbour collapse into the single strongest edge, with its explanation.
    let mut strongest: BTreeMap<(String, String), Edge> = BTreeMap::new();
    for entry in entries {
        let Some(position) = embeddings.position_of(&entry.meta.id) else {
            continue;
        };
        for hit in embeddings.nearest_from(
            position,
            config.semantic_top_k,
            config.semantic_min_similarity,
        ) {
            let (left, right) = ordered_pair(&entry.meta.id, &hit.id);
            let candidate = Edge {
                from: left.clone(),
                to: right.clone(),
                kind: EdgeKind::Semantic,
                derivation: Derivation::Statistical,
                weight: hit.similarity,
                evidence: format!(
                    "cosine {:.3} in {} hashed dimensions; nearest vocabulary: {}",
                    hit.similarity,
                    embeddings.dimensions(),
                    if hit.shared_terms.is_empty() {
                        "none".to_owned()
                    } else {
                        hit.shared_terms.join(", ")
                    }
                ),
            };
            strongest
                .entry((left, right))
                .and_modify(|existing| {
                    if candidate.weight > existing.weight {
                        *existing = candidate.clone();
                    }
                })
                .or_insert(candidate);
        }
    }
    edges.extend(strongest.into_values());
}

fn collect_claim_edges(
    entries: &[&Entry],
    embeddings: &EmbeddingIndex,
    config: &GraphConfig,
    edges: &mut Vec<Edge>,
) {
    let bodies: HashMap<&str, String> = entries
        .iter()
        .map(|entry| {
            (
                entry.meta.id.as_str(),
                format!("{} {}", entry.meta.title, entry.body),
            )
        })
        .collect();

    let owned: Vec<Entry> = entries.iter().map(|entry| (*entry).clone()).collect();
    for contradiction in crate::analysis::detect_contradictions(&owned) {
        let (left, right) = ordered_pair(&contradiction.a, &contradiction.b);
        edges.push(Edge {
            from: left,
            to: right,
            kind: EdgeKind::Contradicts,
            derivation: Derivation::Logical,
            weight: CONTRADICTION_WEIGHT,
            evidence: format!(
                "{}; opposite claim polarity ({:+} vs {:+}) — a review signal, not a verdict",
                contradiction.reason, contradiction.a_polarity, contradiction.b_polarity
            ),
        });
    }

    let mut by_tag: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        for tag in &entry.meta.tags {
            by_tag.entry(tag.as_str()).or_default().push(index);
        }
    }
    for (tag, group) in by_tag {
        if group.len() < 2 || group.len() > MAX_TAG_GROUP_FOR_PAIRS {
            continue;
        }
        for left in 0..group.len() {
            for right in (left + 1)..group.len() {
                let (a, b) = (group[left], group[right]);
                let (Some(a_body), Some(b_body)) = (
                    bodies.get(entries[a].meta.id.as_str()),
                    bodies.get(entries[b].meta.id.as_str()),
                ) else {
                    continue;
                };
                let (a_body, b_body) = (a_body, b_body);
                let (a_claim, b_claim) = (
                    crate::analysis::claim(a_body),
                    crate::analysis::claim(b_body),
                );
                // Both sides must actually state a claim, in the same direction,
                // before agreement is worth recording as a relation.
                if !a_claim.is_decisive(crate::analysis::CLAIM_MINIMUM_STRENGTH)
                    || !b_claim.is_decisive(crate::analysis::CLAIM_MINIMUM_STRENGTH)
                    || a_claim.net() != b_claim.net()
                {
                    continue;
                }
                let a_polarity = a_claim.net();
                let Some(similarity) = embeddings.cosine(&entries[a].meta.id, &entries[b].meta.id)
                else {
                    continue;
                };
                if similarity < config.corroborate_min_similarity {
                    continue;
                }
                let terms = embeddings.explain_pair(
                    &entries[a].meta.id,
                    &entries[b].meta.id,
                    crate::text::REPRESENTATIVE_TERMS_PER_DIMENSION + 1,
                );
                let (from, to) = ordered_pair(&entries[a].meta.id, &entries[b].meta.id);
                edges.push(Edge {
                    from,
                    to,
                    kind: EdgeKind::Corroborates,
                    derivation: Derivation::Logical,
                    weight: similarity,
                    evidence: format!(
                        "same subject '{tag}' with matching {} claim; cosine {similarity:.3}; nearest vocabulary: {}",
                        if a_polarity > 0 { "positive" } else { "negative" },
                        if terms.is_empty() {
                            "none".to_owned()
                        } else {
                            terms.join(", ")
                        }
                    ),
                });
            }
        }
    }
}

/// Keep the strongest edge for each `(kind, from, to)`, canonicalising the
/// direction of symmetric kinds.
fn deduplicate(edges: &mut Vec<Edge>) {
    let mut unique: BTreeMap<(EdgeKind, String, String), Edge> = BTreeMap::new();
    for mut edge in edges.drain(..) {
        if edge.kind.is_symmetric() && edge.from > edge.to {
            std::mem::swap(&mut edge.from, &mut edge.to);
        }
        let key = (edge.kind, edge.from.clone(), edge.to.clone());
        unique
            .entry(key)
            .and_modify(|existing| {
                if edge.weight > existing.weight {
                    *existing = edge.clone();
                }
            })
            .or_insert(edge);
    }
    edges.extend(unique.into_values());
}

/// Apply the per-node budget to inferred edges, strongest first.
///
/// A single pass cannot work: keeping an edge because *either* endpoint wants it
/// lets a hub accumulate one edge from every neighbour. Instead this is a greedy
/// loop that drops the weakest surviving inferred edge of every node that is
/// still over budget, and repeats until no node is. Each round removes at least
/// one edge, so it terminates, and in practice it converges in a few rounds.
///
/// Declared links, version history, and contradictions are never pruned, so a
/// node's final degree is its budget plus however many of those it carries.
fn prune_to_budget(edges: &mut Vec<Edge>, budget: usize, notes: &mut Vec<String>) {
    if edges.is_empty() || budget == 0 {
        return;
    }
    let total = edges.len();
    let mut removed: HashSet<usize> = HashSet::new();
    loop {
        // Incident inferred edges per node, weakest last so `pop` is the victim.
        let mut incidents: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, edge) in edges.iter().enumerate() {
            if edge.kind.is_load_bearing() || removed.contains(&index) {
                continue;
            }
            incidents.entry(edge.from.as_str()).or_default().push(index);
            incidents.entry(edge.to.as_str()).or_default().push(index);
        }
        let mut victims: Vec<usize> = Vec::new();
        for indices in incidents.values_mut() {
            if indices.len() <= budget {
                continue;
            }
            indices.sort_by(|left, right| {
                edges[*left]
                    .weight
                    .total_cmp(&edges[*right].weight)
                    .then_with(|| edges[*right].kind.cmp(&edges[*left].kind))
                    .then_with(|| edges[*right].from.cmp(&edges[*left].from))
                    .then_with(|| edges[*right].to.cmp(&edges[*left].to))
            });
            // Drop the excess, weakest first.
            for index in indices.iter().take(indices.len() - budget) {
                victims.push(*index);
            }
        }
        if victims.is_empty() {
            break;
        }
        for index in victims {
            removed.insert(index);
        }
    }
    if removed.is_empty() {
        return;
    }
    let mut index = 0;
    edges.retain(|_| {
        let retained = !removed.contains(&index);
        index += 1;
        retained
    });
    notes.push(format!(
        "pruned {} inferred edge(s) beyond the per-memory budget of {budget}; declared links, version history, and contradictions were kept",
        total - edges.len()
    ));
}

fn build_nodes(entries: &[&Entry], edges: &[Edge]) -> Vec<Node> {
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for edge in edges {
        *degree.entry(edge.from.as_str()).or_insert(0) += 1;
        *degree.entry(edge.to.as_str()).or_insert(0) += 1;
    }
    entries
        .iter()
        .map(|entry| Node {
            id: entry.meta.id.clone(),
            title: entry.meta.title.clone(),
            kind: entry.meta.kind.clone(),
            status: entry.meta.status.clone(),
            confidence: entry.meta.effective_confidence(),
            tags: entry.meta.tags.clone(),
            degree: degree.get(entry.meta.id.as_str()).copied().unwrap_or(0),
        })
        .collect()
}

fn ordered_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_owned(), right.to_owned())
    } else {
        (right.to_owned(), left.to_owned())
    }
}

/// Jaccard overlap of two subject sets, plus the shared tags for evidence.
fn jaccard(left: &BTreeSet<&str>, right: &BTreeSet<&str>) -> (f64, Vec<String>) {
    if left.is_empty() || right.is_empty() {
        return (0.0, Vec::new());
    }
    let shared: Vec<String> = left
        .intersection(right)
        .map(|tag| (*tag).to_owned())
        .collect();
    let union = left.union(right).count();
    (shared.len() as f64 / union as f64, shared)
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// One memory reached from another, with the step that reached it.
#[derive(Debug, Clone, Serialize)]
pub struct NeighborHit {
    pub id: String,
    pub title: String,
    /// Hops from the origin.
    pub depth: usize,
    /// Product of the edge weights along the best path.
    pub score: f64,
    /// The memory the winning step came from.
    pub via: String,
    pub kind: EdgeKind,
    pub weight: f64,
    pub evidence: String,
}

/// A chain of relations between two memories.
#[derive(Debug, Clone, Serialize)]
pub struct GraphPath {
    pub nodes: Vec<String>,
    pub edges: Vec<Edge>,
    /// The weakest hop on the route: the confidence of the whole chain.
    pub bottleneck: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hub {
    pub id: String,
    pub title: String,
    pub degree: usize,
    pub weighted_degree: f64,
}

/// A health summary of the relation network.
#[derive(Debug, Clone, Serialize)]
pub struct GraphStats {
    pub nodes: usize,
    pub edges: usize,
    pub dimensions: usize,
    pub density: f64,
    pub mean_degree: f64,
    pub components: usize,
    pub largest_component: usize,
    pub isolated: usize,
    pub edges_by_kind: BTreeMap<String, usize>,
    pub edges_by_derivation: BTreeMap<String, usize>,
    pub hubs: Vec<Hub>,
    pub orphans: Vec<String>,
    pub strongest: Vec<Edge>,
    pub notes: Vec<String>,
}

#[derive(PartialEq)]
struct Candidate {
    score: f64,
    id: String,
}

impl Eq for Candidate {}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new(size: usize) -> Self {
        Self {
            parent: (0..size).collect(),
            rank: vec![0; size],
        }
    }

    fn find(&mut self, mut node: usize) -> usize {
        while self.parent[node] != node {
            self.parent[node] = self.parent[self.parent[node]];
            node = self.parent[node];
        }
        node
    }

    fn union(&mut self, left: usize, right: usize) {
        let (left_root, right_root) = (self.find(left), self.find(right));
        if left_root == right_root {
            return;
        }
        match self.rank[left_root].cmp(&self.rank[right_root]) {
            Ordering::Less => self.parent[left_root] = right_root,
            Ordering::Greater => self.parent[right_root] = left_root,
            Ordering::Equal => {
                self.parent[right_root] = left_root;
                self.rank[left_root] += 1;
            }
        }
    }
}

/// Read a comma-separated relation filter from a command line or tool call.
pub fn parse_kinds(values: &[String]) -> Result<Option<Vec<EdgeKind>>> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut kinds = Vec::new();
    for value in values {
        for part in value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let kind =
                EdgeKind::parse(part).with_context(|| format!("unknown relation kind '{part}'"))?;
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    Ok(Some(kinds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryMeta;
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

    fn build(entries: &[Entry]) -> Graph {
        let index = EmbeddingIndex::build(entries, 8192, "test").unwrap();
        Graph::build(entries, &index, &GraphConfig::default(), "test")
    }

    /// Unrelated memories, so the vector layer has the corpus it needs for
    /// inverse document frequency to separate subjects. A vault of two entries
    /// cannot support semantic similarity, and pretending otherwise would make
    /// these tests pass for the wrong reason.
    fn background() -> Vec<Entry> {
        vec![
            entry(
                "bg-git",
                "Git conflict resolution policy",
                "When a merge conflicts the remote version becomes canonical and the local copy is preserved under a conflict folder.",
                &["git"],
            ),
            entry(
                "bg-travel",
                "Alpine travel packing list",
                "Bring layered clothing, a waterproof shell, and broken in boots for the hut.",
                &["life"],
            ),
            entry(
                "bg-decay",
                "Confidence decays from verification",
                "Effective confidence multiplies the stored value by an exponential decay from the last verification date.",
                &["memory"],
            ),
            entry(
                "bg-billing",
                "Invoice rounding rules",
                "Round each invoice line to two decimals and reconcile the total against the ledger.",
                &["finance"],
            ),
        ]
    }

    #[test]
    fn explicit_links_and_version_history_become_edges() {
        let mut old = entry("old", "Old", "Superseded knowledge about chunking.", &["x"]);
        old.meta.superseded_by = Some("new".into());
        let mut new = entry(
            "new",
            "New",
            "Replacement knowledge about chunking.",
            &["x"],
        );
        new.meta.supersedes = vec!["old".into()];
        new.meta.links = vec!["old".into(), "missing".into()];
        let graph = build(&[old, new]);
        assert!(
            graph
                .explain("new", "old")
                .iter()
                .any(|edge| edge.kind == EdgeKind::Supersedes)
        );
        assert!(
            graph
                .explain("new", "old")
                .iter()
                .any(|edge| edge.kind == EdgeKind::Link)
        );
        assert_eq!(graph.nodes.len(), 2, "links to unknown ids create no node");
    }

    #[test]
    fn related_memories_get_a_semantic_edge_with_vocabulary() {
        let graph = build(&[
            entry(
                "a",
                "Chunking strategy",
                "Use 512 token chunks for prose.",
                &["rag"],
            ),
            entry(
                "b",
                "Retrieval chunk size",
                "Chunk prose at 512 tokens for retrieval.",
                &["rag"],
            ),
            entry(
                "c",
                "Git conflict",
                "Remote wins, local is preserved.",
                &["git"],
            ),
        ]);
        let semantic = graph
            .explain("a", "b")
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::Semantic)
            .expect("semantic edge");
        assert_eq!(semantic.derivation, Derivation::Statistical);
        assert!(semantic.evidence.contains("cosine"));
        assert!(
            !graph
                .explain("a", "c")
                .iter()
                .any(|edge| edge.kind == EdgeKind::Semantic)
        );
    }

    #[test]
    fn opposite_claims_become_a_contradiction() {
        let graph = build(&[
            entry(
                "a",
                "Chunking works",
                "The 512 chunk strategy is effective and reliable.",
                &["rag"],
            ),
            entry(
                "b",
                "Chunking fails",
                "The 512 chunk strategy is ineffective and broken.",
                &["rag"],
            ),
        ]);
        let contradiction = graph
            .explain("a", "b")
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::Contradicts)
            .expect("contradiction edge");
        assert_eq!(contradiction.derivation, Derivation::Logical);
        assert!(contradiction.evidence.contains("review signal"));
    }

    #[test]
    fn matching_claims_corroborate() {
        let graph = build(&[
            entry(
                "a",
                "Chunking works",
                "The 512 chunk strategy is effective and reliable for prose.",
                &["rag"],
            ),
            entry(
                "b",
                "Chunking is effective",
                "The 512 chunk strategy is effective and reliable for retrieval.",
                &["rag"],
            ),
        ]);
        assert!(
            graph
                .explain("a", "b")
                .iter()
                .any(|edge| edge.kind == EdgeKind::Corroborates),
            "{:?}",
            graph.edges
        );
    }

    #[test]
    fn neighbourhood_respects_depth_and_weight() {
        // c shares no subject with a, so the only route to it is a's neighbour
        // b, which declares an explicit link. That makes depth meaningful.
        let mut middle = entry(
            "b",
            "Bridge memory about chunking",
            "Bridge memory about chunking prose.",
            &["rag"],
        );
        middle.meta.links = vec!["c".into()];
        let graph = build(&[
            entry("a", "Start", "Start memory about chunking prose.", &["rag"]),
            middle,
            entry(
                "c",
                "Alpine packing",
                "Waterproof gloves and layered clothing for the hut.",
                &["life"],
            ),
        ]);
        let near = graph.neighborhood("a", 1, 0.0, None).unwrap();
        assert!(near.iter().all(|hit| hit.depth == 1));
        assert!(!near.iter().any(|hit| hit.id == "c"));
        let far = graph.neighborhood("a", 2, 0.0, None).unwrap();
        assert!(
            far.iter().any(|hit| hit.id == "c" && hit.depth == 2),
            "{far:?}"
        );
        let only_links = graph
            .neighborhood("a", 2, 0.0, Some(&[EdgeKind::Link]))
            .unwrap();
        assert!(only_links.is_empty(), "{only_links:?}");
    }

    #[test]
    fn path_uses_the_strongest_chain() {
        // Different subject tags leave the semantic relation as the only way
        // across, so the route provably comes from the vector layer.
        let mut entries = vec![
            entry(
                "a",
                "Chunking strategy for retrieval",
                "Use 512 token chunks for prose documents and validate the split against the corpus. Overlapping chunks preserve context across a boundary.",
                &["rag"],
            ),
            entry(
                "b",
                "Retrieval chunk size selection",
                "Chunk prose at 512 tokens so retrieval quality stays stable across queries. Selecting the chunk size matters more than the model.",
                &["retrieval"],
            ),
        ];
        entries.extend(background());
        let graph = build(&entries);
        let path = graph.path("a", "b", 0.0).unwrap().expect("path");
        assert_eq!(path.nodes, vec!["a".to_string(), "b".to_string()]);
        assert!(path.bottleneck > 0.15, "{path:?}");
        assert!(
            path.edges
                .iter()
                .all(|edge| edge.kind == EdgeKind::Semantic)
        );
        assert!(graph.path("a", "b", 0.999).unwrap().is_none());
    }

    #[test]
    fn tiny_vaults_say_why_semantic_edges_are_sparse() {
        let graph = build(&[entry("a", "A", "One memory.", &["x"])]);
        assert!(
            graph
                .notes
                .iter()
                .any(|note| note.contains("only 1 memories")),
            "{:?}",
            graph.notes
        );
    }

    #[test]
    fn a_shared_subject_tag_carries_a_full_weight_edge() {
        let graph = build(&[
            entry("a", "A", "Chunking prose with 512 tokens.", &["rag"]),
            entry(
                "b",
                "B",
                "A completely different sentence about sailing.",
                &["rag"],
            ),
        ]);
        let tag = graph
            .explain("a", "b")
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::Tag)
            .expect("tag edge");
        assert_eq!(tag.weight, 1.0);
        assert!(tag.evidence.contains("shares 1 of 1 subject tags"));
    }

    #[test]
    fn unconnected_entries_have_no_path() {
        let graph = build(&[
            entry("a", "A", "Chunking prose with 512 tokens.", &["rag"]),
            entry("b", "B", "Alpine travel packing list.", &["life"]),
        ]);
        assert!(graph.path("a", "b", 0.0).unwrap().is_none());
    }

    #[test]
    fn unknown_entries_are_rejected_instead_of_ignored() {
        let graph = build(&[entry("a", "A", "Body text.", &["x"])]);
        assert!(graph.neighborhood("missing", 1, 0.0, None).is_err());
        assert!(graph.path("a", "missing", 0.0).is_err());
    }

    #[test]
    fn stats_report_structure_and_orphans() {
        let graph = build(&[
            entry("a", "A", "Chunking prose with 512 tokens.", &["rag"]),
            entry("b", "B", "Chunking prose with 512 tokens.", &["rag"]),
            entry("lone", "Lone", "Unrelated note about travel.", &["life"]),
        ]);
        let stats = graph.stats(0.0);
        assert_eq!(stats.nodes, 3);
        assert!(stats.orphans.contains(&"lone".to_string()));
        assert_eq!(stats.components, 2);
        assert_eq!(stats.largest_component, 2);
        assert!(stats.edges_by_kind.contains_key("semantic"));
        assert!(stats.hubs.iter().all(|hub| hub.degree > 0));
    }

    #[test]
    fn budget_prunes_inferred_edges_but_keeps_declared_ones() {
        let mut entries: Vec<Entry> = (0..12)
            .map(|index| {
                entry(
                    &format!("e{index:02}"),
                    &format!("Memory {index}"),
                    "Shared subject text about chunking prose and tokens.",
                    &["rag"],
                )
            })
            .collect();
        entries[0].meta.links = vec!["e11".into()];
        let index = EmbeddingIndex::build(&entries, 8192, "test").unwrap();
        let config = GraphConfig {
            max_edges_per_node: 2,
            ..GraphConfig::default()
        };
        let graph = Graph::build(&entries, &index, &config, "test");
        assert!(
            graph
                .explain("e00", "e11")
                .iter()
                .any(|edge| edge.kind == EdgeKind::Link)
        );
        for node in &graph.nodes {
            assert!(
                node.degree <= config.max_edges_per_node + 1,
                "{} has degree {}",
                node.id,
                node.degree
            );
        }
        assert!(graph.notes.iter().any(|note| note.contains("pruned")));
    }

    #[test]
    fn builds_are_deterministic() {
        let entries = vec![
            entry("a", "A", "Chunking prose with 512 tokens.", &["rag"]),
            entry("b", "B", "Chunking prose with 512 tokens.", &["rag"]),
            entry("c", "C", "Git conflict resolution.", &["git"]),
        ];
        let first = build(&entries);
        let second = build(&entries);
        assert_eq!(first.edges, second.edges);
        assert_eq!(first.nodes, second.nodes);
    }

    #[test]
    fn exports_name_the_relation_on_every_line() {
        let graph = build(&[
            entry("a", "A", "Chunking prose with 512 tokens.", &["rag"]),
            entry("b", "B", "Chunking prose with 512 tokens.", &["rag"]),
        ]);
        assert!(graph.to_dot(0.0).contains("label=\"semantic"));
        let mermaid = graph.to_mermaid(0.0);
        assert!(mermaid.starts_with("graph LR"));
        assert!(mermaid.contains("semantic"));
    }

    #[test]
    fn fingerprint_changes_with_content_not_only_timestamps() {
        let original = vec![entry("a", "A", "Original body.", &["x"])];
        let mut edited = original.clone();
        edited[0].body = "Edited body.".into();
        assert_ne!(fingerprint(&original), fingerprint(&edited));
        assert_eq!(fingerprint(&original), fingerprint(&original.clone()));
    }

    #[test]
    fn graph_round_trips_through_disk() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("graph.json");
        let graph = build(&[entry("a", "A", "Body text here.", &["x"])]);
        graph.save(&path).unwrap();
        let loaded = Graph::load(&path, &graph.fingerprint).unwrap().unwrap();
        assert_eq!(loaded.nodes, graph.nodes);
        assert_eq!(loaded.edges, graph.edges);
        assert!(Graph::load(&path, "other").unwrap().is_none());
        assert!(
            Graph::load(&directory.path().join("missing.json"), "x")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn relation_filters_are_parsed_and_validated() {
        assert!(parse_kinds(&[]).unwrap().is_none());
        assert_eq!(
            parse_kinds(&["semantic,link".to_string()])
                .unwrap()
                .unwrap(),
            vec![EdgeKind::Semantic, EdgeKind::Link]
        );
        assert!(parse_kinds(&["nonsense".to_string()]).is_err());
    }
}
