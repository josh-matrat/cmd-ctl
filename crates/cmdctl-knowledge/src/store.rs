//! SQLite-backed knowledge store with FTS5 full-text search.

use anyhow::Result;
use rusqlite::Connection;

/// A single knowledge entry.
#[derive(Debug, Clone)]
pub struct KnowledgeEntry {
    pub id: String,
    pub title: String,
    pub content: String,
    pub scope: String,
    pub tags: String,
    pub source: String,
    pub file_path: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A session summary persisted after a session ends or context is high.
#[derive(Debug, Clone)]
pub struct SessionSummaryEntry {
    pub id: String,
    pub session_id: String,
    pub session_name: String,
    pub working_dir: String,
    pub summary: String,
    pub decisions: String,
    pub unresolved: String,
    pub created_at: String,
}

pub struct KnowledgeStore {
    conn: Connection,
}

impl KnowledgeStore {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(Self::SCHEMA)?;
        Ok(Self { conn })
    }

    const SCHEMA: &'static str = r#"
        CREATE TABLE IF NOT EXISTS knowledge (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            scope TEXT NOT NULL DEFAULT 'global',
            tags TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT 'user',
            file_path TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
            title, content, tags,
            content='knowledge',
            content_rowid='rowid'
        );

        CREATE TRIGGER IF NOT EXISTS knowledge_ai AFTER INSERT ON knowledge BEGIN
            INSERT INTO knowledge_fts(rowid, title, content, tags)
            VALUES (new.rowid, new.title, new.content, new.tags);
        END;

        CREATE TRIGGER IF NOT EXISTS knowledge_ad AFTER DELETE ON knowledge BEGIN
            INSERT INTO knowledge_fts(knowledge_fts, rowid, title, content, tags)
            VALUES ('delete', old.rowid, old.title, old.content, old.tags);
        END;

        CREATE TRIGGER IF NOT EXISTS knowledge_au AFTER UPDATE ON knowledge BEGIN
            INSERT INTO knowledge_fts(knowledge_fts, rowid, title, content, tags)
            VALUES ('delete', old.rowid, old.title, old.content, old.tags);
            INSERT INTO knowledge_fts(rowid, title, content, tags)
            VALUES (new.rowid, new.title, new.content, new.tags);
        END;

        CREATE TABLE IF NOT EXISTS session_summaries (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            session_name TEXT NOT NULL DEFAULT '',
            working_dir TEXT NOT NULL,
            summary TEXT NOT NULL,
            decisions TEXT NOT NULL DEFAULT '',
            unresolved TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
    "#;

    // -----------------------------------------------------------------------
    // Knowledge CRUD
    // -----------------------------------------------------------------------

    pub fn insert(&self, entry: &KnowledgeEntry) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO knowledge (id, title, content, scope, tags, source, file_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))",
            (
                &entry.id,
                &entry.title,
                &entry.content,
                &entry.scope,
                &entry.tags,
                &entry.source,
                &entry.file_path,
            ),
        )?;
        Ok(())
    }

    pub fn update(
        &self,
        id: &str,
        title: Option<&str>,
        content: Option<&str>,
        tags: Option<&str>,
    ) -> Result<bool> {
        let mut sets = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(t) = title {
            sets.push("title = ?");
            params.push(Box::new(t.to_string()));
        }
        if let Some(c) = content {
            sets.push("content = ?");
            params.push(Box::new(c.to_string()));
        }
        if let Some(tg) = tags {
            sets.push("tags = ?");
            params.push(Box::new(tg.to_string()));
        }

        if sets.is_empty() {
            return Ok(false);
        }

        sets.push("updated_at = datetime('now')");
        params.push(Box::new(id.to_string()));

        let sql = format!(
            "UPDATE knowledge SET {} WHERE id = ?",
            sets.join(", ")
        );

        let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let changed = self.conn.execute(&sql, params_refs.as_slice())?;
        Ok(changed > 0)
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let changed = self.conn.execute("DELETE FROM knowledge WHERE id = ?1", (id,))?;
        Ok(changed > 0)
    }

    pub fn remove_by_file_path(&self, file_path: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "DELETE FROM knowledge WHERE file_path = ?1",
            (file_path,),
        )?;
        Ok(changed > 0)
    }

    pub fn get(&self, id: &str) -> Result<Option<KnowledgeEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, scope, tags, source, file_path, created_at, updated_at
             FROM knowledge WHERE id = ?1"
        )?;
        let mut rows = stmt.query_map((id,), Self::row_to_entry)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_by_file_path(&self, file_path: &str) -> Result<Option<KnowledgeEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, scope, tags, source, file_path, created_at, updated_at
             FROM knowledge WHERE file_path = ?1"
        )?;
        let mut rows = stmt.query_map((file_path,), Self::row_to_entry)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list(&self, scope: Option<&str>) -> Result<Vec<KnowledgeEntry>> {
        match scope {
            Some(s) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, title, content, scope, tags, source, file_path, created_at, updated_at
                     FROM knowledge WHERE scope = ?1 ORDER BY updated_at DESC"
                )?;
                let results = stmt.query_map((s,), Self::row_to_entry)?.collect::<Result<Vec<_>, _>>()?;
                Ok(results)
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, title, content, scope, tags, source, file_path, created_at, updated_at
                     FROM knowledge ORDER BY updated_at DESC"
                )?;
                let results = stmt.query_map([], Self::row_to_entry)?.collect::<Result<Vec<_>, _>>()?;
                Ok(results)
            }
        }
    }

    // -----------------------------------------------------------------------
    // Full-text search
    // -----------------------------------------------------------------------

    pub fn search(&self, query: &str, scope: Option<&str>) -> Result<Vec<KnowledgeEntry>> {
        let fts_query = Self::sanitize_fts_query(query);
        if fts_query.is_empty() {
            return self.list(scope);
        }

        match scope {
            Some(s) => {
                let mut stmt = self.conn.prepare(
                    "SELECT k.id, k.title, k.content, k.scope, k.tags, k.source, k.file_path, k.created_at, k.updated_at
                     FROM knowledge k
                     JOIN knowledge_fts fts ON k.rowid = fts.rowid
                     WHERE knowledge_fts MATCH ?1 AND k.scope = ?2
                     ORDER BY rank"
                )?;
                let results = stmt.query_map((&fts_query, s), Self::row_to_entry)?.collect::<Result<Vec<_>, _>>()?;
                Ok(results)
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT k.id, k.title, k.content, k.scope, k.tags, k.source, k.file_path, k.created_at, k.updated_at
                     FROM knowledge k
                     JOIN knowledge_fts fts ON k.rowid = fts.rowid
                     WHERE knowledge_fts MATCH ?1
                     ORDER BY rank"
                )?;
                let results = stmt.query_map((&fts_query,), Self::row_to_entry)?.collect::<Result<Vec<_>, _>>()?;
                Ok(results)
            }
        }
    }

    /// Resolve all knowledge entries relevant to a working directory.
    /// Returns global entries + entries whose scope matches the working_dir.
    pub fn resolve_for_dir(&self, working_dir: &str) -> Result<Vec<KnowledgeEntry>> {
        // Global + exact scope match + scope-is-prefix-of-working_dir + working_dir-contains-scope-basename
        let mut stmt = self.conn.prepare(
            "SELECT id, title, content, scope, tags, source, file_path, created_at, updated_at
             FROM knowledge
             ORDER BY
                CASE WHEN scope = 'global' THEN 0 ELSE 1 END,
                updated_at DESC"
        )?;
        let all: Vec<KnowledgeEntry> = stmt.query_map([], Self::row_to_entry)?
            .collect::<Result<Vec<_>, _>>()?;

        let wd = working_dir.to_lowercase();
        Ok(all.into_iter().filter(|e| {
            if e.scope == "global" {
                return true;
            }
            let scope = e.scope.to_lowercase();
            // Exact match
            if scope == wd {
                return true;
            }
            // Scope is a prefix of working_dir (project root)
            if wd.starts_with(&scope) {
                return true;
            }
            // Working dir is under a path that contains scope as a component
            // e.g., scope="my-api" matches working_dir="/Users/josh/my-api/src"
            if !scope.starts_with('/') {
                return wd.contains(&format!("/{}/", scope))
                    || wd.ends_with(&format!("/{}", scope));
            }
            false
        }).collect())
    }

    // -----------------------------------------------------------------------
    // Session summaries
    // -----------------------------------------------------------------------

    pub fn save_summary(&self, entry: &SessionSummaryEntry) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO session_summaries
             (id, session_id, session_name, working_dir, summary, decisions, unresolved)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                &entry.id,
                &entry.session_id,
                &entry.session_name,
                &entry.working_dir,
                &entry.summary,
                &entry.decisions,
                &entry.unresolved,
            ),
        )?;
        Ok(())
    }

    pub fn list_summaries(&self, working_dir: Option<&str>) -> Result<Vec<SessionSummaryEntry>> {
        match working_dir {
            Some(wd) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, session_id, session_name, working_dir, summary, decisions, unresolved, created_at
                     FROM session_summaries
                     WHERE working_dir = ?1
                     ORDER BY created_at DESC
                     LIMIT 10"
                )?;
                let results = stmt.query_map((wd,), Self::row_to_summary)?.collect::<Result<Vec<_>, _>>()?;
                Ok(results)
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, session_id, session_name, working_dir, summary, decisions, unresolved, created_at
                     FROM session_summaries
                     ORDER BY created_at DESC
                     LIMIT 20"
                )?;
                let results = stmt.query_map([], Self::row_to_summary)?.collect::<Result<Vec<_>, _>>()?;
                Ok(results)
            }
        }
    }

    pub fn recent_summaries_for_dir(&self, working_dir: &str, limit: usize) -> Result<Vec<SessionSummaryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, session_name, working_dir, summary, decisions, unresolved, created_at
             FROM session_summaries
             WHERE working_dir = ?1
             ORDER BY created_at DESC
             LIMIT ?2"
        )?;
        let results = stmt.query_map((working_dir, limit as i64), Self::row_to_summary)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(results)
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<KnowledgeEntry> {
        Ok(KnowledgeEntry {
            id: row.get(0)?,
            title: row.get(1)?,
            content: row.get(2)?,
            scope: row.get(3)?,
            tags: row.get(4)?,
            source: row.get(5)?,
            file_path: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    }

    fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<SessionSummaryEntry> {
        Ok(SessionSummaryEntry {
            id: row.get(0)?,
            session_id: row.get(1)?,
            session_name: row.get(2)?,
            working_dir: row.get(3)?,
            summary: row.get(4)?,
            decisions: row.get(5)?,
            unresolved: row.get(6)?,
            created_at: row.get(7)?,
        })
    }

    /// Sanitize a user query for FTS5. Wraps each term in quotes to prevent
    /// syntax errors from special characters.
    fn sanitize_fts_query(query: &str) -> String {
        query
            .split_whitespace()
            .map(|term| {
                let clean: String = term.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
                if clean.is_empty() {
                    String::new()
                } else {
                    format!("\"{}\"", clean)
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Rebuild the FTS index from scratch. Useful after bulk operations.
    pub fn rebuild_fts(&self) -> Result<()> {
        self.conn.execute_batch(
            "INSERT INTO knowledge_fts(knowledge_fts) VALUES ('rebuild');"
        )?;
        Ok(())
    }
}
