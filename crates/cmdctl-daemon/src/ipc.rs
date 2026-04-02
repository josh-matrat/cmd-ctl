//! IPC protocol between the daemon and clients (UI app, CLI).

use serde::{Deserialize, Serialize};

pub type SessionId = String;

/// Messages sent from a client to the daemon.
#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    ListSessions,
    CreateSession {
        name: String,
        agent_type: String,
        working_dir: Option<String>,
        /// For Claude sessions in a git repo, the branch/ref to base the worktree on.
        /// Defaults to HEAD if None.
        base_branch: Option<String>,
        /// Launch Claude with --dangerously-skip-permissions.
        #[serde(default)]
        dangerously_skip_permissions: bool,
    },
    AttachSession(SessionId),
    DetachSession(SessionId),
    KillSession(SessionId),
    RenameSession {
        id: SessionId,
        new_name: String,
    },
    /// Send input bytes to a session's PTY.
    Input(SessionId, Vec<u8>),
    /// Resize a session's terminal.
    Resize(SessionId, u16, u16, u16, u16), // cols, rows, cell_w, cell_h
    /// Request the current terminal grid content for rendering.
    GetGrid(SessionId),
    /// Scroll a session's terminal viewport (positive = up into scrollback, negative = down).
    ScrollSession(SessionId, i32),
    /// Shutdown the daemon.
    Shutdown,

    // -- Knowledge operations --

    /// Add a new knowledge entry.
    AddKnowledge {
        title: String,
        content: String,
        scope: String,
        tags: String,
    },
    /// Update an existing knowledge entry.
    UpdateKnowledge {
        id: String,
        title: Option<String>,
        content: Option<String>,
        tags: Option<String>,
    },
    /// Remove a knowledge entry by ID.
    RemoveKnowledge { id: String },
    /// List knowledge entries, optionally filtered by scope.
    ListKnowledge { scope: Option<String> },
    /// Full-text search over knowledge entries.
    SearchKnowledge { query: String, scope: Option<String> },
    /// Assemble the full context document for a working directory.
    GetContext { working_dir: String },

    // -- Session summary operations --

    /// Persist a session summary.
    SaveSessionSummary {
        session_id: String,
        summary: String,
        decisions: String,
        unresolved: String,
    },
    /// List session summaries, optionally filtered by working directory.
    ListSessionSummaries { working_dir: Option<String> },

    // -- Ticket operations --

    /// List all tickets from configured providers.
    ListTickets,
    /// Force refresh tickets from all providers.
    RefreshTickets,
    /// Get a specific ticket by key.
    GetTicket { key: String },
    /// Update a ticket's title locally.
    UpdateTicketTitle { key: String, title: String },

    // -- Skill operations --

    /// List all available Claude skills.
    ListSkills,
    /// Get the full content of a skill by name.
    GetSkill { name: String },
}

/// Messages sent from the daemon to a client.
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Error(String),
    SessionList(Vec<SessionEntry>),
    SessionCreated(SessionId),
    /// Terminal grid snapshot for rendering.
    Grid(GridSnapshot),

    // -- Knowledge responses --

    KnowledgeList(Vec<KnowledgeEntryIpc>),
    KnowledgeCreated(String),
    SearchResults(Vec<KnowledgeEntryIpc>),
    ContextAssembled(String),
    SessionSummaryList(Vec<SessionSummaryIpc>),

    // -- Ticket responses --

    TicketList(Vec<TicketIpc>),
    TicketDetail(TicketIpc),

    // -- Skill responses --

    SkillList(Vec<SkillIpc>),
    SkillDetail(SkillIpc),
}

/// Session metadata for listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub id: String,
    pub name: String,
    pub agent_type: String,
    pub status: String,
    pub working_dir: String,
    /// Estimated context window usage as percentage (0-100), for Claude sessions.
    pub context_percent: u8,
}

/// A snapshot of the terminal grid for client-side rendering.
#[derive(Debug, Serialize, Deserialize)]
pub struct GridSnapshot {
    pub session_id: String,
    pub cols: u16,
    pub rows: u16,
    /// Each cell: (col, row, char, fg_rgba, bg_rgba, is_cursor)
    pub cells: Vec<CellData>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CellData {
    pub col: u16,
    pub row: u16,
    pub ch: char,
    pub fg: [u8; 4],
    pub bg: [u8; 4],
    pub is_cursor: bool,
}

/// Knowledge entry as exposed over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntryIpc {
    pub id: String,
    pub title: String,
    pub content: String,
    pub scope: String,
    pub tags: String,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Session summary as exposed over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryIpc {
    pub id: String,
    pub session_id: String,
    pub session_name: String,
    pub working_dir: String,
    pub summary: String,
    pub decisions: String,
    pub unresolved: String,
    pub created_at: String,
}

/// Ticket entry as exposed over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketIpc {
    pub key: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub status_icon: String,
    pub priority: String,
    pub priority_icon: String,
    pub provider: String,
    pub url: String,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    /// Pre-built context prompt for feeding to a Claude session.
    pub context_prompt: String,
}

/// Skill entry as exposed over IPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillIpc {
    pub name: String,
    pub description: String,
    pub plugin: String,
    pub content: String,
}

/// Read a length-prefixed bincode message from a stream.
pub fn read_message<T: serde::de::DeserializeOwned>(stream: &mut impl std::io::Read) -> anyhow::Result<T> {
    let mut len_buf = [0u8; 4];
    std::io::Read::read_exact(stream, &mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        anyhow::bail!("Message too large: {} bytes", len);
    }
    let mut buf = vec![0u8; len];
    std::io::Read::read_exact(stream, &mut buf)?;
    Ok(bincode::deserialize(&buf)?)
}

/// Write a length-prefixed bincode message to a stream.
pub fn write_message<T: serde::Serialize>(stream: &mut impl std::io::Write, msg: &T) -> anyhow::Result<()> {
    let buf = bincode::serialize(msg)?;
    let len = (buf.len() as u32).to_le_bytes();
    std::io::Write::write_all(stream, &len)?;
    std::io::Write::write_all(stream, &buf)?;
    std::io::Write::flush(stream)?;
    Ok(())
}
