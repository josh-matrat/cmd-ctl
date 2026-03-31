use std::collections::HashMap;

/// A command identifier like "session.create".
pub type CommandId = &'static str;

/// Modifier flags for key bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub cmd: bool,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Modifiers {
    pub const NONE: Self = Self { cmd: false, shift: false, ctrl: false, alt: false };
    pub const CMD: Self = Self { cmd: true, shift: false, ctrl: false, alt: false };
    pub const CMD_SHIFT: Self = Self { cmd: true, shift: true, ctrl: false, alt: false };
}

/// A key binding: modifier + key name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub modifiers: Modifiers,
    pub key: String,
}

/// Manages key bindings and dispatches commands.
pub struct KeybindingManager {
    bindings: HashMap<KeyBinding, CommandId>,
}

impl KeybindingManager {
    pub fn new() -> Self {
        let mut bindings = HashMap::new();

        // Default Cmd-based keybindings.
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "t".into() },
            "session.create",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "w".into() },
            "session.close",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "k".into() },
            "session.kill",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "d".into() },
            "session.detach",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD_SHIFT, key: "p".into() },
            "palette.open",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "/".into() },
            "search.open",
        );

        // Cmd-1 through Cmd-9 for session switching.
        for i in 1..=9u8 {
            bindings.insert(
                KeyBinding { modifiers: Modifiers::CMD, key: i.to_string() },
                // We'll handle the argument part in the command dispatch.
                "session.switch",
            );
        }

        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD_SHIFT, key: "]".into() },
            "session.next",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD_SHIFT, key: "[".into() },
            "session.prev",
        );

        Self { bindings }
    }

    /// Look up a command for a key binding. Returns None if the key should be
    /// forwarded to the terminal.
    pub fn lookup(&self, binding: &KeyBinding) -> Option<CommandId> {
        self.bindings.get(binding).copied()
    }
}

impl Default for KeybindingManager {
    fn default() -> Self {
        Self::new()
    }
}
