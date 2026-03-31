use std::sync::Arc;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use metal::*;
use raw_window_handle::HasWindowHandle;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::window::{Window, WindowAttributes, WindowId};

use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};

use nexus_input::keybinding::{KeyBinding, KeybindingManager, Modifiers};
use nexus_renderer::grid_renderer::{colors, GridRenderer};
use nexus_renderer::text::FontInfo;
use nexus_terminal::session::{Session, SessionEvent, TermSize};

const FONT_NAME: &str = "Menlo";
const FONT_SIZE: f64 = 14.0;
const INITIAL_COLS: u16 = 80;
const INITIAL_ROWS: u16 = 24;

pub fn run() -> Result<()> {
    let event_loop = EventLoop::new().context("Failed to create event loop")?;
    let mut app = NexusApp::new()?;
    event_loop.run_app(&mut app).context("Event loop error")?;
    Ok(())
}

struct NexusApp {
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
    session: Session,
    event_rx: Receiver<SessionEvent>,
    cols: u16,
    rows: u16,
}

impl NexusApp {
    fn new() -> Result<Self> {
        let font = FontInfo::load(FONT_NAME, FONT_SIZE)?;
        let keybindings = KeybindingManager::new();
        Ok(Self {
            font,
            keybindings,
            state: None,
            modifiers: ModifiersState::empty(),
        })
    }
}

impl ApplicationHandler for NexusApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }

        let result = init_window(event_loop, &self.font);
        match result {
            Ok(state) => self.state = Some(state),
            Err(e) => {
                tracing::error!("Failed to initialize: {}", e);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let state = match &mut self.state {
            Some(s) => s,
            None => return,
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }

                let mods = Modifiers {
                    cmd: self.modifiers.super_key(),
                    shift: self.modifiers.shift_key(),
                    ctrl: self.modifiers.control_key(),
                    alt: self.modifiers.alt_key(),
                };

                if mods.cmd {
                    let key_str = match &event.logical_key {
                        Key::Character(c) => c.to_string(),
                        _ => String::new(),
                    };

                    if !key_str.is_empty() {
                        let binding = KeyBinding {
                            modifiers: mods,
                            key: key_str.clone(),
                        };

                        if let Some(cmd) = self.keybindings.lookup(&binding) {
                            tracing::info!("Command: {}", cmd);
                            handle_command(cmd, &key_str, state, event_loop);
                            return;
                        }
                    }
                }

                // Forward input to the PTY.
                match &event.logical_key {
                    Key::Character(c) => {
                        let mut bytes = c.as_bytes().to_vec();
                        if mods.ctrl && bytes.len() == 1 {
                            let b = bytes[0];
                            if b.is_ascii_lowercase() {
                                bytes = vec![b - b'a' + 1];
                            }
                        }
                        state.session.write(&bytes);
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
                        state.session.write(bytes);
                    }
                    _ => {}
                }
            }

            WindowEvent::Resized(size) => {
                handle_resize(state, size, &self.font);
            }

            WindowEvent::RedrawRequested => {
                // Drain terminal events.
                while let Ok(event) = state.event_rx.try_recv() {
                    match event {
                        SessionEvent::Exit => {
                            tracing::info!("Session exited");
                            event_loop.exit();
                            return;
                        }
                        SessionEvent::Title(title) => {
                            state.window.set_title(&title);
                        }
                        _ => {}
                    }
                }

                render_frame(state);
                state.window.request_redraw();
            }

            _ => {}
        }
    }
}

fn init_window(event_loop: &ActiveEventLoop, font: &FontInfo) -> Result<AppState> {
    let cell_w = font.cell_width;
    let cell_h = font.cell_height;
    let width = (INITIAL_COLS as f64 * cell_w) as u32;
    let height = (INITIAL_ROWS as f64 * cell_h) as u32;

    let attrs = WindowAttributes::default()
        .with_title("Nexus")
        .with_inner_size(LogicalSize::new(width, height));

    let window = Arc::new(event_loop.create_window(attrs).context("Failed to create window")?);

    // Set up Metal.
    let device = Device::system_default().context("No Metal device")?;
    let command_queue = device.new_command_queue();
    let layer = MetalLayer::new();
    layer.set_device(&device);
    layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    layer.set_presents_with_transaction(false);

    let physical = window.inner_size();
    layer.set_drawable_size(core_graphics::geometry::CGSize::new(
        physical.width as f64,
        physical.height as f64,
    ));

    // Attach the Metal layer to the window's NSView.
    unsafe {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;

        let raw = window.window_handle().unwrap().as_raw();
        if let raw_window_handle::RawWindowHandle::AppKit(handle) = raw {
            let ns_view = handle.ns_view.as_ptr() as *mut AnyObject;
            let _: () = msg_send![ns_view, setWantsLayer: true];
            let _: () = msg_send![ns_view, setLayer: layer.as_ref() as *const MetalLayerRef as *mut AnyObject];
        }
    }

    let renderer_font = FontInfo::load(FONT_NAME, FONT_SIZE)?;
    let renderer = GridRenderer::new(&device, renderer_font)?;

    let term_size = TermSize {
        columns: INITIAL_COLS,
        rows: INITIAL_ROWS,
        cell_width: cell_w.ceil() as u16,
        cell_height: cell_h.ceil() as u16,
    };

    let (session, event_rx) = Session::new(
        "main".to_string(),
        "Main".to_string(),
        term_size,
    ).context("Failed to create terminal session")?;

    window.request_redraw();

    Ok(AppState {
        window,
        device,
        command_queue,
        layer,
        renderer,
        session,
        event_rx,
        cols: INITIAL_COLS,
        rows: INITIAL_ROWS,
    })
}

fn handle_resize(state: &mut AppState, size: PhysicalSize<u32>, font: &FontInfo) {
    if size.width == 0 || size.height == 0 {
        return;
    }

    state.layer.set_drawable_size(core_graphics::geometry::CGSize::new(
        size.width as f64,
        size.height as f64,
    ));

    let cell_w = font.cell_width;
    let cell_h = font.cell_height;
    let new_cols = (size.width as f64 / cell_w) as u16;
    let new_rows = (size.height as f64 / cell_h) as u16;

    if new_cols != state.cols || new_rows != state.rows {
        state.cols = new_cols;
        state.rows = new_rows;

        let term_size = TermSize {
            columns: new_cols,
            rows: new_rows,
            cell_width: cell_w.ceil() as u16,
            cell_height: cell_h.ceil() as u16,
        };
        state.session.resize(term_size);
    }
}

fn handle_command(cmd: &str, _key: &str, _state: &mut AppState, event_loop: &ActiveEventLoop) {
    match cmd {
        "session.close" => {
            event_loop.exit();
        }
        _ => {
            tracing::debug!("Unhandled command: {}", cmd);
        }
    }
}

fn render_frame(state: &mut AppState) {
    let drawable = match state.layer.next_drawable() {
        Some(d) => d,
        None => return,
    };

    let texture = drawable.texture();
    let desc = RenderPassDescriptor::new();
    let color_attachment = desc.color_attachments().object_at(0).unwrap();
    color_attachment.set_texture(Some(texture));
    color_attachment.set_load_action(MTLLoadAction::Clear);
    color_attachment.set_clear_color(MTLClearColor::new(
        colors::BG[0] as f64,
        colors::BG[1] as f64,
        colors::BG[2] as f64,
        1.0,
    ));
    color_attachment.set_store_action(MTLStoreAction::Store);

    let command_buffer = state.command_queue.new_command_buffer();
    let encoder = command_buffer.new_render_command_encoder(desc);

    let size = state.window.inner_size();
    let viewport_w = size.width as f32;
    let viewport_h = size.height as f32;

    // Read terminal grid and build cell data.
    let cells = {
        let term = state.session.term.lock();
        let content = term.renderable_content();
        let mut cells = Vec::with_capacity(state.cols as usize * state.rows as usize);

        let cursor_point = content.cursor.point;

        for indexed in content.display_iter {
            let point = indexed.point;
            let col = point.column.0;
            let line = point.line.0;

            if line < 0 {
                continue;
            }
            let row = line as usize;

            let ch = indexed.cell.c;
            let is_cursor = point == cursor_point;

            let fg = term_color_to_rgba(&indexed.cell.fg, &colors::FG);
            let mut bg = term_color_to_rgba(&indexed.cell.bg, &colors::BG);

            if is_cursor {
                bg = colors::CURSOR;
            }

            cells.push((col, row, ch, fg, bg, is_cursor));
        }

        cells
    };

    state.renderer.render(
        encoder,
        viewport_w,
        viewport_h,
        state.cols as usize,
        state.rows as usize,
        &cells,
    );

    encoder.end_encoding();
    command_buffer.present_drawable(drawable);
    command_buffer.commit();
}

/// Convert alacritty terminal color to RGBA float array.
fn term_color_to_rgba(color: &TermColor, default: &[f32; 4]) -> [f32; 4] {
    match color {
        TermColor::Named(name) => {
            let idx = match name {
                NamedColor::Black => 0,
                NamedColor::Red => 1,
                NamedColor::Green => 2,
                NamedColor::Yellow => 3,
                NamedColor::Blue => 4,
                NamedColor::Magenta => 5,
                NamedColor::Cyan => 6,
                NamedColor::White => 7,
                NamedColor::BrightBlack => 8,
                NamedColor::BrightRed => 9,
                NamedColor::BrightGreen => 10,
                NamedColor::BrightYellow => 11,
                NamedColor::BrightBlue => 12,
                NamedColor::BrightMagenta => 13,
                NamedColor::BrightCyan => 14,
                NamedColor::BrightWhite => 15,
                NamedColor::Foreground => return colors::FG,
                NamedColor::Background => return colors::BG,
                _ => return *default,
            };
            colors::ANSI[idx]
        }
        TermColor::Spec(rgb) => {
            [rgb.r as f32 / 255.0, rgb.g as f32 / 255.0, rgb.b as f32 / 255.0, 1.0]
        }
        TermColor::Indexed(idx) => {
            if (*idx as usize) < 16 {
                colors::ANSI[*idx as usize]
            } else {
                *default
            }
        }
    }
}
