//! Daemon process: Unix socket server, PID file, lifecycle management.

use std::fs;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use parking_lot::Mutex;

use cmdctl_knowledge::store::KnowledgeStore;
use cmdctl_tickets::manager::TicketManager;
use crate::db::SessionDb;
use crate::ipc::{self, KnowledgeEntryIpc, Request, Response, SessionSummaryIpc, TicketIpc};
use crate::session_manager::SessionManager;

const SOCKET_NAME: &str = "cmdctl.sock";
const PID_FILE: &str = "cmdctl.pid";
const DB_FILE: &str = "sessions.db";
const KNOWLEDGE_DB_FILE: &str = "knowledge.db";

/// Handle returned after starting the daemon. Used to shut it down.
pub struct DaemonHandle {
    running: Arc<AtomicBool>,
    #[allow(dead_code)]
    base_dir: PathBuf,
}

impl DaemonHandle {
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

// No Drop impl — daemon keeps running until explicitly shut down.
// Use `shutdown()` or `cmdctl-cli shutdown` to stop it.

/// Get the base directory (~/.cmdctl/).
pub fn base_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cmdctl")
}

pub fn socket_path() -> PathBuf {
    base_dir().join(SOCKET_NAME)
}

/// Check if a daemon is already running.
pub fn is_running() -> bool {
    let pid_path = base_dir().join(PID_FILE);
    if let Ok(pid_str) = fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            if pid > 0 {
                unsafe { libc::kill(pid, 0) == 0 }
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    }
}

/// Start the daemon in a background thread. Returns a handle for shutdown.
pub fn start_background() -> Result<DaemonHandle> {
    let dir = base_dir();
    fs::create_dir_all(&dir)?;

    // Restrict base directory to owner-only access.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }

    // Write PID file.
    fs::write(dir.join(PID_FILE), std::process::id().to_string())?;

    // Clean stale socket.
    let sock = dir.join(SOCKET_NAME);
    let _ = fs::remove_file(&sock);

    // Open DBs.
    let db = SessionDb::open(&dir.join(DB_FILE))
        .context("Failed to open session database")?;
    let knowledge_store = KnowledgeStore::open(&dir.join(KNOWLEDGE_DB_FILE))
        .context("Failed to open knowledge database")?;
    let knowledge = Arc::new(Mutex::new(knowledge_store));

    // Initial knowledge file sync.
    {
        let ks = knowledge.lock();
        if let Err(e) = cmdctl_knowledge::sync::full_sync(&ks) {
            tracing::warn!("Initial knowledge sync failed: {}", e);
        }
    }

    let manager = Arc::new(Mutex::new(SessionManager::new(db, knowledge.clone())));
    let ticket_manager = Arc::new(Mutex::new(TicketManager::new()));
    let running = Arc::new(AtomicBool::new(true));

    // Periodic knowledge file sync (every 30s).
    let sync_knowledge = knowledge.clone();
    let sync_running = running.clone();
    thread::spawn(move || {
        let mut last_sync = std::time::SystemTime::now();
        while sync_running.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_secs(30));
            let now = std::time::SystemTime::now();
            let ks = sync_knowledge.lock();
            if let Err(e) = cmdctl_knowledge::sync::incremental_sync(&ks, last_sync) {
                tracing::warn!("Knowledge sync error: {}", e);
            }
            last_sync = now;
        }
    });

    // Spawn the socket server thread.
    let server_manager = manager.clone();
    let server_knowledge = knowledge.clone();
    let server_tickets = ticket_manager.clone();
    let server_running = running.clone();
    let server_dir = dir.clone();
    thread::spawn(move || {
        if let Err(e) = run_server(server_dir, server_manager, server_knowledge, server_tickets, server_running) {
            tracing::error!("Daemon server error: {}", e);
        }
    });

    Ok(DaemonHandle {
        running,
        base_dir: dir,
    })
}

fn run_server(
    dir: PathBuf,
    manager: Arc<Mutex<SessionManager>>,
    knowledge: Arc<Mutex<KnowledgeStore>>,
    ticket_manager: Arc<Mutex<TicketManager>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let socket_path = dir.join(SOCKET_NAME);
    let listener = UnixListener::bind(&socket_path)
        .context("Failed to bind Unix socket")?;

    // Restrict socket to owner-only access.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600));
    }

    listener.set_nonblocking(true)?;

    tracing::info!("Daemon listening on {}", socket_path.display());

    // Periodic poll thread for session status.
    let poll_mgr = manager.clone();
    let poll_run = running.clone();
    thread::spawn(move || {
        while poll_run.load(Ordering::Relaxed) {
            poll_mgr.lock().poll();
            thread::sleep(Duration::from_secs(1));
        }
    });

    // Accept loop.
    while running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let mgr = manager.clone();
                let ks = knowledge.clone();
                let tmgr = ticket_manager.clone();
                let run = running.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_client(stream, mgr, ks, tmgr, run) {
                        tracing::debug!("Client disconnected: {}", e);
                    }
                });
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                tracing::error!("Accept error: {}", e);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    let _ = fs::remove_file(&socket_path);
    Ok(())
}

fn handle_client(
    mut stream: UnixStream,
    manager: Arc<Mutex<SessionManager>>,
    knowledge: Arc<Mutex<KnowledgeStore>>,
    ticket_manager: Arc<Mutex<TicketManager>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    stream.set_nonblocking(false)?;

    // Verify the connecting process belongs to the same user.
    {
        use std::os::unix::io::AsRawFd;
        let fd = stream.as_raw_fd();
        let mut euid: libc::uid_t = 0;
        let mut egid: libc::gid_t = 0;
        let ret = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if ret != 0 || euid != unsafe { libc::getuid() } {
            anyhow::bail!("IPC connection rejected: peer UID mismatch");
        }
    }

    stream.set_read_timeout(Some(Duration::from_secs(300)))?;

    loop {
        if !running.load(Ordering::Relaxed) {
            break;
        }

        let req: Request = match ipc::read_message(&mut stream) {
            Ok(r) => r,
            Err(_) => break,
        };

        let response = process_request(req, &manager, &knowledge, &ticket_manager, &running);
        ipc::write_message(&mut stream, &response)?;

        if !running.load(Ordering::Relaxed) {
            break;
        }
    }

    Ok(())
}

fn ticket_to_ipc(t: &cmdctl_tickets::provider::Ticket) -> TicketIpc {
    TicketIpc {
        key: t.key.clone(),
        title: t.title.clone(),
        description: t.description.clone(),
        status: t.status.label().to_string(),
        status_icon: t.status.icon().to_string(),
        priority: format!("{:?}", t.priority),
        priority_icon: t.priority.icon().to_string(),
        provider: t.provider.clone(),
        url: t.url.clone(),
        assignee: t.assignee.clone(),
        labels: t.labels.clone(),
        context_prompt: t.to_context_prompt(),
    }
}

fn knowledge_to_ipc(k: &cmdctl_knowledge::store::KnowledgeEntry) -> KnowledgeEntryIpc {
    KnowledgeEntryIpc {
        id: k.id.clone(),
        title: k.title.clone(),
        content: k.content.clone(),
        scope: k.scope.clone(),
        tags: k.tags.clone(),
        source: k.source.clone(),
        created_at: k.created_at.clone(),
        updated_at: k.updated_at.clone(),
    }
}

fn summary_to_ipc(s: &cmdctl_knowledge::store::SessionSummaryEntry) -> SessionSummaryIpc {
    SessionSummaryIpc {
        id: s.id.clone(),
        session_id: s.session_id.clone(),
        session_name: s.session_name.clone(),
        working_dir: s.working_dir.clone(),
        summary: s.summary.clone(),
        decisions: s.decisions.clone(),
        unresolved: s.unresolved.clone(),
        created_at: s.created_at.clone(),
    }
}

fn process_request(
    req: Request,
    manager: &Arc<Mutex<SessionManager>>,
    knowledge: &Arc<Mutex<KnowledgeStore>>,
    ticket_manager: &Arc<Mutex<TicketManager>>,
    running: &Arc<AtomicBool>,
) -> Response {
    match req {
        Request::ListSessions => {
            Response::SessionList(manager.lock().list_sessions())
        }
        Request::CreateSession { name, agent_type, working_dir, base_branch, dangerously_skip_permissions } => {
            match manager.lock().create_session(name, agent_type, working_dir, base_branch, dangerously_skip_permissions, 80, 24, 8, 17) {
                Ok(id) => Response::SessionCreated(id),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::KillSession(id) => {
            match manager.lock().kill_session(&id) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::RenameSession { id, new_name } => {
            match manager.lock().rename_session(&id, &new_name) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::Input(id, data) => {
            match manager.lock().send_input(&id, &data) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::Resize(id, cols, rows, cell_w, cell_h) => {
            match manager.lock().resize_session(&id, cols, rows, cell_w, cell_h) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::GetGrid(id) => {
            match manager.lock().get_grid(&id) {
                Ok(grid) => Response::Grid(grid),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::ScrollSession(id, delta) => {
            match manager.lock().scroll_session(&id, delta) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::AttachSession(_) | Request::DetachSession(_) => Response::Ok,
        Request::Shutdown => {
            running.store(false, Ordering::Relaxed);
            Response::Ok
        }

        // -- Ticket operations --

        Request::ListTickets => {
            let tickets: Vec<TicketIpc> = ticket_manager.lock()
                .tickets()
                .iter()
                .map(ticket_to_ipc)
                .collect();
            Response::TicketList(tickets)
        }
        Request::RefreshTickets => {
            ticket_manager.lock().refresh();
            let tickets: Vec<TicketIpc> = ticket_manager.lock()
                .tickets()
                .iter()
                .map(ticket_to_ipc)
                .collect();
            Response::TicketList(tickets)
        }
        Request::GetTicket { key } => {
            match ticket_manager.lock().get_ticket(&key) {
                Some(t) => Response::TicketDetail(ticket_to_ipc(t)),
                None => Response::Error(format!("Ticket not found: {}", key)),
            }
        }
        Request::UpdateTicketTitle { key, title } => {
            ticket_manager.lock().update_title(&key, title);
            Response::Ok
        }

        // -- Knowledge operations --

        Request::AddKnowledge { title, content, scope, tags } => {
            let ks = knowledge.lock();
            let id = uuid::Uuid::new_v4().to_string();
            let entry = cmdctl_knowledge::store::KnowledgeEntry {
                id: id.clone(),
                title,
                content,
                scope,
                tags,
                source: "user".to_string(),
                file_path: None,
                created_at: String::new(),
                updated_at: String::new(),
            };
            // Write to DB and optionally to file.
            if let Err(e) = ks.insert(&entry) {
                return Response::Error(e.to_string());
            }
            let _ = cmdctl_knowledge::sync::write_knowledge_file(&entry);
            Response::KnowledgeCreated(id)
        }
        Request::UpdateKnowledge { id, title, content, tags } => {
            let ks = knowledge.lock();
            match ks.update(&id, title.as_deref(), content.as_deref(), tags.as_deref()) {
                Ok(true) => Response::Ok,
                Ok(false) => Response::Error("Knowledge entry not found".to_string()),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::RemoveKnowledge { id } => {
            let ks = knowledge.lock();
            match ks.remove(&id) {
                Ok(true) => Response::Ok,
                Ok(false) => Response::Error("Knowledge entry not found".to_string()),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::ListKnowledge { scope } => {
            let ks = knowledge.lock();
            match ks.list(scope.as_deref()) {
                Ok(entries) => Response::KnowledgeList(entries.iter().map(knowledge_to_ipc).collect()),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::SearchKnowledge { query, scope } => {
            let ks = knowledge.lock();
            match ks.search(&query, scope.as_deref()) {
                Ok(entries) => Response::SearchResults(entries.iter().map(knowledge_to_ipc).collect()),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::GetContext { working_dir } => {
            let ks = knowledge.lock();
            match cmdctl_knowledge::context::assemble_context(&ks, &working_dir) {
                Ok(ctx) => Response::ContextAssembled(ctx),
                Err(e) => Response::Error(e.to_string()),
            }
        }

        // -- Session summary operations --

        Request::SaveSessionSummary { session_id, summary, decisions, unresolved } => {
            let ks = knowledge.lock();
            let mgr = manager.lock();
            let sessions = mgr.list_sessions();
            let session = sessions.iter().find(|s| s.id == session_id);
            let (session_name, working_dir) = match session {
                Some(s) => (s.name.clone(), s.working_dir.clone()),
                None => ("unknown".to_string(), String::new()),
            };
            let entry = cmdctl_knowledge::store::SessionSummaryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                session_id,
                session_name,
                working_dir,
                summary,
                decisions,
                unresolved,
                created_at: String::new(),
            };
            match ks.save_summary(&entry) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::ListSessionSummaries { working_dir } => {
            let ks = knowledge.lock();
            match ks.list_summaries(working_dir.as_deref()) {
                Ok(summaries) => Response::SessionSummaryList(summaries.iter().map(summary_to_ipc).collect()),
                Err(e) => Response::Error(e.to_string()),
            }
        }
    }
}
