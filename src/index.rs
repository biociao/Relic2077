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
        let mut statement = self.connection.prepare(
            "SELECT e.id,e.title,snippet(entries_fts,2,'[',']',' … ',18),e.confidence,e.status,e.path
             FROM entries_fts JOIN entries e ON e.rowid=entries_fts.rowid
             WHERE entries_fts MATCH ? ORDER BY bm25(entries_fts), e.confidence DESC LIMIT ?"
        )?;
        let rows = statement.query_map(params![query, limit as i64], |row| {
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
