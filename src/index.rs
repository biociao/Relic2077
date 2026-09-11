use crate::entry::Entry;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::Path;

pub struct Index {
    connection: Connection,
}

#[derive(Debug, serde::Serialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub confidence: f64,
    pub status: String,
    pub path: String,
}

impl Index {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path).context("could not open search index")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS entries (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, body TEXT NOT NULL,
                kind TEXT NOT NULL, status TEXT NOT NULL, confidence REAL NOT NULL,
                tags TEXT NOT NULL, source_agents TEXT NOT NULL, path TEXT NOT NULL,
                updated TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
                id UNINDEXED, title, body, tags, content='entries', content_rowid='rowid'
            );",
        )?;
        Ok(Self { connection })
    }

    pub fn rebuild(&mut self, entries: &[Entry]) -> Result<()> {
        let tx = self.connection.transaction()?;
        tx.execute("DELETE FROM entries_fts", [])?;
        tx.execute("DELETE FROM entries", [])?;
        for entry in entries {
            tx.execute(
                "INSERT INTO entries (id,title,body,kind,status,confidence,tags,source_agents,path,updated) VALUES (?,?,?,?,?,?,?,?,?,?)",
                params![entry.meta.id, entry.meta.title, entry.body, entry.meta.kind, entry.meta.status,
                    entry.meta.confidence, entry.meta.tags.join(","), entry.meta.source_agents.join(","),
                    entry.path.to_string_lossy(), entry.meta.updated.to_rfc3339()],
            )?;
            let rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO entries_fts(rowid,id,title,body,tags) VALUES (?,?,?,?,?)",
                params![
                    rowid,
                    entry.meta.id,
                    entry.meta.title,
                    entry.body,
                    entry.meta.tags.join(" ")
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.run(&fts_query(query, Connector::All), limit)
    }

    /// Search for memories containing *any* term, for prompt-driven retrieval
    /// where the user's wording is a bag of hints rather than a precise query.
    pub fn search_any(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        self.run(&fts_query(query, Connector::Any), limit)
    }

    fn run(&self, expression: &str, limit: usize) -> Result<Vec<SearchHit>> {
        if expression.is_empty() {
            return Ok(Vec::new());
        }
        let mut statement = self.connection.prepare(
            "SELECT e.id,e.title,snippet(entries_fts,2,'[',']',' … ',18),e.confidence,e.status,e.path
             FROM entries_fts JOIN entries e ON e.rowid=entries_fts.rowid
             WHERE entries_fts MATCH ? ORDER BY bm25(entries_fts), e.confidence DESC LIMIT ?"
        )?;
        let rows = statement.query_map(params![expression, limit as i64], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                title: row.get(1)?,
                excerpt: row.get(2)?,
                confidence: row.get(3)?,
                status: row.get(4)?,
                path: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

/// How the terms of a sanitised query are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connector {
    /// Every term must appear: precise, and what a user typing a query expects.
    All,
    /// Any term may appear: better recall when the input is a whole prompt.
    Any,
}

/// How many terms a query may contribute. A long prompt would otherwise become
/// an expression with hundreds of clauses, which costs more than it retrieves.
pub const MAXIMUM_QUERY_TERMS: usize = 24;

/// Turn user input into a valid FTS5 `MATCH` expression.
///
/// Raw input cannot be passed to `MATCH`: characters such as `-`, `*`, `:` or a
/// quote are FTS5 operators, so an ordinary query like `pre-commit` or `why?`
/// is a syntax error rather than a search. Every run of alphanumeric characters
/// becomes a quoted phrase and the runs are joined by `connector`, so arbitrary
/// input — including a whole user prompt — can never inject an operator.
///
/// A run written without word boundaries (CJK, kana, hangul) additionally gets a
/// prefix match. FTS5 indexes such a run as a single token, so without the
/// prefix a search for `知识` would not find a memory containing `知识图谱`.
/// ASCII runs stay exact, because prefixing every English word would trade away
/// the precision that makes the keyword layer worth having — the vector layer is
/// what covers wording differences.
pub fn fts_query(input: &str, connector: Connector) -> String {
    let mut terms: Vec<String> = Vec::new();
    let mut run = String::new();
    for character in input.chars() {
        if character.is_alphanumeric() {
            run.push(character);
            continue;
        }
        push_fts_term(&mut run, &mut terms);
        if terms.len() >= MAXIMUM_QUERY_TERMS {
            break;
        }
    }
    push_fts_term(&mut run, &mut terms);
    let separator = match connector {
        Connector::All => " AND ",
        Connector::Any => " OR ",
    };
    terms.join(separator)
}

fn push_fts_term(run: &mut String, terms: &mut Vec<String>) {
    let value = std::mem::take(run);
    if value.is_empty() || terms.len() >= MAXIMUM_QUERY_TERMS {
        return;
    }
    // A single ASCII letter is not a useful term; a single CJK character is.
    if value.len() == 1 && value.is_ascii() && !value.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    let prefix = if value.is_ascii() { "" } else { "*" };
    let term = format!("\"{}\"{prefix}", value.replace('"', "\"\""));
    if !terms.contains(&term) {
        terms.push(term);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, title: &str, body: &str) -> Entry {
        Entry {
            meta: crate::entry::EntryMeta {
                id: id.into(),
                kind: "knowledge".into(),
                title: title.into(),
                status: "active".into(),
                confidence: 0.8,
                tags: vec![],
                source_agents: vec![],
                created: chrono::Utc::now(),
                updated: chrono::Utc::now(),
                last_verified: chrono::Utc::now(),
                expires: None,
                supersedes: vec![],
                superseded_by: None,
                links: vec![],
                decay_rate: 0.0,
            },
            body: body.into(),
            path: std::path::PathBuf::from(format!("{id}.md")),
        }
    }

    fn index_with(entries: &[Entry]) -> (tempfile::TempDir, Index) {
        let directory = tempfile::tempdir().unwrap();
        let mut index = Index::open(&directory.path().join("index.sqlite")).unwrap();
        index.rebuild(entries).unwrap();
        (directory, index)
    }

    #[test]
    fn quotes_terms_and_keeps_implicit_and() {
        assert_eq!(
            fts_query("chunk strategy", Connector::All),
            "\"chunk\" AND \"strategy\""
        );
        assert_eq!(
            fts_query("chunk strategy", Connector::Any),
            "\"chunk\" OR \"strategy\""
        );
    }

    #[test]
    fn neutralises_fts5_operators_instead_of_failing() {
        for query in [
            "pre-commit",
            "why?",
            "NOT this",
            "key:value",
            "\"quoted\"",
            "atomic writes OR \" : ()",
            "knowledge* AND (graph)",
        ] {
            let expression = fts_query(query, Connector::All);
            assert!(!expression.is_empty(), "{query}");
            assert_eq!(
                expression.matches('"').count() % 2,
                0,
                "{query} produced {expression}"
            );
        }
        assert_eq!(
            fts_query("pre-commit", Connector::All),
            "\"pre\" AND \"commit\""
        );
        // Only single letters around the operator, so there is nothing to match.
        assert_eq!(fts_query("a*b", Connector::All), "");
    }

    #[test]
    fn a_run_without_word_boundaries_gets_a_prefix_match() {
        assert_eq!(fts_query("知识", Connector::All), "\"知识\"*");
        assert_eq!(
            fts_query("知识 图谱", Connector::All),
            "\"知识\"* AND \"图谱\"*"
        );
    }

    #[test]
    fn punctuation_only_input_matches_nothing() {
        assert_eq!(fts_query("!!!", Connector::All), "");
    }

    #[test]
    fn single_ascii_letters_are_dropped_but_digits_and_cjk_survive() {
        assert_eq!(fts_query("a b 7", Connector::All), "\"7\"");
        assert_eq!(fts_query("图", Connector::All), "\"图\"*");
    }

    #[test]
    fn duplicate_terms_are_collapsed_and_terms_are_capped() {
        assert_eq!(fts_query("rag rag", Connector::All), "\"rag\"");
        let long: String = (0..200).map(|index| format!("term{index} ")).collect();
        let expression = fts_query(&long, Connector::Any);
        assert_eq!(expression.matches(" OR ").count() + 1, MAXIMUM_QUERY_TERMS);
    }

    #[test]
    fn a_query_with_operators_searches_instead_of_erroring() {
        let (_directory, index) = index_with(&[entry(
            "a",
            "Pre-commit hooks",
            "Install pre-commit hooks before the first commit.",
        )]);
        let hits = index.search("pre-commit", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }

    #[test]
    fn any_connector_retrieves_on_partial_prompt_matches() {
        let (_directory, index) = index_with(&[
            entry("a", "Atomic writes", "Persist review before publication"),
            entry(
                "b",
                "Alpine travel",
                "Waterproof gloves and layered clothing",
            ),
        ]);
        // A whole prompt, including characters that are FTS5 operators.
        let prompt = "atomic writes OR \" : ()";
        assert!(index.search(prompt, 8).unwrap().is_empty());
        let hits = index.search_any(prompt, 8).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a");
    }
}
