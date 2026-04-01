use std::sync::Arc;

use anyhow::{Context, Result};
use metal::*;
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use cmdctl_daemon::client::DaemonClient;
use cmdctl_daemon::ipc::{SessionEntry, TicketIpc};
use cmdctl_input::keybinding::{KeyBinding, KeybindingManager, Modifiers};
use cmdctl_renderer::grid_renderer::{colors, GridRenderer};
use cmdctl_renderer::text::FontInfo;
use cmdctl_renderer::ui_renderer::{self, SidebarSection};

const FONT_NAME: &str = "Menlo";
const FONT_SIZE: f64 = 13.0;
const INITIAL_COLS: u16 = 160;
const INITIAL_ROWS: u16 = 45;
const SIDEBAR_COLS: usize = 30;

pub fn run() -> Result<()> {
    let event_loop = EventLoop::new().context("Failed to create event loop")?;
    let mut app = CmdctlApp::new()?;
    event_loop.run_app(&mut app).context("Event loop error")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// State types
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum Focus {
    Sidebar,
    Pane(usize), // index into `panes` vec (0-3)
}

enum Modal {
    PathInput { agent_type: String, input: String, cursor_pos: usize },
    BranchInput {
        agent_type: String,
        working_dir: std::path::PathBuf,
        branches: Vec<String>,
        input: String,
        cursor_pos: usize,
        selected: usize,
    },
    RenameInput { session_index: usize, input: String, cursor_pos: usize },
    TicketDetail { ticket: TicketIpc },
}

struct QuickTerminal {
    session_id: String,
    visible: bool,
}

enum Direction { Up, Down, Left, Right }

struct PaneRect {
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
}

// ---------------------------------------------------------------------------
// App structs
// ---------------------------------------------------------------------------

struct CmdctlApp {
    font: FontInfo,
    keybindings: KeybindingManager,
    state: Option<AppState>,
    modifiers: ModifiersState,
}

struct AppState {
    window: Arc<Window>,
    #[allow(dead_code)]
    device: Device,
    command_queue: CommandQueue,
    layer: MetalLayer,
    renderer: GridRenderer,
    client: DaemonClient,
    sessions: Vec<SessionEntry>,
    // Tickets
    tickets: Vec<TicketIpc>,
    ticket_selected: usize,
    sidebar_section: SidebarSection,
    // Layout
    focus: Focus,
    panes: [Option<String>; 4], // fixed 2x2 grid of pane slots
    sidebar_selected: usize,
    modal: Option<Modal>,
    quick_terminal: Option<QuickTerminal>,
    // Grid dimensions (full window in cells)
    cols: u16,
    rows: u16,
    scale_factor: f64,
    // Mouse position in logical pixels (for click-to-focus).
    cursor_position: (f64, f64),
}

impl AppState {
    /// Number of occupied pane slots.
    fn pane_count(&self) -> usize {
        self.panes.iter().filter(|p| p.is_some()).count()
    }

    /// Find which slot a session is in, if any.
    fn pane_slot_for(&self, session_id: &str) -> Option<usize> {
        self.panes.iter().position(|p| p.as_deref() == Some(session_id))
    }

    /// Find the first empty pane slot (0-3), if any.
    fn first_empty_slot(&self) -> Option<usize> {
        self.panes.iter().position(|p| p.is_none())
    }

    /// Remove a session from whichever pane slot it occupies.
    fn remove_from_panes(&mut self, session_id: &str) {
        for slot in &mut self.panes {
            if slot.as_deref() == Some(session_id) {
                *slot = None;
            }
        }
    }

    /// Find the nearest occupied pane slot for focus fallback.
    fn nearest_occupied_pane(&self, from: usize) -> Option<usize> {
        // Try forward then backward.
        (from..4).chain((0..from).rev())
            .find(|&i| self.panes[i].is_some())
    }
}

impl CmdctlApp {
    fn new() -> Result<Self> {
        let font = FontInfo::load(FONT_NAME, FONT_SIZE)?;
        Ok(Self {
            font,
            keybindings: KeybindingManager::new(),
            state: None,
            modifiers: ModifiersState::empty(),
        })
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

impl ApplicationHandler for CmdctlApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() { return; }
        match init_window(event_loop, &self.font) {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                tracing::error!("Failed to initialize: {}", e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _wid: WindowId, event: WindowEvent) {
        let state = match &mut self.state { Some(s) => s, None => return };

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed { return; }

                let mods = Modifiers {
                    cmd: self.modifiers.super_key(),
                    shift: self.modifiers.shift_key(),
                    ctrl: self.modifiers.control_key(),
                    alt: self.modifiers.alt_key(),
                };

                // Quick terminal intercepts all non-Cmd input when visible.
                if !mods.cmd {
                    if let Some(qt) = &state.quick_terminal {
                        if qt.visible {
                            let sid = qt.session_id.clone();
                            handle_terminal_input(&event.logical_key, mods, state, &sid);
                            return;
                        }
                    }
                }

                // Modal intercepts all non-Cmd input.
                if state.modal.is_some() && !mods.cmd {
                    handle_modal_input(&event.logical_key, state, &self.font);
                    return;
                }

                if mods.cmd {
                    // Cmd+Enter on a ticket: open a Claude session for this ticket.
                    if let Key::Named(NamedKey::Enter) = &event.logical_key {
                        // From the ticket detail modal
                        let ticket_from_modal = if let Some(Modal::TicketDetail { ticket }) = &state.modal {
                            Some(ticket.clone())
                        } else {
                            None
                        };
                        if let Some(ticket) = ticket_from_modal {
                            state.modal = None;
                            let default_dir = dirs::home_dir()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "~".to_string());
                            let name = format!("Ticket {} \u{2014} {}", ticket.key,
                                ticket.title.chars().take(30).collect::<String>());
                            open_ticket_session(state, &name, &default_dir, &ticket.context_prompt, &self.font);
                            return;
                        }
                        // From the sidebar tickets section
                        if state.focus == Focus::Sidebar
                            && state.sidebar_section == SidebarSection::Tickets
                            && state.ticket_selected < state.tickets.len()
                        {
                            let ticket = state.tickets[state.ticket_selected].clone();
                            let default_dir = dirs::home_dir()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|| "~".to_string());
                            let name = format!("Ticket {} \u{2014} {}", ticket.key,
                                ticket.title.chars().take(30).collect::<String>());
                            open_ticket_session(state, &name, &default_dir, &ticket.context_prompt, &self.font);
                            return;
                        }
                    }

                    // Cmd+Arrow: pane/focus navigation.
                    match &event.logical_key {
                        Key::Named(NamedKey::ArrowRight) => { navigate_focus(state, Direction::Right); return; }
                        Key::Named(NamedKey::ArrowLeft)  => { navigate_focus(state, Direction::Left);  return; }
                        Key::Named(NamedKey::ArrowDown)  => { navigate_focus(state, Direction::Down);  return; }
                        Key::Named(NamedKey::ArrowUp)    => { navigate_focus(state, Direction::Up);    return; }
                        _ => {}
                    }

                    // Cmd+Character: keybinding lookup.
                    let key_str = match &event.logical_key {
                        Key::Character(c) => c.to_string(),
                        _ => String::new(),
                    };
                    if !key_str.is_empty() {
                        let binding = KeyBinding { modifiers: mods, key: key_str.clone() };
                        if let Some(cmd) = self.keybindings.lookup(&binding) {
                            handle_global_command(cmd, &key_str, state, event_loop, &self.font);
                            return;
                        }
                    }
                }

                // Route to focused area.
                match &state.focus {
                    Focus::Sidebar => handle_sidebar_input(&event.logical_key, state, &self.font),
                    Focus::Pane(idx) => {
                        let idx = *idx;
                        if let Some(session_id) = state.panes[idx].clone() {
                            handle_terminal_input(&event.logical_key, mods, state, &session_id);
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                // Scroll the focused pane's terminal.
                if let Focus::Pane(idx) = &state.focus {
                    if let Some(session_id) = state.panes[*idx].clone() {
                        let lines = match delta {
                            MouseScrollDelta::LineDelta(_, y) => y as i32 * 3,
                            MouseScrollDelta::PixelDelta(pos) => (pos.y / state.scale_factor / self.font.cell_height) as i32,
                        };
                        if lines != 0 {
                            let _ = state.client.scroll_session(&session_id, lines);
                        }
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                // Track logical position for click-to-focus.
                state.cursor_position = (position.x / state.scale_factor, position.y / state.scale_factor);
            }

            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                let (lx, ly) = state.cursor_position;
                let click_col = (lx / self.font.cell_width) as usize;
                let click_row = (ly / self.font.cell_height) as usize;

                if click_col < SIDEBAR_COLS {
                    state.focus = Focus::Sidebar;
                } else {
                    let rects = compute_pane_rects(state.cols as usize, state.rows as usize, state.pane_count());
                    // Map rect index back to actual pane slot index.
                    let occupied: Vec<usize> = (0..4).filter(|i| state.panes[*i].is_some()).collect();
                    for (rect_idx, rect) in rects.iter().enumerate() {
                        if click_col >= rect.col && click_col < rect.col + rect.cols
                            && click_row >= rect.row && click_row < rect.row + rect.rows
                        {
                            if let Some(&slot) = occupied.get(rect_idx) {
                                state.focus = Focus::Pane(slot);
                            }
                            break;
                        }
                    }
                }
            }

            WindowEvent::Resized(size) => handle_resize(state, size, &self.font),

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.scale_factor = scale_factor;
                let size = state.window.inner_size();
                handle_resize(state, size, &self.font);
            }

            WindowEvent::RedrawRequested => {
                // Refresh session list.
                if let Ok(sessions) = state.client.list_sessions() {
                    state.sessions = sessions;
                }

                // Refresh tickets (cached in daemon, cheap to call each frame).
                if let Ok(tickets) = state.client.list_tickets() {
                    state.tickets = tickets;
                }
                if !state.tickets.is_empty() && state.ticket_selected >= state.tickets.len() {
                    state.ticket_selected = state.tickets.len() - 1;
                }

                // Remove panes whose sessions exited or were killed.
                for slot in &mut state.panes {
                    if let Some(id) = slot.as_ref() {
                        if !state.sessions.iter().any(|s| s.id == *id && s.status != "exited") {
                            *slot = None;
                        }
                    }
                }

                // Clean up quick terminal if its session exited.
                if let Some(qt) = &state.quick_terminal {
                    if !state.sessions.iter().any(|s| s.id == qt.session_id && s.status != "exited") {
                        state.quick_terminal = None;
                    }
                }

                // Clamp focus: if focused pane is now empty, fall back.
                if let Focus::Pane(idx) = &state.focus {
                    if state.panes[*idx].is_none() {
                        state.focus = state.nearest_occupied_pane(*idx)
                            .map(Focus::Pane)
                            .unwrap_or(Focus::Sidebar);
                    }
                }
                if !state.sessions.is_empty() && state.sidebar_selected >= state.sessions.len() {
                    state.sidebar_selected = state.sessions.len() - 1;
                }

                // Window title.
                let title = match &state.focus {
                    Focus::Sidebar => "CMD CTL".to_string(),
                    Focus::Pane(idx) => {
                        state.panes[*idx].as_ref()
                            .and_then(|sid| state.sessions.iter().find(|s| s.id == *sid))
                            .map(|s| format!("CMD CTL \u{2014} {}", s.name))
                            .unwrap_or_else(|| "CMD CTL".to_string())
                    }
                };
                state.window.set_title(&title);

                render_frame(state, &self.font);
                state.window.request_redraw();
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Window initialisation (Metal + NSAppearance)
// ---------------------------------------------------------------------------

fn init_window(event_loop: &ActiveEventLoop, font: &FontInfo) -> Result<AppState> {
    let cell_w = font.cell_width;
    let cell_h = font.cell_height;
    let width = (INITIAL_COLS as f64 * cell_w) as u32;
    let height = (INITIAL_ROWS as f64 * cell_h) as u32;

    let attrs = WindowAttributes::default()
        .with_title("CMD CTL")
        .with_inner_size(LogicalSize::new(width, height));

    let window = Arc::new(event_loop.create_window(attrs).context("Failed to create window")?);
    let scale_factor = window.scale_factor();
    let physical = window.inner_size();
    tracing::debug!("scale={}, physical={}x{}, cell={:.1}x{:.1}",
        scale_factor, physical.width, physical.height, cell_w, cell_h);

    let device = Device::system_default().context("No Metal device")?;
    let command_queue = device.new_command_queue();
    let layer = MetalLayer::new();
    layer.set_device(&device);
    layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    layer.set_presents_with_transaction(false);

    unsafe {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};

        let layer_obj = layer.as_ref() as *const MetalLayerRef as *mut AnyObject;
        let _: () = msg_send![layer_obj, setContentsScale: scale_factor as f64];
        let _: () = msg_send![layer_obj, setOpaque: false];

        let raw = window.window_handle().unwrap().as_raw();
        if let raw_window_handle::RawWindowHandle::AppKit(handle) = raw {
            let ns_view = handle.ns_view.as_ptr() as *mut AnyObject;
            let ns_window: *mut AnyObject = msg_send![ns_view, window];

            // Dark appearance + opaque titlebar.
            let ns_appearance_class = AnyClass::get(c"NSAppearance").unwrap();
            let dark_name: *mut AnyObject = msg_send![
                AnyClass::get(c"NSString").unwrap(),
                stringWithUTF8String: c"NSAppearanceNameVibrantDark".as_ptr()
            ];
            let dark_appearance: *mut AnyObject = msg_send![ns_appearance_class, appearanceNamed: dark_name];
            let _: () = msg_send![ns_window, setAppearance: dark_appearance];
            let _: () = msg_send![ns_window, setTitlebarAppearsTransparent: false];
            // Solid dark background — titlebar inherits this.
            let ns_color_class = AnyClass::get(c"NSColor").unwrap();
            let bg_color: *mut AnyObject = msg_send![ns_color_class,
                colorWithRed: 0.05_f64,
                green: 0.03_f64,
                blue: 0.03_f64,
                alpha: 1.0_f64
            ];
            let _: () = msg_send![ns_window, setBackgroundColor: bg_color];

            // Glass effect behind content.
            let ve_class = AnyClass::get(c"NSVisualEffectView").unwrap();
            let ve_view: *mut AnyObject = msg_send![ve_class, new];
            let _: () = msg_send![ve_view, setMaterial: 2_i64];       // Dark
            let _: () = msg_send![ve_view, setBlendingMode: 0_i64];   // behindWindow
            let _: () = msg_send![ve_view, setState: 1_i64];          // active
            let _: () = msg_send![ve_view, setAutoresizingMask: 18_u64]; // w+h

            let superview: *mut AnyObject = msg_send![ns_view, superview];
            if !superview.is_null() {
                let _: () = msg_send![superview, addSubview: ve_view, positioned: 1_i64, relativeTo: ns_view];
            } else {
                let _: () = msg_send![ns_view, addSubview: ve_view, positioned: 1_i64, relativeTo: std::ptr::null::<AnyObject>()];
            }

            let _: () = msg_send![ns_view, setWantsLayer: true];
            let _: () = msg_send![ns_view, setLayer: layer_obj];
        }
    }

    layer.set_drawable_size(core_graphics::geometry::CGSize::new(
        physical.width as f64,
        physical.height as f64,
    ));

    let renderer_font = FontInfo::load(FONT_NAME, FONT_SIZE)?;
    let renderer = GridRenderer::new(&device, renderer_font, scale_factor)?;

    let cols = ((physical.width as f64 / scale_factor) / cell_w).max(1.0) as u16;
    let rows = ((physical.height as f64 / scale_factor) / cell_h).max(1.0) as u16;

    let client = DaemonClient::connect().context("Failed to connect to daemon")?;

    window.request_redraw();

    Ok(AppState {
        window, device, command_queue, layer, renderer, client,
        sessions: Vec::new(),
        tickets: Vec::new(),
        ticket_selected: 0,
        sidebar_section: SidebarSection::Sessions,
        focus: Focus::Sidebar,
        panes: [None, None, None, None],
        sidebar_selected: 0,
        modal: None,
        quick_terminal: None,
        cols, rows, scale_factor,
        cursor_position: (0.0, 0.0),
    })
}

// ---------------------------------------------------------------------------
// Layout computation
// ---------------------------------------------------------------------------

fn compute_pane_rects(total_cols: usize, total_rows: usize, pane_count: usize) -> Vec<PaneRect> {
    let main_start = SIDEBAR_COLS + 1; // sidebar + 1-col border
    let main_cols = total_cols.saturating_sub(main_start);

    match pane_count {
        0 => vec![],
        1 => vec![PaneRect { col: main_start, row: 0, cols: main_cols, rows: total_rows }],
        2 => {
            let left_w = main_cols / 2;
            let right_w = main_cols.saturating_sub(left_w + 1); // -1 for border
            vec![
                PaneRect { col: main_start, row: 0, cols: left_w, rows: total_rows },
                PaneRect { col: main_start + left_w + 1, row: 0, cols: right_w, rows: total_rows },
            ]
        }
        _ => {
            let left_w = main_cols / 2;
            let right_w = main_cols.saturating_sub(left_w + 1);
            let top_h = total_rows / 2;
            let bot_h = total_rows.saturating_sub(top_h + 1);
            let mut rects = vec![
                PaneRect { col: main_start, row: 0, cols: left_w, rows: top_h },
                PaneRect { col: main_start + left_w + 1, row: 0, cols: right_w, rows: top_h },
                PaneRect { col: main_start, row: top_h + 1, cols: left_w, rows: bot_h },
            ];
            if pane_count >= 4 {
                rects.push(PaneRect { col: main_start + left_w + 1, row: top_h + 1, cols: right_w, rows: bot_h });
            }
            rects
        }
    }
}

// ---------------------------------------------------------------------------
// Focus navigation (Cmd+Arrow)
// ---------------------------------------------------------------------------

fn navigate_focus(state: &mut AppState, dir: Direction) {
    match (&state.focus, &dir) {
        (Focus::Sidebar, Direction::Right) => {
            // Jump to the first occupied pane.
            if let Some(slot) = state.nearest_occupied_pane(0) {
                state.focus = Focus::Pane(slot);
            }
        }
        (Focus::Pane(idx), _) => {
            let idx = *idx;
            // Map slot index → grid position (col, row) in the 2×2 layout.
            let (cx, cy) = match idx { 0 => (0,0), 1 => (1,0), 2 => (0,1), 3 => (1,1), _ => return };

            match dir {
                Direction::Left => {
                    if cx == 0 { state.focus = Focus::Sidebar; return; }
                    let target = match (cx - 1, cy) { (0,0) => 0, (0,1) => 2, _ => return };
                    if state.panes[target].is_some() { state.focus = Focus::Pane(target); }
                }
                Direction::Right => {
                    let target = match (cx + 1, cy) { (1,0) => 1, (1,1) => 3, _ => return };
                    if state.panes[target].is_some() { state.focus = Focus::Pane(target); }
                }
                Direction::Up => {
                    if cy == 0 { return; }
                    let target = match (cx, cy - 1) { (0,0) => 0, (1,0) => 1, _ => return };
                    if state.panes[target].is_some() { state.focus = Focus::Pane(target); }
                }
                Direction::Down => {
                    let target = match (cx, cy + 1) { (0,1) => 2, (1,1) => 3, _ => return };
                    if state.panes[target].is_some() { state.focus = Focus::Pane(target); }
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

fn create_session(state: &mut AppState, name: &str, agent_type: &str, working_dir: Option<std::path::PathBuf>, base_branch: Option<&str>, font: &FontInfo) {
    let wd = working_dir.map(|p| p.to_string_lossy().to_string());
    match state.client.create_session(name, agent_type, wd.as_deref(), base_branch) {
        Ok(session_id) => {
            if let Some(slot) = state.first_empty_slot() {
                // Auto-assign to the next free pane slot.
                state.panes[slot] = Some(session_id.clone());
                state.focus = Focus::Pane(slot);
                resize_pane_sessions(state, font);
            } else {
                // All pane slots full — session lives in sidebar, user can assign with ⌘⇧1-4.
                state.focus = Focus::Sidebar;
                if let Ok(sessions) = state.client.list_sessions() {
                    if let Some(pos) = sessions.iter().position(|s| s.id == session_id) {
                        state.sidebar_selected = pos;
                    }
                    state.sessions = sessions;
                }
            }
        }
        Err(e) => tracing::error!("Failed to create session: {}", e),
    }
}

fn resize_pane_sessions(state: &mut AppState, font: &FontInfo) {
    let active: Vec<(usize, String)> = state.panes.iter().enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|id| (i, id.clone())))
        .collect();
    let rects = compute_pane_rects(state.cols as usize, state.rows as usize, active.len());
    for (rect_idx, (_slot, session_id)) in active.iter().enumerate() {
        if let Some(rect) = rects.get(rect_idx) {
            let _ = state.client.resize_session(
                session_id,
                rect.cols as u16, rect.rows as u16,
                font.cell_width.ceil() as u16, font.cell_height.ceil() as u16,
            );
        }
    }
}

fn quick_terminal_rect(cols: usize, rows: usize) -> PaneRect {
    let overlay_cols = (cols * 80 / 100).max(40).min(cols.saturating_sub(4));
    let overlay_rows = (rows * 70 / 100).max(15).min(rows.saturating_sub(4));
    let start_col = cols.saturating_sub(overlay_cols) / 2;
    let start_row = rows.saturating_sub(overlay_rows) / 2;
    PaneRect { col: start_col, row: start_row, cols: overlay_cols, rows: overlay_rows }
}

fn resize_quick_terminal(state: &mut AppState, font: &FontInfo) {
    if let Some(qt) = &state.quick_terminal {
        if qt.visible {
            let rect = quick_terminal_rect(state.cols as usize, state.rows as usize);
            let inner_cols = rect.cols.saturating_sub(2);
            let inner_rows = rect.rows.saturating_sub(2);
            let _ = state.client.resize_session(
                &qt.session_id,
                inner_cols as u16, inner_rows as u16,
                font.cell_width.ceil() as u16, font.cell_height.ceil() as u16,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Global command handler (Cmd keybindings)
// ---------------------------------------------------------------------------

fn handle_global_command(cmd: &str, key: &str, state: &mut AppState, event_loop: &ActiveEventLoop, font: &FontInfo) {
    match cmd {
        "session.create.shell" | "session.create.agent" => {
            let agent_type = if cmd == "session.create.agent" { "claude" } else { "shell" };
            let default = dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "~".to_string());
            state.modal = Some(Modal::PathInput {
                agent_type: agent_type.to_string(),
                input: default.clone(),
                cursor_pos: default.len(),
            });
        }
        "session.close" => {
            if state.modal.is_some() {
                state.modal = None;
            } else if let Focus::Pane(idx) = &state.focus {
                let idx = *idx;
                // Remove session from pane slot (session keeps running).
                state.panes[idx] = None;
                state.focus = state.nearest_occupied_pane(idx)
                    .map(Focus::Pane)
                    .unwrap_or(Focus::Sidebar);
                resize_pane_sessions(state, font);
            } else {
                event_loop.exit();
            }
        }
        "session.kill" => {
            match &state.focus {
                Focus::Sidebar => {
                    if state.sidebar_selected < state.sessions.len() {
                        let id = state.sessions[state.sidebar_selected].id.clone();
                        let _ = state.client.kill_session(&id);
                        state.remove_from_panes(&id);
                        resize_pane_sessions(state, font);
                    }
                }
                Focus::Pane(idx) => {
                    let idx = *idx;
                    if let Some(id) = state.panes[idx].clone() {
                        let _ = state.client.kill_session(&id);
                        state.panes[idx] = None;
                        state.focus = state.nearest_occupied_pane(idx)
                            .map(Focus::Pane)
                            .unwrap_or(Focus::Sidebar);
                        resize_pane_sessions(state, font);
                    }
                }
            }
        }
        "pane.focus" => {
            if let Ok(n) = key.parse::<usize>() {
                let slot = n - 1; // key "1" → slot 0
                if slot < 4 && state.panes[slot].is_some() {
                    state.focus = Focus::Pane(slot);
                }
            }
        }
        "pane.assign" => {
            // Assign the sidebar-selected session to pane slot N (⌘⇧1-4).
            if let Ok(n) = key.parse::<usize>() {
                let slot = n - 1; // key "1" → slot 0
                if slot > 3 { return; }

                // Determine which session to assign.
                let session_id = if state.focus == Focus::Sidebar {
                    state.sessions.get(state.sidebar_selected)
                        .filter(|s| s.status != "exited")
                        .map(|s| s.id.clone())
                } else if let Focus::Pane(pane_idx) = &state.focus {
                    state.panes[*pane_idx].clone()
                } else {
                    None
                };

                if let Some(sid) = session_id {
                    // Remove from current slot if already placed elsewhere.
                    state.remove_from_panes(&sid);
                    // Place in the target slot (replaces whatever was there).
                    state.panes[slot] = Some(sid);
                    state.focus = Focus::Pane(slot);
                    resize_pane_sessions(state, font);
                }
            }
        }
        "quick_terminal.toggle" => {
            if let Some(qt) = &mut state.quick_terminal {
                qt.visible = !qt.visible;
                if qt.visible {
                    resize_quick_terminal(state, font);
                }
            } else {
                let home = dirs::home_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "~".to_string());
                match state.client.create_session("Quick Terminal", "shell", Some(&home), None) {
                    Ok(session_id) => {
                        state.quick_terminal = Some(QuickTerminal {
                            session_id,
                            visible: true,
                        });
                        resize_quick_terminal(state, font);
                    }
                    Err(e) => tracing::error!("Failed to create quick terminal: {}", e),
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Sidebar input (when sidebar is focused, no modal)
// ---------------------------------------------------------------------------

fn handle_sidebar_input(key: &Key, state: &mut AppState, font: &FontInfo) {
    match key {
        Key::Named(NamedKey::Tab) => {
            // Toggle between Sessions and Tickets sections.
            state.sidebar_section = match state.sidebar_section {
                SidebarSection::Sessions => SidebarSection::Tickets,
                SidebarSection::Tickets => SidebarSection::Sessions,
            };
        }
        Key::Named(NamedKey::ArrowUp) => {
            match state.sidebar_section {
                SidebarSection::Sessions => {
                    if state.sidebar_selected > 0 { state.sidebar_selected -= 1; }
                }
                SidebarSection::Tickets => {
                    if state.ticket_selected > 0 { state.ticket_selected -= 1; }
                }
            }
        }
        Key::Named(NamedKey::ArrowDown) => {
            match state.sidebar_section {
                SidebarSection::Sessions => {
                    if state.sidebar_selected + 1 < state.sessions.len() { state.sidebar_selected += 1; }
                }
                SidebarSection::Tickets => {
                    if state.ticket_selected + 1 < state.tickets.len() { state.ticket_selected += 1; }
                }
            }
        }
        Key::Named(NamedKey::Enter) => {
            match state.sidebar_section {
                SidebarSection::Sessions => {
                    if state.sidebar_selected < state.sessions.len() {
                        let session = &state.sessions[state.sidebar_selected];
                        if session.status == "exited" { return; }
                        let sid = session.id.clone();

                        if let Some(slot) = state.pane_slot_for(&sid) {
                            state.focus = Focus::Pane(slot);
                        } else if let Some(slot) = state.first_empty_slot() {
                            state.panes[slot] = Some(sid);
                            state.focus = Focus::Pane(slot);
                            resize_pane_sessions(state, font);
                        }
                    }
                }
                SidebarSection::Tickets => {
                    // Enter on a ticket: show ticket detail popup.
                    if state.ticket_selected < state.tickets.len() {
                        let ticket = state.tickets[state.ticket_selected].clone();
                        state.modal = Some(Modal::TicketDetail { ticket });
                    }
                }
            }
        }
        Key::Character(c) => {
            match c.as_str() {
                "n" | "N" => {
                    let default = dirs::home_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "~".to_string());
                    state.modal = Some(Modal::PathInput {
                        agent_type: "shell".to_string(),
                        input: default.clone(),
                        cursor_pos: default.len(),
                    });
                }
                "c" | "C" => {
                    let default = dirs::home_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "~".to_string());
                    state.modal = Some(Modal::PathInput {
                        agent_type: "claude".to_string(),
                        input: default.clone(),
                        cursor_pos: default.len(),
                    });
                }
                "r" | "R" => {
                    if state.sidebar_selected < state.sessions.len() {
                        let name = state.sessions[state.sidebar_selected].name.clone();
                        state.modal = Some(Modal::RenameInput {
                            session_index: state.sidebar_selected,
                            input: name.clone(),
                            cursor_pos: name.len(),
                        });
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Open a Claude session pre-loaded with ticket context.
fn open_ticket_session(state: &mut AppState, name: &str, working_dir: &str, context_prompt: &str, font: &FontInfo) {
    match state.client.create_session(name, "claude", Some(working_dir), None) {
        Ok(session_id) => {
            // Send the ticket context as the first prompt to the Claude session.
            // Wait a moment for the session to initialize, then send context.
            let prompt = format!("{}\r", context_prompt);
            // We'll send it after a brief delay to let the PTY start.
            let sid_for_input = session_id.clone();
            let client_prompt = prompt;

            if let Some(slot) = state.first_empty_slot() {
                state.panes[slot] = Some(session_id);
                state.focus = Focus::Pane(slot);
                resize_pane_sessions(state, font);
            } else {
                state.focus = Focus::Sidebar;
                if let Ok(sessions) = state.client.list_sessions() {
                    if let Some(pos) = sessions.iter().position(|s| s.id == sid_for_input) {
                        state.sidebar_selected = pos;
                    }
                    state.sessions = sessions;
                }
            }

            // Queue the context prompt to be sent to the session.
            // The daemon's Claude session auto-types `claude` — we send our prompt after.
            let _ = state.client.send_input(&sid_for_input, client_prompt.as_bytes());
        }
        Err(e) => tracing::error!("Failed to create ticket session: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Modal input (path / rename dialogs)
// ---------------------------------------------------------------------------

fn handle_modal_input(key: &Key, state: &mut AppState, font: &FontInfo) {
    let modal = match &mut state.modal { Some(m) => m, None => return };

    match modal {
        Modal::PathInput { agent_type, input, cursor_pos } => {
            match key {
                Key::Named(NamedKey::Enter) => {
                    let path_str = shellexpand::tilde(input).to_string();
                    let path = std::path::PathBuf::from(&path_str);

                    if *agent_type == "claude" {
                        // Transition to branch picker for Claude sessions.
                        let branches = list_git_branches(&path);
                        let at = agent_type.clone();
                        if branches.is_empty() {
                            // Not a git repo — create directly without a worktree.
                            let dir_name = path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.to_string_lossy().to_string());
                            let n = state.sessions.len() + 1;
                            let name = format!("Claude {} \u{2014} {}", n, dir_name);
                            state.modal = None;
                            create_session(state, &name, &at, Some(path), None, font);
                        } else {
                            state.modal = Some(Modal::BranchInput {
                                agent_type: at,
                                working_dir: path,
                                branches,
                                input: String::new(),
                                cursor_pos: 0,
                                selected: 0,
                            });
                        }
                        return;
                    }

                    let dir_name = path.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| path.to_string_lossy().to_string());
                    let n = state.sessions.len() + 1;
                    let name = format!("Shell {} \u{2014} {}", n, dir_name);
                    state.modal = None;
                    create_session(state, &name, "shell", Some(path), None, font);
                }
                Key::Named(NamedKey::Escape) => { state.modal = None; }
                Key::Named(NamedKey::Backspace) => {
                    if *cursor_pos > 0 { input.remove(*cursor_pos - 1); *cursor_pos -= 1; }
                }
                Key::Named(NamedKey::ArrowLeft) => { if *cursor_pos > 0 { *cursor_pos -= 1; } }
                Key::Named(NamedKey::ArrowRight) => { if *cursor_pos < input.len() { *cursor_pos += 1; } }
                Key::Named(NamedKey::Tab) => {
                    let expanded = shellexpand::tilde(input).to_string();
                    let path = std::path::Path::new(&expanded);
                    let (search_dir, prefix) = if path.is_dir() {
                        (path.to_path_buf(), String::new())
                    } else {
                        let parent = path.parent().unwrap_or(std::path::Path::new("/"));
                        let file_prefix = path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default();
                        (parent.to_path_buf(), file_prefix)
                    };
                    if let Ok(entries) = std::fs::read_dir(&search_dir) {
                        let matches: Vec<String> = entries
                            .filter_map(|e| e.ok())
                            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                            .filter(|e| {
                                let name = e.file_name().to_string_lossy().to_string();
                                name.starts_with(&prefix) && !name.starts_with('.')
                            })
                            .map(|e| e.path().to_string_lossy().to_string())
                            .collect();
                        if matches.len() == 1 {
                            let completed = format!("{}/", matches[0]);
                            *input = completed.clone();
                            *cursor_pos = completed.len();
                        }
                    }
                }
                Key::Character(c) => {
                    for ch in c.chars() { input.insert(*cursor_pos, ch); *cursor_pos += 1; }
                }
                _ => {}
            }
        }
        Modal::BranchInput { agent_type, working_dir, branches, input, cursor_pos, selected } => {
            // Filter branches by current input.
            let filtered: Vec<String> = if input.is_empty() {
                branches.clone()
            } else {
                let lower = input.to_lowercase();
                branches.iter()
                    .filter(|b| b.to_lowercase().contains(&lower))
                    .cloned()
                    .collect()
            };

            match key {
                Key::Named(NamedKey::Enter) => {
                    let base = if !filtered.is_empty() {
                        let idx = (*selected).min(filtered.len().saturating_sub(1));
                        Some(filtered[idx].clone())
                    } else if !input.is_empty() {
                        // Allow typing an arbitrary ref.
                        Some(input.clone())
                    } else {
                        None // HEAD
                    };
                    let dir_name = working_dir.file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| working_dir.to_string_lossy().to_string());
                    let branch_label = base.as_deref().unwrap_or("HEAD");
                    let n = state.sessions.len() + 1;
                    let name = format!("Claude {} \u{2014} {} ({})", n, dir_name, branch_label);
                    let wd = working_dir.clone();
                    let at = agent_type.clone();
                    state.modal = None;
                    create_session(state, &name, &at, Some(wd), base.as_deref(), font);
                }
                Key::Named(NamedKey::Escape) => { state.modal = None; }
                Key::Named(NamedKey::ArrowUp) => {
                    if *selected > 0 { *selected -= 1; }
                }
                Key::Named(NamedKey::ArrowDown) => {
                    if !filtered.is_empty() && *selected + 1 < filtered.len() {
                        *selected += 1;
                    }
                }
                Key::Named(NamedKey::Backspace) => {
                    if *cursor_pos > 0 { input.remove(*cursor_pos - 1); *cursor_pos -= 1; *selected = 0; }
                }
                Key::Named(NamedKey::ArrowLeft) => { if *cursor_pos > 0 { *cursor_pos -= 1; } }
                Key::Named(NamedKey::ArrowRight) => { if *cursor_pos < input.len() { *cursor_pos += 1; } }
                Key::Character(c) => {
                    for ch in c.chars() { input.insert(*cursor_pos, ch); *cursor_pos += 1; }
                    *selected = 0;
                }
                _ => {}
            }
        }
        Modal::RenameInput { session_index, input, cursor_pos } => {
            match key {
                Key::Named(NamedKey::Enter) => {
                    let idx = *session_index;
                    if idx < state.sessions.len() && !input.is_empty() {
                        let id = state.sessions[idx].id.clone();
                        let _ = state.client.rename_session(&id, input);
                    }
                    state.modal = None;
                }
                Key::Named(NamedKey::Escape) => { state.modal = None; }
                Key::Named(NamedKey::Backspace) => {
                    if *cursor_pos > 0 { input.remove(*cursor_pos - 1); *cursor_pos -= 1; }
                }
                Key::Named(NamedKey::ArrowLeft) => { if *cursor_pos > 0 { *cursor_pos -= 1; } }
                Key::Named(NamedKey::ArrowRight) => { if *cursor_pos < input.len() { *cursor_pos += 1; } }
                Key::Character(c) => {
                    for ch in c.chars() { input.insert(*cursor_pos, ch); *cursor_pos += 1; }
                }
                _ => {}
            }
        }
        Modal::TicketDetail { .. } => {
            match key {
                Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter) => {
                    state.modal = None;
                }
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Git branch listing (for branch picker)
// ---------------------------------------------------------------------------

fn list_git_branches(dir: &std::path::Path) -> Vec<String> {
    let output = match std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(dir)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut branches: Vec<String> = stdout.lines()
        .map(|l| l.to_string())
        .filter(|l| !l.is_empty())
        .collect();

    // Put the current branch (HEAD) first if we can determine it.
    if let Ok(head_output) = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
    {
        if head_output.status.success() {
            let current = String::from_utf8_lossy(&head_output.stdout).trim().to_string();
            if let Some(pos) = branches.iter().position(|b| *b == current) {
                branches.remove(pos);
                branches.insert(0, current);
            }
        }
    }

    branches
}

// ---------------------------------------------------------------------------
// Terminal input (when a pane is focused)
// ---------------------------------------------------------------------------

fn handle_terminal_input(key: &Key, mods: Modifiers, state: &mut AppState, session_id: &str) {
    // Shift+PageUp/PageDown: scroll the viewport instead of sending to PTY.
    if mods.shift {
        if let Key::Named(NamedKey::PageUp) = key {
            let _ = state.client.scroll_session(session_id, state.rows as i32);
            return;
        }
        if let Key::Named(NamedKey::PageDown) = key {
            let _ = state.client.scroll_session(session_id, -(state.rows as i32));
            return;
        }
    }

    match key {
        Key::Character(c) => {
            let mut bytes = c.as_bytes().to_vec();
            if mods.ctrl && bytes.len() == 1 {
                let b = bytes[0];
                if b.is_ascii_lowercase() { bytes = vec![b - b'a' + 1]; }
            }
            let _ = state.client.send_input(session_id, &bytes);
        }
        Key::Named(named) => {
            let bytes: &[u8] = match named {
                NamedKey::Enter => b"\r",
                NamedKey::Backspace => b"\x7f",
                NamedKey::Tab => b"\t",
                NamedKey::Escape => b"\x1b",
                NamedKey::ArrowUp => b"\x1b[A",
                NamedKey::ArrowDown => b"\x1b[B",
                NamedKey::ArrowRight => b"\x1b[C",
                NamedKey::ArrowLeft => b"\x1b[D",
                NamedKey::Home => b"\x1b[H",
                NamedKey::End => b"\x1b[F",
                NamedKey::PageUp => b"\x1b[5~",
                NamedKey::PageDown => b"\x1b[6~",
                NamedKey::Delete => b"\x1b[3~",
                NamedKey::Space => b" ",
                _ => return,
            };
            let _ = state.client.send_input(session_id, bytes);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Resize
// ---------------------------------------------------------------------------

fn handle_resize(state: &mut AppState, size: PhysicalSize<u32>, font: &FontInfo) {
    if size.width == 0 || size.height == 0 { return; }

    state.layer.set_drawable_size(core_graphics::geometry::CGSize::new(
        size.width as f64, size.height as f64,
    ));

    let cell_w = font.cell_width;
    let cell_h = font.cell_height;
    let new_cols = (size.width as f64 / state.scale_factor / cell_w).max(1.0) as u16;
    let new_rows = (size.height as f64 / state.scale_factor / cell_h).max(1.0) as u16;

    if new_cols != state.cols || new_rows != state.rows {
        state.cols = new_cols;
        state.rows = new_rows;
        resize_pane_sessions(state, font);
        resize_quick_terminal(state, font);
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_frame(state: &mut AppState, font: &FontInfo) {
    let size = state.window.inner_size();
    let viewport_w = size.width as f32;
    let viewport_h = size.height as f32;
    let scale = state.scale_factor as f32;
    let cols = state.cols as usize;
    let rows = state.rows as usize;
    let sf = state.scale_factor;

    // --- Pre-fetch terminal grids (releases borrow on client before atlas use) ---
    // Collect occupied pane slots: (slot_index, session_id).
    let active_panes: Vec<(usize, String)> = state.panes.iter().enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|id| (i, id.clone())))
        .collect();
    let pane_ids: Vec<String> = active_panes.iter().map(|(_, id)| id.clone()).collect();
    let mut pane_grids = Vec::with_capacity(pane_ids.len());
    for sid in &pane_ids {
        pane_grids.push(state.client.get_grid(sid));
    }

    // Pre-fetch quick terminal grid.
    let qt_grid = state.quick_terminal.as_ref()
        .filter(|qt| qt.visible)
        .map(|qt| state.client.get_grid(&qt.session_id));

    // --- Build cell buffer ---
    let mut cells: Vec<(usize, usize, char, [f32; 4], [f32; 4], bool)> = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            cells.push((col, row, ' ', colors::FG, colors::BG, false));
        }
    }

    // 1. Sidebar
    let sidebar_w = SIDEBAR_COLS.min(cols);
    let sidebar_infos: Vec<ui_renderer::SessionInfo> = state.sessions.iter().map(|e| {
        ui_renderer::SessionInfo {
            id: e.id.clone(), name: e.name.clone(),
            status: e.status.clone(), agent_type: e.agent_type.clone(),
            context_percent: e.context_percent,
        }
    }).collect();
    let sidebar_focused = state.focus == Focus::Sidebar && state.modal.is_none();
    let ticket_infos: Vec<ui_renderer::TicketInfo> = state.tickets.iter().map(|t| {
        ui_renderer::TicketInfo {
            key: t.key.clone(),
            title: t.title.clone(),
            status_icon: t.status_icon.clone(),
            priority_icon: t.priority_icon.clone(),
            provider: t.provider.clone(),
        }
    }).collect();
    let sidebar_cells = ui_renderer::build_sidebar(
        sidebar_w, rows, &sidebar_infos, state.sidebar_selected,
        &state.panes, sidebar_focused,
        &ticket_infos, state.ticket_selected, state.sidebar_section,
        &mut state.renderer.atlas, font, sf,
    );
    for cell in &sidebar_cells {
        if cell.0 < sidebar_w {
            let idx = cell.1 * cols + cell.0;
            if idx < cells.len() { cells[idx] = *cell; }
        }
    }

    // 2. Sidebar border
    let border_col = SIDEBAR_COLS;
    if border_col < cols {
        state.renderer.atlas.get_or_insert('\u{2502}', font, sf); // │
        for row in 0..rows {
            let idx = row * cols + border_col;
            if idx < cells.len() {
                cells[idx] = (border_col, row, '\u{2502}', colors::ANSI[8], colors::BG, false);
            }
        }
    }

    // 3. Terminal panes
    let rects = compute_pane_rects(cols, rows, active_panes.len());
    let focused_slot = match &state.focus { Focus::Pane(idx) => Some(*idx), _ => None };

    for (rect_idx, grid_result) in pane_grids.iter().enumerate() {
        let slot = active_panes[rect_idx].0;
        if let (Ok(grid), Some(rect)) = (grid_result, rects.get(rect_idx)) {
            let is_active = focused_slot == Some(slot);
            for cell in &grid.cells {
                let c = cell.col as usize;
                let r = cell.row as usize;
                if c >= rect.cols || r >= rect.rows { continue; }
                let col = rect.col + c;
                let row = rect.row + r;
                if col >= cols || row >= rows { continue; }
                let fg = [cell.fg[0] as f32/255.0, cell.fg[1] as f32/255.0, cell.fg[2] as f32/255.0, cell.fg[3] as f32/255.0];
                let mut bg = [cell.bg[0] as f32/255.0, cell.bg[1] as f32/255.0, cell.bg[2] as f32/255.0, cell.bg[3] as f32/255.0];
                let show_cursor = cell.is_cursor && is_active;
                if show_cursor { bg = colors::CURSOR; }
                if cell.ch > ' ' {
                    state.renderer.atlas.get_or_insert(cell.ch, font, sf);
                }
                let idx = row * cols + col;
                if idx < cells.len() {
                    cells[idx] = (col, row, cell.ch, fg, bg, show_cursor);
                }
            }
        }
    }

    // 4. Inter-pane borders
    if active_panes.len() >= 2 {
        let main_start = SIDEBAR_COLS + 1;
        let main_cols = cols.saturating_sub(main_start);
        let vcol = main_start + main_cols / 2;
        if vcol < cols {
            state.renderer.atlas.get_or_insert('\u{2502}', font, sf);
            for row in 0..rows {
                let idx = row * cols + vcol;
                if idx < cells.len() {
                    cells[idx] = (vcol, row, '\u{2502}', colors::ANSI[8], colors::BG, false);
                }
            }
        }

        if active_panes.len() >= 3 {
            let hrow = rows / 2;
            if hrow < rows {
                state.renderer.atlas.get_or_insert('\u{2500}', font, sf); // ─
                state.renderer.atlas.get_or_insert('\u{253C}', font, sf); // ┼
                for col in main_start..cols {
                    let idx = hrow * cols + col;
                    if idx < cells.len() {
                        let ch = if col == vcol { '\u{253C}' } else { '\u{2500}' };
                        cells[idx] = (col, hrow, ch, colors::ANSI[8], colors::BG, false);
                    }
                }
            }
        }
    }

    // 5. Empty main area message
    if active_panes.is_empty() && state.modal.is_none() {
        let main_start = SIDEBAR_COLS + 1;
        let main_cols = cols.saturating_sub(main_start);
        let msg = "Press Enter to open a session in a pane";
        let msg_col = main_start + main_cols.saturating_sub(msg.len()) / 2;
        let msg_row = rows / 2;
        for (i, ch) in msg.chars().enumerate() {
            let col = msg_col + i;
            if col < cols && msg_row < rows {
                if ch > ' ' { state.renderer.atlas.get_or_insert(ch, font, sf); }
                let idx = msg_row * cols + col;
                if idx < cells.len() {
                    cells[idx] = (col, msg_row, ch, colors::ANSI[10], colors::BG, false);
                }
            }
        }
    }

    // 6. Modal overlay (renders into main area)
    if let Some(modal) = &state.modal {
        let main_start = SIDEBAR_COLS + 1;
        let main_cols = cols.saturating_sub(main_start);
        let modal_cells = match modal {
            Modal::PathInput { agent_type, input, cursor_pos } => {
                ui_renderer::build_path_input(main_cols, rows, agent_type, input, *cursor_pos,
                    &mut state.renderer.atlas, font, sf)
            }
            Modal::BranchInput { branches, input, cursor_pos, selected, .. } => {
                ui_renderer::build_branch_input(main_cols, rows, branches, input, *cursor_pos, *selected,
                    &mut state.renderer.atlas, font, sf)
            }
            Modal::RenameInput { input, cursor_pos, .. } => {
                ui_renderer::build_rename_input(main_cols, rows, "", input, *cursor_pos,
                    &mut state.renderer.atlas, font, sf)
            }
            Modal::TicketDetail { ticket } => {
                let detail = ui_renderer::TicketDetailInfo {
                    key: ticket.key.clone(),
                    title: ticket.title.clone(),
                    description: ticket.description.clone(),
                    status: ticket.status.clone(),
                    status_icon: ticket.status_icon.clone(),
                    priority: ticket.priority.clone(),
                    priority_icon: ticket.priority_icon.clone(),
                    provider: ticket.provider.clone(),
                    url: ticket.url.clone(),
                    assignee: ticket.assignee.clone(),
                    labels: ticket.labels.clone(),
                };
                ui_renderer::build_ticket_detail(main_cols, rows, &detail,
                    &mut state.renderer.atlas, font, sf)
            }
        };
        for cell in &modal_cells {
            let col = cell.0 + main_start;
            let row = cell.1;
            if col < cols && row < rows {
                let idx = row * cols + col;
                if idx < cells.len() {
                    cells[idx] = (col, row, cell.2, cell.3, cell.4, cell.5);
                }
            }
        }
    }

    // 7. Quick terminal overlay
    if let Some(qt) = &state.quick_terminal {
        if qt.visible {
            // Dim everything behind the overlay.
            for cell in cells.iter_mut() {
                cell.3[0] *= 0.3;
                cell.3[1] *= 0.3;
                cell.3[2] *= 0.3;
                cell.4[0] *= 0.3;
                cell.4[1] *= 0.3;
                cell.4[2] *= 0.3;
            }

            let rect = quick_terminal_rect(cols, rows);
            let qt_bg: [f32; 4] = [0.08, 0.05, 0.05, 1.0];
            let border_fg = colors::ANSI[3]; // gold

            // Box-drawing characters.
            let h = '\u{2500}';  // ─
            let v = '\u{2502}';  // │
            let tl = '\u{250C}'; // ┌
            let tr = '\u{2510}'; // ┐
            let bl = '\u{2514}'; // └
            let br = '\u{2518}'; // ┘
            for ch in [h, v, tl, tr, bl, br] {
                state.renderer.atlas.get_or_insert(ch, font, sf);
            }

            // Fill overlay background.
            for r in rect.row..rect.row + rect.rows {
                for c in rect.col..rect.col + rect.cols {
                    if r < rows && c < cols {
                        let idx = r * cols + c;
                        if idx < cells.len() {
                            cells[idx] = (c, r, ' ', colors::FG, qt_bg, false);
                        }
                    }
                }
            }

            let top = rect.row;
            let bot = rect.row + rect.rows - 1;
            let left = rect.col;
            let right = rect.col + rect.cols - 1;

            // Top border.
            for c in left..=right {
                if top < rows && c < cols {
                    let ch = if c == left { tl } else if c == right { tr } else { h };
                    let idx = top * cols + c;
                    if idx < cells.len() {
                        cells[idx] = (c, top, ch, border_fg, qt_bg, false);
                    }
                }
            }

            // Title centered in top border.
            let title = " Quick Terminal ";
            let title_start = left + rect.cols.saturating_sub(title.len()) / 2;
            for (i, ch) in title.chars().enumerate() {
                let c = title_start + i;
                if c < right && top < rows && c < cols {
                    if ch > ' ' { state.renderer.atlas.get_or_insert(ch, font, sf); }
                    let idx = top * cols + c;
                    if idx < cells.len() {
                        cells[idx] = (c, top, ch, colors::ANSI[15], qt_bg, false);
                    }
                }
            }

            // Bottom border.
            for c in left..=right {
                if bot < rows && c < cols {
                    let ch = if c == left { bl } else if c == right { br } else { h };
                    let idx = bot * cols + c;
                    if idx < cells.len() {
                        cells[idx] = (c, bot, ch, border_fg, qt_bg, false);
                    }
                }
            }

            // Hint centered in bottom border.
            let hint = " \u{2318}T to close ";
            let hint_start = left + rect.cols.saturating_sub(hint.len()) / 2;
            for (i, ch) in hint.chars().enumerate() {
                let c = hint_start + i;
                if c < right && bot < rows && c < cols {
                    if ch > ' ' { state.renderer.atlas.get_or_insert(ch, font, sf); }
                    let idx = bot * cols + c;
                    if idx < cells.len() {
                        cells[idx] = (c, bot, ch, colors::ANSI[10], qt_bg, false);
                    }
                }
            }

            // Left and right borders.
            for r in (top + 1)..bot {
                if r < rows {
                    if left < cols {
                        let idx = r * cols + left;
                        if idx < cells.len() {
                            cells[idx] = (left, r, v, border_fg, qt_bg, false);
                        }
                    }
                    if right < cols {
                        let idx = r * cols + right;
                        if idx < cells.len() {
                            cells[idx] = (right, r, v, border_fg, qt_bg, false);
                        }
                    }
                }
            }

            // Terminal content inside border.
            if let Some(Ok(grid)) = &qt_grid {
                let inner_col = left + 1;
                let inner_row = top + 1;
                let inner_cols = rect.cols.saturating_sub(2);
                let inner_rows = rect.rows.saturating_sub(2);

                for cell in &grid.cells {
                    let c = cell.col as usize;
                    let r = cell.row as usize;
                    if c >= inner_cols || r >= inner_rows { continue; }
                    let col = inner_col + c;
                    let row = inner_row + r;
                    if col >= cols || row >= rows { continue; }
                    let fg = [cell.fg[0] as f32/255.0, cell.fg[1] as f32/255.0, cell.fg[2] as f32/255.0, cell.fg[3] as f32/255.0];
                    let mut bg = [cell.bg[0] as f32/255.0, cell.bg[1] as f32/255.0, cell.bg[2] as f32/255.0, cell.bg[3] as f32/255.0];
                    let show_cursor = cell.is_cursor;
                    if show_cursor { bg = colors::CURSOR; }
                    if cell.ch > ' ' {
                        state.renderer.atlas.get_or_insert(cell.ch, font, sf);
                    }
                    let idx = row * cols + col;
                    if idx < cells.len() {
                        cells[idx] = (col, row, cell.ch, fg, bg, show_cursor);
                    }
                }
            }
        }
    }

    // --- Metal render pass ---
    let drawable = match state.layer.next_drawable() { Some(d) => d, None => return };
    let texture = drawable.texture();
    let desc = RenderPassDescriptor::new();
    let ca = desc.color_attachments().object_at(0).unwrap();
    ca.set_texture(Some(texture));
    ca.set_load_action(MTLLoadAction::Clear);
    ca.set_clear_color(MTLClearColor::new(
        colors::BG[0] as f64, colors::BG[1] as f64, colors::BG[2] as f64, 0.0,
    ));
    ca.set_store_action(MTLStoreAction::Store);

    let command_buffer = state.command_queue.new_command_buffer();
    let encoder = command_buffer.new_render_command_encoder(desc);
    state.renderer.render(encoder, viewport_w, viewport_h, scale, &cells);
    encoder.end_encoding();
    command_buffer.present_drawable(drawable);
    command_buffer.commit();
}
