use crate::atlas::GlyphAtlas;
use crate::grid_renderer::colors;
use crate::text::FontInfo;

/// A line of styled text for the Command Center.
pub struct StyledLine {
    pub text: String,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub centered: bool,
}

impl StyledLine {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_string(),
            fg: colors::FG,
            bg: colors::BG,
            centered: false,
        }
    }

    pub fn fg(mut self, color: [f32; 4]) -> Self {
        self.fg = color;
        self
    }

    pub fn bg(mut self, color: [f32; 4]) -> Self {
        self.bg = color;
        self
    }

    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }
}

/// Session info for display.
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub status: String,
    pub agent_type: String,
    pub context_percent: u8,
}

/// Ticket info for sidebar display.
#[derive(Clone)]
pub struct TicketInfo {
    pub key: String,
    pub title: String,
    pub status_icon: String,
    pub priority_icon: String,
    pub provider: String,
}

/// Sidebar background — slightly darker than main for visual separation.
const SIDEBAR_BG: [f32; 4] = [0.04, 0.02, 0.02, 0.95];

/// Render styled lines onto a pre-filled cell grid.
fn render_lines(
    lines: &[StyledLine],
    cols: usize,
    rows: usize,
    base_bg: [f32; 4],
    cells: &mut Vec<(usize, usize, char, [f32; 4], [f32; 4], bool)>,
    atlas: &mut GlyphAtlas,
    font: &FontInfo,
    scale: f64,
) {
    for (line_idx, styled_line) in lines.iter().enumerate() {
        if line_idx >= rows { break; }

        let text_chars: Vec<char> = styled_line.text.chars().collect();
        let start_col = if styled_line.centered {
            cols.saturating_sub(text_chars.len()) / 2
        } else {
            0
        };

        // Set background for the full row if it differs from the base.
        if styled_line.bg != base_bg {
            for col in 0..cols {
                let cell_idx = line_idx * cols + col;
                if cell_idx < cells.len() {
                    cells[cell_idx].4 = styled_line.bg;
                }
            }
        }

        for (char_idx, &ch) in text_chars.iter().enumerate() {
            let col = start_col + char_idx;
            if col >= cols { break; }
            if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
            let cell_idx = line_idx * cols + col;
            if cell_idx < cells.len() {
                cells[cell_idx] = (col, line_idx, ch, styled_line.fg, styled_line.bg, false);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sidebar (VS Code-style session explorer)
// ---------------------------------------------------------------------------

/// Which section of the sidebar is focused.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SidebarSection {
    Sessions,
    Tickets,
}

/// Build cell data for the sidebar panel.
pub fn build_sidebar(
    cols: usize,
    rows: usize,
    sessions: &[SessionInfo],
    selected_index: usize,
    pane_slots: &[Option<String>],
    focused: bool,
    tickets: &[TicketInfo],
    ticket_selected: usize,
    sidebar_section: SidebarSection,
    atlas: &mut GlyphAtlas,
    font: &FontInfo,
    scale: f64,
) -> Vec<(usize, usize, char, [f32; 4], [f32; 4], bool)> {
    let bg = SIDEBAR_BG;
    let mut cells = Vec::with_capacity(cols * rows);

    // Pre-fill with sidebar background.
    for row in 0..rows {
        for col in 0..cols {
            cells.push((col, row, ' ', colors::FG, bg, false));
        }
    }

    let sb = |text: &str| StyledLine::new(text).bg(bg);

    let mut lines: Vec<StyledLine> = Vec::new();

    lines.push(sb(""));
    lines.push(sb("CMD CTL").fg(colors::ANSI[3]).centered());
    lines.push(sb(""));

    // -- Sessions header --
    let sessions_hdr_fg = if focused && sidebar_section == SidebarSection::Sessions {
        colors::ANSI[3]
    } else {
        colors::ANSI[10]
    };
    lines.push(sb(" SESSIONS").fg(sessions_hdr_fg));

    if sessions.is_empty() {
        lines.push(sb("  (none)").fg(colors::ANSI[10]));
    } else {
        for (i, session) in sessions.iter().enumerate() {
            let pane_slot = pane_slots.iter().position(|p| p.as_deref() == Some(&session.id));
            let pane_label = match pane_slot {
                Some(n) => format!("{}", n + 1),
                None => " ".to_string(),
            };

            let is_blocked = session.status.starts_with("blocked");
            let is_idle = session.status.starts_with("idle");
            let is_exited = session.status == "exited";
            let needs_input = is_blocked || (is_idle && session.agent_type == "claude");

            let icon = if needs_input { "!" }
                else if is_exited { "x" }
                else if is_idle { "-" }
                else { "*" };

            let max_name = cols.saturating_sub(7);
            let name: String = session.name.chars().take(max_name).collect();
            let line = format!(" {} {} {}", pane_label, icon, name);

            let is_selected = i == selected_index && focused && sidebar_section == SidebarSection::Sessions;

            let fg = if needs_input {
                colors::ANSI[1]
            } else if is_exited {
                colors::ANSI[10]
            } else if is_selected {
                colors::ANSI[15]
            } else {
                colors::FG
            };

            let row_bg = if is_selected {
                [0.18, 0.08, 0.03, 0.85]
            } else if needs_input {
                [0.12, 0.03, 0.03, 0.95]
            } else {
                bg
            };

            lines.push(StyledLine::new(&line).fg(fg).bg(row_bg));
        }
    }

    // -- Tickets section --
    lines.push(sb(""));
    let tickets_hdr_fg = if focused && sidebar_section == SidebarSection::Tickets {
        colors::ANSI[3]
    } else {
        colors::ANSI[10]
    };
    lines.push(sb(" TICKETS").fg(tickets_hdr_fg));

    if tickets.is_empty() {
        lines.push(sb("  (none configured)").fg(colors::ANSI[10]));
    } else {
        let max_tickets = rows.saturating_sub(lines.len() + 12);
        for (i, ticket) in tickets.iter().take(max_tickets).enumerate() {
            let is_selected = i == ticket_selected && focused && sidebar_section == SidebarSection::Tickets;

            let max_title = cols.saturating_sub(ticket.key.len() + 6);
            let title: String = ticket.title.chars().take(max_title).collect();
            let line = format!(" {} {} {}", ticket.status_icon, ticket.key, title);

            let fg = if is_selected {
                colors::ANSI[15]
            } else {
                match ticket.status_icon.as_str() {
                    "!" => colors::ANSI[1],  // blocked = red
                    "*" => colors::ANSI[3],  // in progress = yellow
                    "~" => colors::ANSI[6],  // in review = cyan
                    "x" => colors::ANSI[10], // done = dim
                    _ => colors::FG,         // todo = default
                }
            };

            let row_bg = if is_selected {
                [0.18, 0.08, 0.03, 0.85]
            } else {
                bg
            };

            lines.push(StyledLine::new(&line).fg(fg).bg(row_bg));
        }
        if tickets.len() > max_tickets {
            let more = tickets.len() - max_tickets;
            lines.push(sb(&format!("  +{} more", more)).fg(colors::ANSI[10]));
        }
    }

    // -- Actions --
    lines.push(sb(""));
    lines.push(sb(" ACTIONS (\u{2318} + key)").fg(colors::ANSI[10]));
    lines.push(sb("  \u{2318}n Shell").fg(colors::ANSI[3]));
    lines.push(sb("  \u{2318}c Claude").fg(colors::ANSI[3]));
    lines.push(sb("  \u{2318}r Rename").fg(colors::ANSI[6]));
    lines.push(sb(""));
    lines.push(sb("  \u{2191}\u{2193}  Navigate").fg(colors::ANSI[8]));
    lines.push(sb("  Tab  Switch section").fg(colors::ANSI[8]));
    lines.push(sb("  Enter  Open/Work ticket").fg(colors::ANSI[8]));
    lines.push(sb("  \u{2318}1-4 Focus pane").fg(colors::ANSI[8]));
    lines.push(sb("  \u{2318}\u{2190}\u{2192} Move focus").fg(colors::ANSI[8]));

    render_lines(&lines, cols, rows, bg, &mut cells, atlas, font, scale);
    cells
}

// ---------------------------------------------------------------------------
// Modal: path input
// ---------------------------------------------------------------------------

/// Build cell data for the path input view.
pub fn build_path_input(
    cols: usize,
    rows: usize,
    agent_type: &str,
    input: &str,
    cursor_pos: usize,
    atlas: &mut GlyphAtlas,
    font: &FontInfo,
    scale: f64,
) -> Vec<(usize, usize, char, [f32; 4], [f32; 4], bool)> {
    let mut cells = Vec::new();

    let mut lines: Vec<StyledLine> = Vec::new();

    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("C M D C T L")
        .fg(colors::ANSI[3])
        .centered());
    lines.push(StyledLine::new(""));

    let label = if agent_type == "claude" {
        "New Claude Code Session"
    } else {
        "New Shell Session"
    };
    let label_color = if agent_type == "claude" { colors::ANSI[1] } else { colors::ANSI[3] };
    lines.push(StyledLine::new(&format!("  {}", label)).fg(label_color));
    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("  Working directory:").fg(colors::ANSI[7]));
    lines.push(StyledLine::new(""));
    // The input line will be drawn manually below for cursor support.
    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("  Tab        Complete path").fg(colors::ANSI[10]));
    lines.push(StyledLine::new("  Enter      Create session").fg(colors::ANSI[10]));
    lines.push(StyledLine::new("  Escape     Cancel").fg(colors::ANSI[10]));

    // Fill grid with background.
    for row in 0..rows {
        for col in 0..cols {
            cells.push((col, row, ' ', colors::FG, colors::BG, false));
        }
    }

    render_lines(&lines, cols, rows, colors::BG, &mut cells, atlas, font, scale);

    // Draw the input line at row 7 with a prompt and cursor.
    let input_row = 7;
    if input_row < rows {
        let prompt = "  > ";
        let prompt_chars: Vec<char> = prompt.chars().collect();
        let input_chars: Vec<char> = input.chars().collect();
        let input_fg = colors::ANSI[15]; // bright white
        let input_bg = [0.12, 0.08, 0.05, 1.0]; // slightly lighter

        // Highlight the input row background.
        for col in 0..cols {
            let cell_idx = input_row * cols + col;
            if cell_idx < cells.len() {
                cells[cell_idx].4 = input_bg;
            }
        }

        // Draw prompt.
        for (i, &ch) in prompt_chars.iter().enumerate() {
            if i >= cols { break; }
            let cell_idx = input_row * cols + i;
            if cell_idx < cells.len() {
                if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
                cells[cell_idx] = (i, input_row, ch, colors::ANSI[3], input_bg, false);
            }
        }

        // Draw input text.
        let offset = prompt_chars.len();
        for (i, &ch) in input_chars.iter().enumerate() {
            let col = offset + i;
            if col >= cols { break; }
            let is_cursor = i == cursor_pos;
            let bg = if is_cursor { colors::CURSOR } else { input_bg };
            if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
            let cell_idx = input_row * cols + col;
            if cell_idx < cells.len() {
                cells[cell_idx] = (col, input_row, ch, input_fg, bg, is_cursor);
            }
        }

        // Draw cursor at end of input.
        if cursor_pos >= input_chars.len() {
            let col = offset + cursor_pos;
            if col < cols {
                let cell_idx = input_row * cols + col;
                if cell_idx < cells.len() {
                    cells[cell_idx] = (col, input_row, ' ', input_fg, colors::CURSOR, true);
                }
            }
        }
    }

    cells
}

// ---------------------------------------------------------------------------
// Modal: rename input
// ---------------------------------------------------------------------------

/// Build cell data for the rename input view.
pub fn build_rename_input(
    cols: usize,
    rows: usize,
    _session_name: &str,
    input: &str,
    cursor_pos: usize,
    atlas: &mut GlyphAtlas,
    font: &FontInfo,
    scale: f64,
) -> Vec<(usize, usize, char, [f32; 4], [f32; 4], bool)> {
    let mut cells = Vec::new();

    for row in 0..rows {
        for col in 0..cols {
            cells.push((col, row, ' ', colors::FG, colors::BG, false));
        }
    }

    let header_lines: [(&str, [f32; 4]); 5] = [
        ("", colors::FG),
        ("  Rename Session", colors::ANSI[7]),
        ("", colors::FG),
        ("  New name:", colors::ANSI[7]),
        ("", colors::FG),
    ];

    for (line_idx, (text, fg)) in header_lines.iter().enumerate() {
        if line_idx >= rows { break; }
        for (i, ch) in text.chars().enumerate() {
            if i >= cols { break; }
            if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
            let idx = line_idx * cols + i;
            if idx < cells.len() {
                cells[idx] = (i, line_idx, ch, *fg, colors::BG, false);
            }
        }
    }

    let input_row = 5;
    if input_row < rows {
        let prompt = "  > ";
        let input_chars: Vec<char> = input.chars().collect();
        let input_fg = colors::ANSI[15];
        let input_bg = [0.12, 0.08, 0.05, 1.0];

        for col in 0..cols {
            let idx = input_row * cols + col;
            if idx < cells.len() { cells[idx].4 = input_bg; }
        }
        for (i, ch) in prompt.chars().enumerate() {
            if i >= cols { break; }
            if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
            let idx = input_row * cols + i;
            if idx < cells.len() {
                cells[idx] = (i, input_row, ch, colors::ANSI[3], input_bg, false);
            }
        }
        let offset = prompt.len();
        for (i, &ch) in input_chars.iter().enumerate() {
            let col = offset + i;
            if col >= cols { break; }
            let bg = if i == cursor_pos { colors::CURSOR } else { input_bg };
            if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
            let idx = input_row * cols + col;
            if idx < cells.len() {
                cells[idx] = (col, input_row, ch, input_fg, bg, i == cursor_pos);
            }
        }
        if cursor_pos >= input_chars.len() {
            let col = offset + cursor_pos;
            if col < cols {
                let idx = input_row * cols + col;
                if idx < cells.len() {
                    cells[idx] = (col, input_row, ' ', input_fg, colors::CURSOR, true);
                }
            }
        }
    }

    cells
}

// ---------------------------------------------------------------------------
// Modal: branch picker
// ---------------------------------------------------------------------------

/// Build cell data for the branch picker view.
pub fn build_branch_input(
    cols: usize,
    rows: usize,
    branches: &[String],
    filter: &str,
    cursor_pos: usize,
    selected: usize,
    atlas: &mut GlyphAtlas,
    font: &FontInfo,
    scale: f64,
) -> Vec<(usize, usize, char, [f32; 4], [f32; 4], bool)> {
    let mut cells = Vec::new();

    for row in 0..rows {
        for col in 0..cols {
            cells.push((col, row, ' ', colors::FG, colors::BG, false));
        }
    }

    let mut lines: Vec<StyledLine> = Vec::new();

    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("C M D C T L")
        .fg(colors::ANSI[3])
        .centered());
    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("  Base Branch").fg(colors::ANSI[1]));
    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("  Filter:").fg(colors::ANSI[7]));
    lines.push(StyledLine::new("")); // input row placeholder (row 6)
    lines.push(StyledLine::new(""));

    // Filter branches.
    let filtered: Vec<&String> = if filter.is_empty() {
        branches.iter().collect()
    } else {
        let lower = filter.to_lowercase();
        branches.iter().filter(|b| b.to_lowercase().contains(&lower)).collect()
    };

    // Show up to (rows - 12) branches to leave room for header/footer.
    let max_visible = rows.saturating_sub(12);
    let clamped_selected = selected.min(filtered.len().saturating_sub(1));

    // Scroll the list so the selected item is visible.
    let scroll_offset = if clamped_selected >= max_visible {
        clamped_selected - max_visible + 1
    } else {
        0
    };

    for (i, branch) in filtered.iter().enumerate().skip(scroll_offset).take(max_visible) {
        let is_selected = i == clamped_selected;
        let prefix = if is_selected { "> " } else { "  " };
        let label: String = branch.chars().take(cols.saturating_sub(6)).collect();
        let line_text = format!("  {}{}", prefix, label);
        let fg = if is_selected { colors::ANSI[15] } else { colors::FG };
        let bg = if is_selected {
            [0.18, 0.08, 0.03, 0.85]
        } else {
            colors::BG
        };
        lines.push(StyledLine::new(&line_text).fg(fg).bg(bg));
    }

    if filtered.is_empty() {
        lines.push(StyledLine::new("  (no matches)").fg(colors::ANSI[10]));
    }

    // Pad to push help text to end of branch list area.
    let used = lines.len();
    let footer_start = 8 + max_visible;
    for _ in used..footer_start {
        lines.push(StyledLine::new(""));
    }

    lines.push(StyledLine::new(""));
    lines.push(StyledLine::new("  \u{2191}\u{2193}       Select branch").fg(colors::ANSI[10]));
    lines.push(StyledLine::new("  Enter    Create session").fg(colors::ANSI[10]));
    lines.push(StyledLine::new("  Escape   Cancel").fg(colors::ANSI[10]));

    render_lines(&lines, cols, rows, colors::BG, &mut cells, atlas, font, scale);

    // Draw the filter input at row 6 with cursor.
    let input_row = 6;
    if input_row < rows {
        let prompt = "  > ";
        let prompt_chars: Vec<char> = prompt.chars().collect();
        let input_chars: Vec<char> = filter.chars().collect();
        let input_fg = colors::ANSI[15];
        let input_bg = [0.12, 0.08, 0.05, 1.0];

        for col in 0..cols {
            let cell_idx = input_row * cols + col;
            if cell_idx < cells.len() {
                cells[cell_idx].4 = input_bg;
            }
        }

        for (i, &ch) in prompt_chars.iter().enumerate() {
            if i >= cols { break; }
            let cell_idx = input_row * cols + i;
            if cell_idx < cells.len() {
                if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
                cells[cell_idx] = (i, input_row, ch, colors::ANSI[3], input_bg, false);
            }
        }

        let offset = prompt_chars.len();
        for (i, &ch) in input_chars.iter().enumerate() {
            let col = offset + i;
            if col >= cols { break; }
            let is_cursor = i == cursor_pos;
            let bg = if is_cursor { colors::CURSOR } else { input_bg };
            if ch > ' ' { atlas.get_or_insert(ch, font, scale); }
            let cell_idx = input_row * cols + col;
            if cell_idx < cells.len() {
                cells[cell_idx] = (col, input_row, ch, input_fg, bg, is_cursor);
            }
        }

        if cursor_pos >= input_chars.len() {
            let col = offset + cursor_pos;
            if col < cols {
                let cell_idx = input_row * cols + col;
                if cell_idx < cells.len() {
                    cells[cell_idx] = (col, input_row, ' ', input_fg, colors::CURSOR, true);
                }
            }
        }
    }

    cells
}
