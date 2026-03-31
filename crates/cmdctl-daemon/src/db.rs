//! SQLite session metadata store.

use anyhow::Result;
use rusqlite::Connection;

use crate::ipc::SessionEntry;

pub struct SessionDb {
    conn: Connection,
}

impl SessionDb {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                agent_type TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                working_dir TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        )?;
        Ok(Self { conn })
    }

    pub fn insert(&self, entry: &SessionEntry) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, name, agent_type, status, working_dir) VALUES (?1, ?2, ?3, ?4, ?5)",
            (&entry.id, &entry.name, &entry.agent_type, &entry.status, &entry.working_dir),
        )?;
        Ok(())
    }

    pub fn update_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status = ?1 WHERE id = ?2",
            (status, id),
        )?;
        Ok(())
    }

    pub fn update_name(&self, id: &str, name: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET name = ?1 WHERE id = ?2",
            (name, id),
        )?;
        Ok(())
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM sessions WHERE id = ?1", (id,))?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<SessionEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, agent_type, status, working_dir FROM sessions ORDER BY created_at"
        )?;
        let entries = stmt.query_map([], |row| {
            Ok(SessionEntry {
                id: row.get(0)?,
                name: row.get(1)?,
                agent_type: row.get(2)?,
                status: row.get(3)?,
                working_dir: row.get(4)?,
                context_percent: 0,
            })
        })?.collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }
}
