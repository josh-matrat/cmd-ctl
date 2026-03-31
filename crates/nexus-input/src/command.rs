/// Command registry - will be expanded in Phase 3.

#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

/// Returns the default command list.
pub fn default_commands() -> Vec<CommandInfo> {
    vec![
        CommandInfo { id: "session.create", name: "New Session", description: "Create a new terminal session" },
        CommandInfo { id: "session.close", name: "Close Session", description: "Close the current session" },
        CommandInfo { id: "session.kill", name: "Kill Session", description: "Kill the session's process" },
        CommandInfo { id: "session.switch", name: "Switch Session", description: "Switch to a session by number" },
        CommandInfo { id: "session.next", name: "Next Session", description: "Switch to the next session" },
        CommandInfo { id: "session.prev", name: "Previous Session", description: "Switch to the previous session" },
        CommandInfo { id: "session.detach", name: "Detach", description: "Detach from the current session" },
        CommandInfo { id: "palette.open", name: "Command Palette", description: "Open the command palette" },
        CommandInfo { id: "search.open", name: "Search", description: "Search across session output" },
    ]
}
