# CMD CTL

A GPU-accelerated terminal emulator and AI agent orchestrator for macOS.

CMD CTL is a native macOS terminal built with Metal rendering that doubles as a command center for managing multiple shell and AI agent sessions side-by-side. It uses [alacritty_terminal](https://github.com/alacritty/alacritty) for terminal emulation and renders everything through a custom Metal pipeline.
> **Status:** Early alpha. Expect rough edges, missing features, and breaking changes. Feedback and contributions welcome.


<img width="1232" height="739" alt="image" src="https://github.com/user-attachments/assets/3d4a2135-a417-4b44-a03f-4a5d7d7ac434" />

## Features

- **Metal-rendered terminal** — GPU-accelerated text rendering via a custom Metal shader pipeline
- **Quick terminal** — Toggle a fast terminal overlay with `Cmd+T`
- **Session management** — Run multiple shell and Claude Code sessions in a 2x2 pane grid
- **Persistent daemon** — Sessions survive window close; reconnect anytime
- **Block detection** — Automatically detects when sessions are waiting for input or idle
- **Context estimation** — Tracks approximate context window usage for AI agent sessions
- **Knowledge base** — Scoped, searchable context storage shared across sessions
- **Ticket integration** — Pull tickets from Jira, Notion, or Imperrium and launch Claude sessions with ticket context
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
| `Cmd+W` | Close / back to command center |
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
| `Enter` | Attach to selected session |
| `Tab` | Toggle between Sessions and Tickets |
| `Arrow Up/Down` | Navigate sessions/tickets |

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
