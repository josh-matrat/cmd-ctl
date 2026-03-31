# CMD CTL

A GPU-accelerated terminal emulator and AI agent orchestrator for macOS.

CMD CTL is a native macOS terminal built with Metal rendering that doubles as a command center for managing multiple shell and AI agent sessions side-by-side. It uses [alacritty_terminal](https://github.com/alacritty/alacritty) for terminal emulation and renders everything through a custom Metal pipeline.

> **Status:** Early alpha. Expect rough edges, missing features, and breaking changes. Feedback and contributions welcome.

## Features

- **Metal-rendered terminal** — GPU-accelerated text rendering via a custom Metal shader pipeline
- **Session management** — Run multiple shell and Claude Code sessions in a 2x2 pane grid
- **Persistent daemon** — Sessions survive window close; reconnect anytime
- **Block detection** — Automatically detects when sessions are waiting for input or idle
- **Context estimation** — Tracks approximate context window usage for AI agent sessions
- **Knowledge base** — Scoped, searchable context storage shared across sessions
- **Ticket integration** — Pull tickets from Jira, Notion, or Imperrium into your workflow
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

# Knowledge and ticket management
cmdctl-cli knowledge ls
cmdctl-cli tickets list
```

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `n` | New shell session |
| `c` | New Claude Code session |
| `r` | Rename selected session |
| `Enter` | Attach to selected session |
| `Cmd+W` | Close / back to command center |
| `Cmd+T` | New shell session |
| `Cmd+1-9` | Switch to session by number |
| `Cmd+]` / `Cmd+[` | Next / previous session |
| `Cmd+K` | Kill selected session |

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
