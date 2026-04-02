//! Manages all terminal sessions. Owns PTYs and terminal state.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use parking_lot::Mutex;

use cmdctl_knowledge::store::KnowledgeStore;
use cmdctl_terminal::block_detector::BlockedReason;
use cmdctl_terminal::session::{Session, SessionEvent, TermSize};

use crate::db::SessionDb;
use crate::ipc::{CellData, GridSnapshot, SessionEntry, SessionId};

struct ManagedSession {
    session: Session,
    event_rx: Receiver<SessionEvent>,
    entry: SessionEntry,
    /// True if the user manually renamed this session. Prevents shell title overwrite.
    user_renamed: bool,
    /// Accumulated output lines (screen + scrollback) for context estimation.
    peak_total_lines: usize,
    /// If this session uses a git worktree, the path to it (for cleanup on kill).
    worktree_path: Option<PathBuf>,
}

pub struct SessionManager {
    sessions: HashMap<SessionId, ManagedSession>,
    db: SessionDb,
    knowledge: Arc<Mutex<KnowledgeStore>>,
    next_id: u32,
}

impl SessionManager {
    pub fn new(db: SessionDb, knowledge: Arc<Mutex<KnowledgeStore>>) -> Self {
        // Mark stale sessions from a previous daemon run as exited.
        // (PTY processes can't survive daemon restart.)
        if let Ok(old) = db.list() {
            for entry in &old {
                if entry.status != "exited" {
                    let _ = db.update_status(&entry.id, "exited");
                }
            }
        }
        Self {
            sessions: HashMap::new(),
            db,
            knowledge,
            next_id: 1,
        }
    }

    pub fn create_session(
        &mut self,
        name: String,
        agent_type: String,
        working_dir: Option<String>,
        base_branch: Option<String>,
        dangerously_skip_permissions: bool,
        cols: u16,
        rows: u16,
        cell_width: u16,
        cell_height: u16,
    ) -> Result<SessionId> {
        let id = format!("session-{}", self.next_id);
        self.next_id += 1;

        let size = TermSize { columns: cols, rows, cell_width, cell_height };
        let original_wd = working_dir.as_deref().and_then(|wd| {
            let path = PathBuf::from(wd);
            // Canonicalize to resolve symlinks and ".." components.
            path.canonicalize().ok()
        });
        let wd_str = working_dir.unwrap_or_default();

        // For Claude sessions in a git repo, create an isolated worktree.
        let (wd, worktree_path) = if agent_type == "claude" {
            if let Some(ref dir) = original_wd {
                match setup_worktree(dir, &id, base_branch.as_deref()) {
                    Ok(wt_path) => {
                        tracing::info!("Created worktree for {}: {}", id, wt_path.display());
                        (Some(wt_path.clone()), Some(wt_path))
                    }
                    Err(e) => {
                        tracing::debug!("No worktree for {} ({}), using original dir", id, e);
                        (original_wd.clone(), None)
                    }
                }
            } else {
                (None, None)
            }
        } else {
            (original_wd.clone(), None)
        };

        // Inject knowledge context before launching Claude (need the path before wd is moved).
        if agent_type == "claude" {
            if let Some(ref wd_path) = wd {
                let ks = self.knowledge.lock();
                let wd_str = wd_path.to_string_lossy();
                match cmdctl_knowledge::context::write_context_file(&ks, &wd_str) {
                    Ok(Some(path)) => tracing::info!("Injected knowledge context: {}", path),
                    Ok(None) => tracing::debug!("No knowledge context for {}", wd_str),
                    Err(e) => tracing::warn!("Failed to write context file: {}", e),
                }
            }
        }

        let (session, event_rx) = Session::new(
            id.clone(),
            name.clone(),
            size,
            wd,
            &agent_type,
        )?;

        // If Claude Code session, resolve the binary to an absolute path and launch.
        if agent_type == "claude" {
            let claude_path = resolve_claude_binary()?;
            let cmd = if dangerously_skip_permissions {
                format!("{} --dangerously-skip-permissions\r", claude_path)
            } else {
                format!("{}\r", claude_path)
            };
            session.write(cmd.as_bytes());
        }

        let entry = SessionEntry {
            id: id.clone(),
            name,
            agent_type,
            status: "running".to_string(),
            working_dir: wd_str,
            context_percent: 0,
        };

        let _ = self.db.insert(&entry);
        self.sessions.insert(id.clone(), ManagedSession {
            session, event_rx, entry, user_renamed: false, peak_total_lines: 0,
            worktree_path,
        });

        Ok(id)
    }

    pub fn kill_session(&mut self, id: &str) -> Result<()> {
        if let Some(managed) = self.sessions.remove(id) {
            let _ = self.db.remove(id);
            // Clean up worktree if one was created for this session.
            if let Some(wt_path) = &managed.worktree_path {
                cleanup_worktree(wt_path);
            }
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", id)
        }
    }

    pub fn rename_session(&mut self, id: &str, new_name: &str) -> Result<()> {
        if let Some(managed) = self.sessions.get_mut(id) {
            managed.entry.name = new_name.to_string();
            managed.session.name = new_name.to_string();
            managed.user_renamed = true;
            let _ = self.db.update_name(id, new_name);
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", id)
        }
    }

    pub fn send_input(&self, id: &str, data: &[u8]) -> Result<()> {
        if let Some(managed) = self.sessions.get(id) {
            managed.session.write(data);
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", id)
        }
    }

    pub fn resize_session(&mut self, id: &str, cols: u16, rows: u16, cell_w: u16, cell_h: u16) -> Result<()> {
        if let Some(managed) = self.sessions.get_mut(id) {
            let size = TermSize { columns: cols, rows, cell_width: cell_w, cell_height: cell_h };
            managed.session.resize(size);
            Ok(())
        } else {
            anyhow::bail!("Session not found: {}", id)
        }
    }

    pub fn scroll_session(&self, id: &str, delta: i32) -> Result<()> {
        let managed = self.sessions.get(id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;
        let mut term = managed.session.term.lock();
        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        Ok(())
    }

    pub fn list_sessions(&self) -> Vec<SessionEntry> {
        self.sessions.values().map(|m| m.entry.clone()).collect()
    }

    pub fn get_grid(&self, id: &str) -> Result<GridSnapshot> {
        let managed = self.sessions.get(id)
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", id))?;

        let term = managed.session.term.lock();
        let content = term.renderable_content();
        let cursor_point = content.cursor.point;

        let mut cells = Vec::new();
        for indexed in content.display_iter {
            let point = indexed.point;
            if point.line.0 < 0 { continue; }

            let ch = indexed.cell.c;
            let is_cursor = point == cursor_point;

            // Convert colors to u8 RGBA (we'll convert to f32 on the client side).
            let fg = color_to_u8(&indexed.cell.fg);
            let bg = color_to_u8(&indexed.cell.bg);

            cells.push(CellData {
                col: point.column.0 as u16,
                row: point.line.0 as u16,
                ch,
                fg,
                bg,
                is_cursor,
            });
        }

        let grid = term.grid();
        Ok(GridSnapshot {
            session_id: id.to_string(),
            cols: grid.columns() as u16,
            rows: grid.screen_lines() as u16,
            cells,
        })
    }

    /// Poll all sessions for events and update status. Call periodically.
    pub fn poll(&mut self) {
        let mut exited = Vec::new();

        for (id, managed) in &mut self.sessions {
            let mut got_output = false;

            while let Ok(event) = managed.event_rx.try_recv() {
                match event {
                    SessionEvent::Wakeup => { got_output = true; }
                    SessionEvent::Exit => {
                        managed.entry.status = "exited".to_string();
                        exited.push(id.clone());
                    }
                    SessionEvent::Title(title) => {
                        if !managed.user_renamed {
                            managed.entry.name = title;
                        }
                    }
                    _ => {}
                }
            }

            if managed.entry.status == "exited" {
                continue;
            }

            if got_output {
                // Read terminal content and feed to block detector.
                let last_line = {
                    let term = managed.session.term.lock();
                    let content = term.renderable_content();
                    let mut line = String::new();
                    for indexed in content.display_iter {
                        let ch = indexed.cell.c;
                        line.push(ch);
                    }
                    // Get just the last non-empty line.
                    line.lines().rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .to_string()
                };

                if !last_line.is_empty() {
                    managed.session.block_detector.on_output(last_line.as_bytes());
                }
                managed.entry.status = "running".to_string();
            }

            // Block detection.
            let timeout = if managed.entry.agent_type == "claude" { 5 } else { 30 };
            if let Some(reason) = managed.session.block_detector.check(timeout) {
                managed.entry.status = match reason {
                    BlockedReason::PatternMatch { description } => format!("blocked: {}", description),
                    BlockedReason::Quiescence { idle_secs } => format!("idle {}s", idle_secs),
                };
            }

            // Context window estimation for Claude sessions.
            if managed.entry.agent_type == "claude" {
                let term = managed.session.term.lock();
                let total = term.grid().total_lines();
                if total > managed.peak_total_lines {
                    managed.peak_total_lines = total;
                }
                // Rough heuristic: each terminal line ≈ 80 chars ≈ 20 tokens.
                // Claude context also includes hidden content (file reads, tool
                // results) at ~3x visible output. Use 200k as baseline context.
                let est_tokens = managed.peak_total_lines as u64 * 20 * 3;
                let pct = ((est_tokens * 100) / 200_000).min(100) as u8;
                managed.entry.context_percent = pct;
            }

            let _ = self.db.update_status(&managed.entry.id, &managed.entry.status);
        }
    }
}

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

/// Resolve the `claude` binary to an absolute path to prevent PATH hijacking.
///
/// When the app is launched from Finder/Spotlight, the daemon inherits a minimal
/// launchd environment whose PATH lacks user-installed tool directories
/// (~/.local/bin, /opt/homebrew/bin, etc.). We spawn a login shell to perform
/// the lookup so that ~/.zprofile and friends are sourced first.
fn resolve_claude_binary() -> Result<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = Command::new(&shell)
        .args(["-lc", "which claude"])
        .output()
        .context("Failed to resolve claude binary via login shell")?;
    if !output.status.success() {
        anyhow::bail!(
            "claude binary not found on PATH — install Claude Code first \
             (https://docs.anthropic.com/en/docs/claude-code)"
        );
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        anyhow::bail!("claude binary not found on PATH");
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Git worktree helpers
// ---------------------------------------------------------------------------

/// Check if a directory is inside a git repo and return the repo root.
fn git_repo_root(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() { return None; }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(PathBuf::from(root))
}

/// Create a git worktree for a session. Returns the worktree path.
/// Creates a new branch named `cmdctl/<session-id>` based on the given ref (or HEAD).
fn setup_worktree(working_dir: &Path, session_id: &str, base_ref: Option<&str>) -> Result<PathBuf> {
    let repo_root = git_repo_root(working_dir)
        .ok_or_else(|| anyhow::anyhow!("Not a git repository"))?;

    let branch_name = format!("cmdctl/{}", session_id);
    let start_point = base_ref.unwrap_or("HEAD");

    // Place worktrees under <repo>/.git/cmdctl-worktrees/<session-id>
    let wt_path = repo_root.join(".git").join("cmdctl-worktrees").join(session_id);

    let output = Command::new("git")
        .args(["worktree", "add", "-b", &branch_name])
        .arg(&wt_path)
        .arg(start_point)
        .current_dir(&repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {}", stderr.trim());
    }

    Ok(wt_path)
}

/// Remove a worktree and its branch. Best-effort — won't fail the caller.
fn cleanup_worktree(wt_path: &Path) {
    // Find the repo root from the worktree itself.
    let repo_root = git_repo_root(wt_path);

    // Remove the worktree.
    if let Some(ref root) = repo_root {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(wt_path)
            .current_dir(root)
            .output();
    }

    // Prune stale worktree references.
    if let Some(ref root) = repo_root {
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(root)
            .output();
    }
}

// ---------------------------------------------------------------------------

use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use alacritty_terminal::grid::Dimensions;

fn color_to_u8(color: &TermColor) -> [u8; 4] {
    // Black / red / gold palette.
    const ANSI: [[u8; 3]; 16] = [
        [20, 13, 13], [217, 38, 38], [140, 153, 77], [217, 166, 33],
        [115, 128, 153], [191, 64, 89], [166, 166, 153], [230, 222, 209],
        [64, 56, 51], [242, 77, 64], [115, 107, 97], [242, 191, 64],
        [140, 133, 128], [153, 115, 166], [179, 173, 161], [242, 235, 222],
    ];
    const FG: [u8; 3] = [199, 191, 179];
    const BG: [u8; 3] = [13, 8, 8];

    match color {
        TermColor::Named(name) => {
            let rgb = match name {
                NamedColor::Black => ANSI[0],
                NamedColor::Red => ANSI[1],
                NamedColor::Green => ANSI[2],
                NamedColor::Yellow => ANSI[3],
                NamedColor::Blue => ANSI[4],
                NamedColor::Magenta => ANSI[5],
                NamedColor::Cyan => ANSI[6],
                NamedColor::White => ANSI[7],
                NamedColor::BrightBlack => ANSI[8],
                NamedColor::BrightRed => ANSI[9],
                NamedColor::BrightGreen => ANSI[10],
                NamedColor::BrightYellow => ANSI[11],
                NamedColor::BrightBlue => ANSI[12],
                NamedColor::BrightMagenta => ANSI[13],
                NamedColor::BrightCyan => ANSI[14],
                NamedColor::BrightWhite => ANSI[15],
                NamedColor::Foreground => FG,
                NamedColor::Background => BG,
                _ => FG,
            };
            [rgb[0], rgb[1], rgb[2], 255]
        }
        TermColor::Spec(rgb) => [rgb.r, rgb.g, rgb.b, 255],
        TermColor::Indexed(idx) => {
            if (*idx as usize) < 16 {
                let rgb = ANSI[*idx as usize];
                [rgb[0], rgb[1], rgb[2], 255]
            } else {
                [FG[0], FG[1], FG[2], 255]
            }
        }
    }
}
