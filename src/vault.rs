use crate::entry::{Entry, EntryMeta, slugify};
use crate::index::Index;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Utc};
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
                decay_rate: 0.05,
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

    pub fn create_reflection(&self, period: &str) -> Result<PathBuf> {
        let now = Utc::now();
        let (folder, filename) = match period {
            "daily" => ("reflections/daily", now.format("%Y-%m-%d.md").to_string()),
            "weekly" => (
                "reflections/weekly",
                format!("{}-w{:02}.md", now.year(), now.iso_week().week()),
            ),
            "monthly" => ("reflections/monthly", now.format("%Y-%m.md").to_string()),
            _ => bail!("period must be daily, weekly, or monthly"),
        };
        let path = self.root.join(folder).join(filename);
        if path.exists() {
            bail!("reflection already exists: {}", path.display());
        }
        let recent = self
            .entries()?
            .into_iter()
            .take(10)
            .map(|entry| format!("- [{}] {}", entry.meta.id, entry.meta.title))
            .collect::<Vec<_>>()
            .join("\n");
        let id = format!("reflection-{}-{}", now.format("%Y%m%d"), period);
        let text = format!(
            "---\nid: {id}\ntype: reflection\ntitle: \"{} reflection\"\nstatus: active\nconfidence: 1.0\ntags: [reflection, {period}]\nsource_agents: []\ncreated: {}\nupdated: {}\nlast_verified: {}\nexpires: null\nsupersedes: []\nsuperseded_by: null\nlinks: []\ndecay_rate: 0.0\n---\n\n# {} reflection\n\n## Recent knowledge\n{recent}\n\n## Contradictions\n- None recorded yet.\n\n## Patterns worth extracting\n- [ ] Review recurring themes.\n\n## Actions\n- [ ] Verify fading knowledge.\n",
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
}

fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if !path.exists() {
        fs::write(path, content)?;
    }
    Ok(())
}

const DEFAULT_CONFIG: &str = "version: 1\nvault:\n  name: Relic Vault\n  default_confidence: 0.7\n  default_decay_rate: 0.05\nsync:\n  mode: manual\n  remotes: []\nevolution:\n  fading_threshold: 0.3\n";
const TAXONOMY: &str = "# Taxonomy\n\nEdit this file to define your own knowledge domains and tag conventions.\n\n- ai-engineering\n- career\n- projects\n- personal\n";
const AGENTS: &str = "# Relic Vault instructions\n\nThis repository is a local-first knowledge vault. Knowledge lives in Markdown files with YAML front matter.\n\n- Search before creating to avoid duplicates.\n- Preserve entry IDs and version history.\n- Record sources and confidence honestly.\n- Supersede obsolete knowledge instead of deleting it.\n- Never commit `.relic/index.sqlite`, embeddings, or state files.\n";
const VAULT_GITIGNORE: &str =
    ".relic/index.sqlite*\n.relic/state.json\n.relic/embeddings/\n.DS_Store\n";
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
