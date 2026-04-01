//! Client for connecting to the daemon over Unix socket.

use std::os::unix::net::UnixStream;

use anyhow::{Context, Result};

use crate::daemon;
use crate::ipc::{self, GridSnapshot, KnowledgeEntryIpc, Request, Response, SessionEntry, SessionId, SessionSummaryIpc, TicketIpc};

pub struct DaemonClient {
    stream: UnixStream,
}

impl DaemonClient {
    /// Connect to a running daemon.
    pub fn connect() -> Result<Self> {
        let path = daemon::socket_path();
        let stream = UnixStream::connect(&path)
            .with_context(|| format!("Cannot connect to daemon at {}", path.display()))?;
        Ok(Self { stream })
    }

    fn request(&mut self, req: Request) -> Result<Response> {
        ipc::write_message(&mut self.stream, &req)?;
        ipc::read_message(&mut self.stream)
    }

    pub fn list_sessions(&mut self) -> Result<Vec<SessionEntry>> {
        match self.request(Request::ListSessions)? {
            Response::SessionList(list) => Ok(list),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn create_session(&mut self, name: &str, agent_type: &str, working_dir: Option<&str>, base_branch: Option<&str>) -> Result<SessionId> {
        match self.request(Request::CreateSession {
            name: name.to_string(),
            agent_type: agent_type.to_string(),
            working_dir: working_dir.map(|s| s.to_string()),
            base_branch: base_branch.map(|s| s.to_string()),
        })? {
            Response::SessionCreated(id) => Ok(id),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn kill_session(&mut self, id: &str) -> Result<()> {
        match self.request(Request::KillSession(id.to_string()))? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn rename_session(&mut self, id: &str, new_name: &str) -> Result<()> {
        match self.request(Request::RenameSession {
            id: id.to_string(),
            new_name: new_name.to_string(),
        })? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn send_input(&mut self, id: &str, data: &[u8]) -> Result<()> {
        match self.request(Request::Input(id.to_string(), data.to_vec()))? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn resize_session(&mut self, id: &str, cols: u16, rows: u16, cell_w: u16, cell_h: u16) -> Result<()> {
        match self.request(Request::Resize(id.to_string(), cols, rows, cell_w, cell_h))? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn scroll_session(&mut self, id: &str, delta: i32) -> Result<()> {
        match self.request(Request::ScrollSession(id.to_string(), delta))? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn get_grid(&mut self, id: &str) -> Result<GridSnapshot> {
        match self.request(Request::GetGrid(id.to_string()))? {
            Response::Grid(grid) => Ok(grid),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.request(Request::Shutdown);
        Ok(())
    }

    pub fn list_tickets(&mut self) -> Result<Vec<TicketIpc>> {
        match self.request(Request::ListTickets)? {
            Response::TicketList(list) => Ok(list),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn refresh_tickets(&mut self) -> Result<Vec<TicketIpc>> {
        match self.request(Request::RefreshTickets)? {
            Response::TicketList(list) => Ok(list),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn get_ticket(&mut self, key: &str) -> Result<TicketIpc> {
        match self.request(Request::GetTicket { key: key.to_string() })? {
            Response::TicketDetail(t) => Ok(t),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn update_ticket_title(&mut self, key: &str, title: &str) -> Result<()> {
        match self.request(Request::UpdateTicketTitle {
            key: key.to_string(),
            title: title.to_string(),
        })? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    // -- Knowledge operations --

    pub fn add_knowledge(&mut self, title: &str, content: &str, scope: &str, tags: &str) -> Result<String> {
        match self.request(Request::AddKnowledge {
            title: title.to_string(),
            content: content.to_string(),
            scope: scope.to_string(),
            tags: tags.to_string(),
        })? {
            Response::KnowledgeCreated(id) => Ok(id),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn list_knowledge(&mut self, scope: Option<&str>) -> Result<Vec<KnowledgeEntryIpc>> {
        match self.request(Request::ListKnowledge { scope: scope.map(|s| s.to_string()) })? {
            Response::KnowledgeList(list) => Ok(list),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn search_knowledge(&mut self, query: &str, scope: Option<&str>) -> Result<Vec<KnowledgeEntryIpc>> {
        match self.request(Request::SearchKnowledge {
            query: query.to_string(),
            scope: scope.map(|s| s.to_string()),
        })? {
            Response::SearchResults(list) => Ok(list),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn remove_knowledge(&mut self, id: &str) -> Result<()> {
        match self.request(Request::RemoveKnowledge { id: id.to_string() })? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn get_context(&mut self, working_dir: &str) -> Result<String> {
        match self.request(Request::GetContext { working_dir: working_dir.to_string() })? {
            Response::ContextAssembled(ctx) => Ok(ctx),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn save_session_summary(&mut self, session_id: &str, summary: &str, decisions: &str, unresolved: &str) -> Result<()> {
        match self.request(Request::SaveSessionSummary {
            session_id: session_id.to_string(),
            summary: summary.to_string(),
            decisions: decisions.to_string(),
            unresolved: unresolved.to_string(),
        })? {
            Response::Ok => Ok(()),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }

    pub fn list_session_summaries(&mut self, working_dir: Option<&str>) -> Result<Vec<SessionSummaryIpc>> {
        match self.request(Request::ListSessionSummaries { working_dir: working_dir.map(|s| s.to_string()) })? {
            Response::SessionSummaryList(list) => Ok(list),
            Response::Error(e) => anyhow::bail!("{}", e),
            _ => anyhow::bail!("Unexpected response"),
        }
    }
}
