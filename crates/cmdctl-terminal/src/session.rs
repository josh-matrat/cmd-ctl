use std::borrow::Cow;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::tty;
use alacritty_terminal::Term;
use alacritty_terminal::term::Config as TermConfig;

use crate::block_detector::BlockDetector;

/// Wrapper around alacritty_terminal::Term with session metadata.
pub struct Session {
    pub id: String,
    pub name: String,
    pub term: Arc<FairMutex<Term<SessionEventProxy>>>,
    pub notifier: Notifier,
    pub block_detector: BlockDetector,
}

/// Proxy that forwards terminal events to a channel.
#[derive(Clone)]
pub struct SessionEventProxy {
    sender: crossbeam_channel::Sender<SessionEvent>,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    Wakeup,
    Title(String),
    Bell,
    Exit,
}

impl EventListener for SessionEventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => { let _ = self.sender.send(SessionEvent::Wakeup); }
            Event::Title(t) => { let _ = self.sender.send(SessionEvent::Title(t)); }
            Event::Bell => { let _ = self.sender.send(SessionEvent::Bell); }
            Event::Exit => { let _ = self.sender.send(SessionEvent::Exit); }
            _ => {}
        }
    }
}

/// Terminal size info for creating PTY.
#[derive(Clone, Copy)]
pub struct TermSize {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl TermSize {
    pub fn window_size(&self) -> WindowSize {
        WindowSize {
            num_cols: self.columns,
            num_lines: self.rows,
            cell_width: self.cell_width,
            cell_height: self.cell_height,
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }

    fn screen_lines(&self) -> usize {
        self.rows as usize
    }

    fn columns(&self) -> usize {
        self.columns as usize
    }
}

impl Session {
    /// Create a new terminal session.
    /// `working_dir` sets the initial directory. If None, defaults to home directory.
    pub fn new(
        id: String,
        name: String,
        size: TermSize,
        working_dir: Option<PathBuf>,
        agent_type: &str,
    ) -> io::Result<(Self, crossbeam_channel::Receiver<SessionEvent>)> {
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let event_proxy = SessionEventProxy { sender: event_tx };

        let term_config = TermConfig::default();
        let term = Term::new(term_config, &size, event_proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());

        let pty_config = tty::Options {
            shell: Some(tty::Shell::new(shell, vec![String::from("-l")])),
            working_directory: working_dir,
            ..Default::default()
        };

        let window_size = size.window_size();
        let pty = tty::new(&pty_config, window_size, 0)?;

        let event_loop = EventLoop::new(
            term.clone(),
            event_proxy,
            pty,
            pty_config.drain_on_exit,
            false,
        )?;

        let notifier = Notifier(event_loop.channel());
        let _join_handle = event_loop.spawn();

        let session = Session {
            id,
            name,
            term,
            notifier,
            block_detector: BlockDetector::new(agent_type),
        };

        Ok((session, event_rx))
    }

    /// Send input bytes to the PTY.
    pub fn write(&self, data: &[u8]) {
        let _ = self.notifier.0.send(Msg::Input(Cow::Owned(data.to_vec())));
    }

    /// Resize the terminal.
    pub fn resize(&mut self, size: TermSize) {
        let window_size = size.window_size();
        let _ = self.notifier.0.send(Msg::Resize(window_size));
        let mut term = self.term.lock();
        term.resize(size);
    }
}
