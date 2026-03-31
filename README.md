# CMD CTL

A GPU-accelerated terminal emulator and AI agent orchestrator for macOS.

CMD CTL is a native macOS terminal built with Metal rendering that doubles as a command center for managing multiple shell and AI agent sessions. It uses [alacritty_terminal](https://github.com/alacritty/alacritty) for terminal emulation and renders everything through a custom Metal pipeline.

## Features

- **Metal-rendered terminal** — GPU-accelerated text rendering via a custom Metal shader pipeline
- **Session management** — Run multiple shell and Claude Code sessions side-by-side
- **Command center UI** — Switch between sessions, monitor status, and manage your workflow
- **Block detection** — Automatically detects when sessions are waiting for input or idle
- **Context estimation** — Tracks approximate context window usage for AI agent sessions
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

# CLI for managing sessions
cmdctl-cli list
cmdctl-cli kill <session-id>
cmdctl-cli dump <session-id>
cmdctl-cli shutdown
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

CMD CTL is organized as a Cargo workspace with six crates:

| Crate | Purpose |
|-------|---------|
| `cmdctl-app` | Main application, window management, event loop |
| `cmdctl-cli` | CLI tool for interacting with the daemon |
| `cmdctl-daemon` | Background daemon managing sessions via Unix socket |
| `cmdctl-renderer` | Metal rendering pipeline, glyph atlas, UI drawing |
| `cmdctl-terminal` | Terminal emulation wrapper, block detection |
| `cmdctl-input` | Keybinding configuration and input handling |

The app starts a background daemon that owns all terminal sessions. The UI connects to the daemon over a Unix socket (`~/.cmdctl/cmdctl.sock`). Sessions persist even after the window is closed — relaunch `cmdctl` to reconnect.

## License

[MIT](LICENSE)
