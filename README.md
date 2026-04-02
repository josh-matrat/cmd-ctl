# CMD CTL

A GPU-accelerated terminal emulator and AI agent orchestrator for macOS.

CMD CTL is a native macOS terminal built with Metal rendering that doubles as a command center for managing multiple shell and AI agent sessions side-by-side. It uses [alacritty_terminal](https://github.com/alacritty/alacritty) for terminal emulation and renders everything through a custom Metal pipeline.
> **Status:** Early alpha. Expect rough edges, missing features, and breaking changes. Feedback and contributions welcome.


<img width="1232" height="739" alt="image" src="https://github.com/user-attachments/assets/3d4a2135-a417-4b44-a03f-4a5d7d7ac434" />

## Features

- **Metal-rendered terminal** — GPU-accelerated text rendering via a custom Metal shader pipeline
- **Quick terminal** — Toggle a fast terminal overlay with `Cmd+T`
- **Session management** — Run multiple shell and Claude Code sessions in a 2x2 pane grid with minimize/restore
- **Persistent daemon** — Sessions survive window close; reconnect anytime
- **Block detection** — Automatically detects when sessions are waiting for input or idle
- **Context estimation** — Tracks approximate context window usage for AI agent sessions
- **Knowledge base** — Scoped, searchable context storage shared across sessions
- **Ticket integration** — Pull tickets from Jira, Notion, or Imperrium; browse, create, update status, and launch agent sessions with full ticket context
- **Commands & Skills** — Extensible markdown-based commands and reusable Claude Code skill workflows, loaded from plugins or user-defined files
- **Native macOS** — Dark appearance, vibrancy effects, proper titlebar integration

## Requirements

- macOS (Apple Silicon or Intel with Metal support)
- Rust toolchain (install via [rustup](https://rustup.rs))

## Building

```bash
cargo build --release
```

The binary is output to `target/release/cmdctl`.

### macOS App Bundle

```bash
./bundle.sh
```

This creates `bundle/CMDCTL.app` which can be copied to `/Applications/`.

## Usage

```bash
# Run the app (starts the daemon automatically)
cmdctl

# CLI for managing sessions headlessly
cmdctl-cli list
cmdctl-cli kill <session-id>
cmdctl-cli dump <session-id>
cmdctl-cli shutdown

# Knowledge management
cmdctl-cli knowledge ls
cmdctl-cli knowledge search <query> [scope]
cmdctl-cli knowledge add <title> <scope> [tags]
cmdctl-cli knowledge remove <id>
cmdctl-cli knowledge context [dir]
cmdctl-cli knowledge summaries [dir]
```

### Keyboard Shortcuts

**Global**

| Shortcut | Action |
|----------|--------|
| `Cmd+T` | Toggle quick terminal overlay |
| `Cmd+N` | New shell session |
| `Cmd+A` | New Claude Code agent session |
| `Cmd+W` | Toggle ticket portal |
| `Cmd+M` | Minimize focused session (hide from panes) |
| `Cmd+K` | Kill selected session |
| `Cmd+1-4` | Switch to pane by number |
| `Cmd+Shift+1-4` | Assign selected session to pane slot |
| `Cmd+Arrow` | Navigate between panes |

**Sidebar**

| Shortcut | Action |
|----------|--------|
| `n` | New shell session |
| `c` | New Claude Code session |
| `r` | Rename selected session |
| `Enter` | Attach to selected session / open detail |
| `Tab` | Cycle: Sessions → Tickets → Skills → Commands |
| `Arrow Up/Down` | Navigate items in active section |
| `Cmd+Enter` | Start agent session on selected ticket |

## Ticket Integration

CMD CTL connects to external ticket providers so you can browse, create, and act on issues without leaving the terminal. Tickets are fetched by the daemon and auto-refresh every 5 minutes.

### Supported Providers

| Provider | Configuration |
|----------|---------------|
| **Jira** | URL, email, API token. Optional JQL filter, project key, issue type |
| **Notion** | API token, database ID. Custom property mappings for title/status/priority |
| **Imperrium** | API endpoint, token. Project/workspace and user identifiers |

Providers are configured in `~/.cmdctl/providers.toml`.

### Ticket Portal

Open the ticket portal with `Cmd+W` to get a full-screen overlay for managing tickets.

<img width="2656" height="1612" alt="image" src="https://github.com/user-attachments/assets/d3883b9c-86e2-4eb8-a8a5-1456cf726608" />

**List view** — Browse all tickets sorted by status and priority. Navigate with arrow keys, press `Enter` to view details, `s` to change status, `r` to rename, `n` to create a new ticket, or `R` to force-refresh from all providers.

**Detail view** — Full ticket info with markdown-formatted description. Scroll with arrow keys. Press `s` to update status or `r` to edit the title.

**Create view** — Press `n` from the list to create a new ticket. `Tab` cycles between title, description, and priority fields. `Enter` submits to the first provider that supports creation.

### Launching Agents on Tickets

Select a ticket in the portal or sidebar and press `Cmd+Enter` to spawn a Claude Code session pre-loaded with the ticket's key, title, status, priority, description, labels, and URL as context. The agent prompt is delivered automatically once the session is ready.

| Shortcut | Context | Action |
|----------|---------|--------|
| `Cmd+W` | Global | Toggle ticket portal |
| `Cmd+Enter` | Portal or sidebar | Start agent on selected ticket |
| `↑/↓` | Portal list | Navigate tickets |
| `Enter` | Portal list | View ticket detail |
| `s` | Portal list/detail | Change ticket status |
| `r` | Portal list/detail | Edit ticket title |
| `n` | Portal list | Create new ticket |
| `R` | Portal list | Force refresh |
| `Esc` | Portal | Close / back |

## Commands & Skills

The sidebar provides access to four sections, cycled with `Tab`: **Sessions**, **Tickets**, **Skills**, and **Commands**.

### Skills

Skills are reusable Claude Code procedures defined as markdown files with YAML frontmatter. They represent automation tasks, integration hooks, or custom workflows. Skills are loaded from:

- **Plugin marketplace:** `~/.claude/plugins/marketplaces/*/plugins/*/skills/*/SKILL.md`
- **User-defined:** `~/.claude/skills/*/SKILL.md`

Each skill file uses the format:

```yaml
---
name: "Skill Name"
description: "What this skill does"
---

Markdown content with instructions, steps, or decision trees...
```

<img width="987" height="777" alt="image" src="https://github.com/user-attachments/assets/09c6c180-11e5-4dc8-84df-ba63d914d7bb" />


### Commands

Commands are executable markdown definitions loaded from:

- **Plugin marketplace:** `~/.claude/plugins/marketplaces/*/plugins/*/commands/*.md`
- **User-defined:** `~/.claude/commands/*.md`

Same frontmatter format as skills, providing command-palette-style access to complex workflows.

### Browsing Skills & Commands

Navigate to the Skills or Commands section in the sidebar with `Tab`, use arrow keys to select, and press `Enter` to view the full markdown content in a scrollable modal.

## Session Management

CMD CTL runs multiple shell and Claude Code sessions in a 2x2 pane grid. Sessions are owned by the persistent daemon — they keep running even if the window is closed.


<img width="1505" height="924" alt="image" src="https://github.com/user-attachments/assets/1bed8c2e-4cbb-4f34-a9e6-ccc05c505812" />


### Minimize & Restore

Press `Cmd+M` to minimize the focused session. The session is removed from its pane slot but keeps running in the background. Minimized sessions remain visible in the sidebar and can be restored at any time:

- Select a session in the sidebar and press `Enter` to assign it to the next free pane
- Use `Cmd+Shift+1-4` to assign a session to a specific pane slot

When a session is minimized, focus shifts to the nearest occupied pane. If no panes are occupied, focus returns to the sidebar.

### Session Status

Each session in the sidebar shows its current state:

| Indicator | Meaning |
|-----------|---------|
| Running | Session active (in pane or minimized) |
| Blocked | Agent waiting for user input |
| Idle | Session inactive but still alive |
| Exited | Session terminated |

Agent sessions additionally display approximate context window usage (0–100%).

### Pane Layout

| Shortcut | Action |
|----------|--------|
| `Cmd+M` | Minimize focused session |
| `Cmd+1-4` | Focus pane by number |
| `Cmd+Shift+1-4` | Assign session to pane slot |
| `Cmd+Arrow` | Navigate between adjacent panes |
| `Cmd+K` | Kill focused session |
| `Cmd+N` | New shell session |
| `Cmd+A` | New Claude Code agent session |

## Architecture

CMD CTL is organized as a Cargo workspace:

```
cmdctl-app          UI frontend, window management, event loop
cmdctl-cli          CLI tool for headless session/daemon management
cmdctl-daemon       Background daemon, Unix socket IPC, SQLite persistence
cmdctl-renderer     Metal rendering pipeline, glyph atlas, UI drawing
cmdctl-terminal     Terminal emulation wrapper (alacritty_terminal), block detection
cmdctl-input        Keybinding configuration and input handling
cmdctl-knowledge    Context and knowledge store for AI sessions
cmdctl-tickets      Multi-provider ticket integration (Jira, Notion, Imperrium)
```

The app starts a background daemon that owns all terminal sessions. The UI connects to the daemon over a Unix socket (`~/.cmdctl/cmdctl.sock`). Sessions persist even after the window is closed — relaunch `cmdctl` to reconnect.

```
┌──────────────────────────┐
│      cmdctl-app (UI)     │──── cmdctl-renderer (Metal GPU)
│      cmdctl-input        │     cmdctl-knowledge
└───────────┬──────────────┘     cmdctl-tickets
            │ Unix socket
┌───────────┴──────────────┐
│    cmdctl-daemon         │
│    cmdctl-terminal (PTY) │
└──────────────────────────┘
```

## License

[MIT](LICENSE)
