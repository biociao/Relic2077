use crate::embedding::{EmbeddingIndex, SimilarityHit};
use crate::entry::{Entry, EntryMeta, slugify};
use crate::graph::Graph;
use crate::index::Index;
use crate::search::RelatedMemory;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Utc};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Debug, Default)]
pub struct EntryPatch {
    pub title: Option<String>,
    pub content: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub confidence: Option<f64>,
    pub tags: Option<Vec<String>>,
    pub source_agents: Option<Vec<String>>,
    pub links: Option<Vec<String>>,
}

#[derive(Clone)]
pub struct Vault {
    pub root: PathBuf,
}

impl Vault {
    pub fn discover(start: &Path) -> Result<Self> {
        for directory in start.ancestors() {
            if directory.join(".relic/config.yaml").is_file() {
                return Ok(Self {
                    root: directory.to_owned(),
                });
            }
        }
        bail!("not inside a Relic vault (run `relic init` first)")
    }

    pub fn init(path: &Path) -> Result<Self> {
        fs::create_dir_all(path)?;
        for directory in [
            "entries/inbox",
            "reflections/daily",
            "reflections/weekly",
            "reflections/monthly",
            "patterns",
            "decisions",
            "sources/papers",
            "sources/articles",
            "sources/conversations",
            "attachments/images",
            "attachments/files",
            ".relic/embeddings",
        ] {
            fs::create_dir_all(path.join(directory))?;
        }
        write_if_missing(&path.join(".relic/config.yaml"), DEFAULT_CONFIG)?;
        write_if_missing(&path.join(".relic/schema.json"), SCHEMA)?;
        write_if_missing(&path.join(".relic/taxonomy.md"), TAXONOMY)?;
        write_if_missing(&path.join("AGENTS.md"), AGENTS)?;
        write_if_missing(&path.join(".gitignore"), VAULT_GITIGNORE)?;
        let vault = Self {
            root: path.to_owned(),
        };
        vault.reindex()?;
        Ok(vault)
    }

    pub fn create(
        &self,
        title: &str,
        content: &str,
        kind: &str,
        tags: Vec<String>,
        confidence: f64,
        source_agent: &str,
    ) -> Result<Entry> {
        if !(0.0..=1.0).contains(&confidence) {
            bail!("confidence must be between 0 and 1");
        }
        // Validate before creating the Markdown file. Configuration changes
        // apply to new memories without rewriting existing confidence history.
        let config = self.config()?;
        let now = Utc::now();
        let short_id = &Uuid::new_v4().simple().to_string()[..6];
        let id = format!("relic-{}-{short_id}", now.format("%Y%m%d"));
        let folder = match kind {
            "pattern" => "patterns",
            "decision" => "decisions",
            "reflection" => "reflections/daily",
            _ => "entries/inbox",
        };
        let path = self
            .root
            .join(folder)
            .join(format!("{}-{}.md", slugify(title), short_id));
        let entry = Entry {
            meta: EntryMeta {
                id,
                kind: kind.into(),
                title: title.into(),
                status: "active".into(),
                confidence,
                tags,
                source_agents: if source_agent.is_empty() {
                    vec![]
                } else {
                    vec![source_agent.into()]
                },
                created: now,
                updated: now,
                last_verified: now,
                expires: None,
                supersedes: vec![],
                superseded_by: None,
                links: vec![],
                decay_rate: config.vault.default_decay_rate,
            },
            body: format!("# {title}\n\n{content}"),
            path,
        };
        fs::write(&entry.path, entry.render()?)?;
        self.reindex()?;
        Ok(entry)
    }

    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for folder in ["entries", "patterns", "decisions", "reflections"] {
            let root = self.root.join(folder);
            if !root.exists() {
                continue;
            }
            for item in WalkDir::new(root)
                .into_iter()
                .filter_map(Result::ok)
                .filter(|item| item.path().extension().is_some_and(|ext| ext == "md"))
            {
                let input = fs::read_to_string(item.path())?;
                entries.push(
                    Entry::parse(item.path(), &input)
                        .with_context(|| format!("invalid entry {}", item.path().display()))?,
                );
            }
        }
        entries.sort_by(|a, b| b.meta.updated.cmp(&a.meta.updated));
        Ok(entries)
    }

    pub fn get(&self, id: &str) -> Result<Entry> {
        self.entries()?
            .into_iter()
            .find(|entry| entry.meta.id == id)
            .with_context(|| format!("entry '{id}' not found"))
    }

    pub fn update(&self, id: &str, patch: EntryPatch) -> Result<Entry> {
        let mut entry = self.get(id)?;
        if let Some(title) = patch.title {
            if title.trim().is_empty() {
                bail!("title cannot be empty");
            }
            entry.meta.title = title;
        }
        if let Some(content) = patch.content {
            entry.body = content;
        }
        if let Some(kind) = patch.kind {
            entry.meta.kind = kind;
        }
        if let Some(status) = patch.status {
            if !["active", "fading", "superseded", "archived"].contains(&status.as_str()) {
                bail!("unsupported status '{status}'");
            }
            entry.meta.status = status;
        }
        if let Some(confidence) = patch.confidence {
            if !(0.0..=1.0).contains(&confidence) {
                bail!("confidence must be between 0 and 1");
            }
            entry.meta.confidence = confidence;
            entry.meta.last_verified = Utc::now();
        }
        if let Some(tags) = patch.tags {
            entry.meta.tags = tags;
        }
        if let Some(source_agents) = patch.source_agents {
            entry.meta.source_agents = source_agents;
        }
        if let Some(links) = patch.links {
            entry.meta.links = links;
        }
        entry.meta.updated = Utc::now();
        fs::write(&entry.path, entry.render()?)?;
        self.reindex()?;
        Ok(entry)
    }

    pub fn supersede(&self, old_id: &str, new_id: &str) -> Result<(Entry, Entry)> {
        if old_id == new_id {
            bail!("an entry cannot supersede itself");
        }
        let mut old = self.get(old_id)?;
        let mut new = self.get(new_id)?;
        old.meta.status = "superseded".into();
        old.meta.superseded_by = Some(new.meta.id.clone());
        old.meta.updated = Utc::now();
        if !new.meta.supersedes.contains(&old.meta.id) {
            new.meta.supersedes.push(old.meta.id.clone());
        }
        if !new.meta.links.contains(&old.meta.id) {
            new.meta.links.push(old.meta.id.clone());
        }
        new.meta.updated = Utc::now();
        fs::write(&old.path, old.render()?)?;
        fs::write(&new.path, new.render()?)?;
        self.reindex()?;
        Ok((old, new))
    }

    pub fn reindex(&self) -> Result<usize> {
        let entries = self.entries()?;
        Index::open(&self.root.join(".relic/index.sqlite"))?.rebuild(&entries)?;
        Ok(entries.len())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<crate::index::SearchHit>> {
        self.reindex()?;
        Index::open(&self.root.join(".relic/index.sqlite"))?.search(query, limit)
    }

    /// Keyword search for a memory containing *any* of the query's terms, for
    /// prompt-driven retrieval where the wording is a bag of hints.
    pub fn search_any(&self, query: &str, limit: usize) -> Result<Vec<crate::index::SearchHit>> {
        self.reindex()?;
        Index::open(&self.root.join(".relic/index.sqlite"))?.search_any(query, limit)
    }

    /// Where the derived relation network lives. Disposable and gitignored: it
    /// is rebuilt from Markdown whenever it is missing or stale.
    pub fn graph_path(&self) -> PathBuf {
        self.root.join(".relic").join("graph.json")
    }

    /// Where the derived vector layer lives. Also disposable and gitignored.
    pub fn embeddings_path(&self) -> PathBuf {
        self.root
            .join(".relic")
            .join("embeddings")
            .join("store.bin")
    }

    /// The knowledge graph for the vault's current content.
    ///
    /// Rebuilt only when the Markdown actually changed, which is what makes it
    /// safe for a query path to call this: an unchanged vault reads one small
    /// JSON file instead of re-tokenizing every memory. A rebuild is never
    /// triggered by a *deleted* cache file alone being an error — every derived
    /// artifact here is reproducible by construction.
    pub fn graph(&self) -> Result<Graph> {
        let entries = self.entries()?;
        let fingerprint = crate::graph::fingerprint(&entries);
        if let Some(graph) = Graph::load(&self.graph_path(), &fingerprint)? {
            return Ok(graph);
        }
        self.build_graph(&entries, &fingerprint)
    }

    /// Rebuild the vector layer and the graph unconditionally.
    pub fn rebuild_graph(&self) -> Result<Graph> {
        let entries = self.entries()?;
        let fingerprint = crate::graph::fingerprint(&entries);
        self.build_graph(&entries, &fingerprint)
    }

    fn build_graph(&self, entries: &[Entry], fingerprint: &str) -> Result<Graph> {
        let config = self.config()?;
        let embeddings = self.build_embeddings(entries, fingerprint, config.graph.dimensions)?;
        let graph = Graph::build(entries, &embeddings, &config.graph, fingerprint);
        graph.save(&self.graph_path())?;
        Ok(graph)
    }

    /// The vector layer for the vault's current content, from cache when fresh.
    pub fn embeddings(&self) -> Result<EmbeddingIndex> {
        let entries = self.entries()?;
        let fingerprint = crate::graph::fingerprint(&entries);
        let dimensions = self.config()?.graph.dimensions;
        self.build_embeddings(&entries, &fingerprint, dimensions)
    }

    fn build_embeddings(
        &self,
        entries: &[Entry],
        fingerprint: &str,
        dimensions: usize,
    ) -> Result<EmbeddingIndex> {
        let path = self.embeddings_path();
        if let Some(index) = EmbeddingIndex::load(&path, fingerprint, dimensions)? {
            return Ok(index);
        }
        let index = EmbeddingIndex::build(entries, dimensions, fingerprint)?;
        // The cache is an optimisation, so a read-only vault or a full disk must
        // not turn a successful analysis into a failure.
        let _ = index.save(&path);
        Ok(index)
    }

    /// Memories closest to one memory in the vector space.
    pub fn similar(
        &self,
        id: &str,
        limit: usize,
        minimum_similarity: f64,
    ) -> Result<Vec<RelatedMemory>> {
        let entries = self.entries()?;
        let entry = entries
            .iter()
            .find(|entry| entry.meta.id == id)
            .with_context(|| format!("entry '{id}' not found"))?;
        let embeddings = self.embeddings()?;
        let position = embeddings
            .position_of(&entry.meta.id)
            .with_context(|| format!("entry '{id}' has no vector"))?;
        Ok(related_from(
            &entries,
            embeddings.nearest_from(position, limit, minimum_similarity),
        ))
    }

    /// Memories closest to arbitrary text.
    pub fn semantic_queries(&self, query: &str, limit: usize) -> Result<Vec<RelatedMemory>> {
        let entries = self.entries()?;
        let embeddings = self.embeddings()?;
        let vector = embeddings.encode(query);
        let minimum = self.config()?.graph.semantic_min_similarity;
        Ok(related_from(
            &entries,
            embeddings.nearest(&vector, limit, minimum, None),
        ))
    }

    /// Hybrid retrieval: FTS5 keyword precision fused with vector recall.
    pub fn hybrid_search(
        &self,
        query: &str,
        limit: usize,
        mode: crate::search::Mode,
    ) -> Result<Vec<crate::search::FusedHit>> {
        crate::search::search(self, query, limit, mode)
    }

    /// Load and validate the vault's `.relic/config.yaml`.
    pub fn config(&self) -> Result<crate::config::Config> {
        crate::config::Config::load(&self.root)
    }

    /// Rebuild the index and, if the reflection trigger is met, write a
    /// reflection draft. This is the workhorse for the `relic watch` daemon.
    pub fn maintain(&self, period: &str, min_entries: usize) -> Result<Maintenance> {
        let captures = crate::capture::work(self, 100)?;
        let entries = self.entries()?;
        self.reindex()?;
        let reflected = if self.should_reflect(period, min_entries)? {
            self.create_reflection(period)?;
            true
        } else {
            false
        };
        Ok(Maintenance {
            entries: entries.len(),
            reflected,
            captures,
        })
    }

    pub fn create_reflection(&self, period: &str) -> Result<PathBuf> {
        let now = Utc::now();
        let (folder, filename) = reflection_target(period, now)?;
        let path = self.root.join(folder).join(filename);
        if path.exists() {
            bail!("reflection already exists: {}", path.display());
        }
        let entries = self.entries()?;
        let recent = entries
            .iter()
            .take(10)
            .map(|entry| format!("- [{}] {}", entry.meta.id, entry.meta.title))
            .collect::<Vec<_>>()
            .join("\n");
        let contradictions = crate::analysis::detect_contradictions(&entries);
        let patterns = crate::analysis::extract_patterns(&entries, 2);
        let contradiction_section = if contradictions.is_empty() {
            "- None recorded yet.".to_string()
        } else {
            contradictions
                .iter()
                .map(|c| format!("- `{}` vs `{}` ({})", c.a, c.b, c.reason))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let pattern_section = if patterns.is_empty() {
            "- No candidate pattern yet; add related entries first.".to_string()
        } else {
            patterns
                .iter()
                .map(|p| format!("- [ ] `{}` from {} member(s)", p.title, p.members.len()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let id = format!("reflection-{}-{}", now.format("%Y%m%d"), period);
        let text = format!(
            "---\nid: {id}\ntype: reflection\ntitle: \"{} reflection\"\nstatus: active\nconfidence: 1.0\ntags: [reflection, {period}]\nsource_agents: []\ncreated: {}\nupdated: {}\nlast_verified: {}\nexpires: null\nsupersedes: []\nsuperseded_by: null\nlinks: []\ndecay_rate: 0.0\n---\n\n# {} reflection\n\n## Recent knowledge\n{recent}\n\n## Contradictions\n{contradiction_section}\n\n## Patterns worth extracting\n{pattern_section}\n\n## Actions\n- [ ] Verify fading knowledge.\n",
            period,
            now.to_rfc3339(),
            now.to_rfc3339(),
            now.to_rfc3339(),
            period
        );
        fs::write(&path, text)?;
        self.reindex()?;
        Ok(path)
    }

    /// The path a reflection would take for `period`, if it already exists.
    pub fn reflection_path(&self, period: &str) -> Result<Option<PathBuf>> {
        let now = Utc::now();
        let (folder, filename) = reflection_target(period, now)?;
        let path = self.root.join(folder).join(filename);
        Ok(path.exists().then_some(path))
    }

    /// Decide whether an automatic reflection should be created: it must not
    /// already exist and the vault must hold at least `min_entries` entries.
    pub fn should_reflect(&self, period: &str, min_entries: usize) -> Result<bool> {
        if self.reflection_path(period)?.is_some() {
            return Ok(false);
        }
        if self.entries()?.len() < min_entries {
            return Ok(false);
        }
        Ok(true)
    }

    /// Create a pattern entry from a pattern proposal for the given tag.
    pub fn write_pattern(&self, tag: &str) -> Result<Entry> {
        let entries = self.entries()?;
        let proposal = crate::analysis::extract_patterns(&entries, 1)
            .into_iter()
            .find(|proposal| proposal.tag == tag)
            .with_context(|| format!("no entries carry the tag '{tag}'"))?;
        let title = proposal.title;
        let members_ids = proposal.members_ids.clone();
        let content = format!(
            "{}\n\nExtracted from: {}",
            proposal.body,
            members_ids
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let entry = self.create(&title, &content, "pattern", proposal.members, 0.6, "relic")?;
        // Record membership as declared links as well as prose, so the derived
        // graph can carry a pattern's provenance as a real edge instead of
        // having to parse the sentence above back out of the body.
        self.update(
            &entry.meta.id,
            EntryPatch {
                links: Some(members_ids),
                ..EntryPatch::default()
            },
        )
    }
}

/// A snapshot of one maintenance pass on the vault.
#[derive(Debug, serde::Serialize)]
pub struct Maintenance {
    pub entries: usize,
    pub reflected: bool,
    pub captures: crate::capture::WorkReport,
}

/// Join vector hits with the entries they name, so a related memory carries the
/// reader-facing fields rather than only an ID and a number.
fn related_from(entries: &[Entry], hits: Vec<SimilarityHit>) -> Vec<RelatedMemory> {
    let by_id: HashMap<&str, &Entry> = entries
        .iter()
        .map(|entry| (entry.meta.id.as_str(), entry))
        .collect();
    hits.into_iter()
        .filter_map(|hit| {
            let entry = by_id.get(hit.id.as_str())?;
            Some(RelatedMemory {
                id: hit.id,
                title: entry.meta.title.clone(),
                path: entry.path.to_string_lossy().into_owned(),
                tags: entry.meta.tags.clone(),
                confidence: entry.meta.effective_confidence(),
                similarity: hit.similarity,
                shared_terms: hit.shared_terms,
            })
        })
        .collect()
}

fn reflection_target(period: &str, now: chrono::DateTime<Utc>) -> Result<(&'static str, String)> {
    match period {
        "daily" => Ok(("reflections/daily", now.format("%Y-%m-%d.md").to_string())),
        "weekly" => Ok((
            "reflections/weekly",
            format!("{}-w{:02}.md", now.year(), now.iso_week().week()),
        )),
        "monthly" => Ok(("reflections/monthly", now.format("%Y-%m.md").to_string())),
        _ => bail!("period must be daily, weekly, or monthly"),
    }
}

fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if !path.exists() {
        fs::write(path, content)?;
    }
    Ok(())
}

const DEFAULT_CONFIG: &str = "version: 1\nvault:\n  name: Relic Vault\n  default_confidence: 0.7\n  default_decay_rate: 0.05\nsync:\n  mode: manual\n  remotes: []\nevolution:\n  fading_threshold: 0.3\ngraph:\n  dimensions: 8192\n  semantic_top_k: 8\n  semantic_min_similarity: 0.08\n  tag_min_jaccard: 0.34\n  corroborate_min_similarity: 0.15\n  max_edges_per_node: 24\n";
const TAXONOMY: &str = "# Taxonomy\n\nEdit this file to define your own knowledge domains and tag conventions.\n\n- ai-engineering\n- career\n- projects\n- personal\n";
const AGENTS: &str = "# Relic Vault instructions\n\nThis repository is a local-first knowledge vault. Knowledge lives in Markdown files with YAML front matter.\n\n- Search before creating to avoid duplicates.\n- Preserve entry IDs and version history.\n- Record sources and confidence honestly.\n- Supersede obsolete knowledge instead of deleting it.\n- Never commit `.relic/index.sqlite`, embeddings, or state files.\n";
const VAULT_GITIGNORE: &str = ".relic/index.sqlite*\n.relic/state.json\n.relic/queue/\n.relic/automation/\n.relic/embeddings/\n.relic/graph.json\n.DS_Store\n";
const SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "Relic knowledge entry",
  "type": "object",
  "required": ["id", "type", "title", "status", "confidence", "created", "updated", "last_verified"],
  "properties": {
    "id": {"type": "string"},
    "type": {"enum": ["knowledge", "pattern", "lesson", "decision", "reflection"]},
    "title": {"type": "string"},
    "status": {"enum": ["active", "fading", "superseded", "archived"]},
    "confidence": {"type": "number", "minimum": 0, "maximum": 1},
    "tags": {"type": "array", "items": {"type": "string"}}
  }
}"#;
