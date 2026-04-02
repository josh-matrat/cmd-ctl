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

        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: ",".into() },
            "settings.open",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "t".into() },
            "quick_terminal.toggle",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "n".into() },
            "session.create.shell",
        );
        bindings.insert(
            KeyBinding { modifiers: Modifiers::CMD, key: "a".into() },
            "session.create.agent",
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
            KeyBinding { modifiers: Modifiers::CMD, key: "m".into() },
            "session.minimize",
        );

        // Cmd-1 through Cmd-4 for pane focus.
        for i in 1..=4u8 {
            bindings.insert(
                KeyBinding { modifiers: Modifiers::CMD, key: i.to_string() },
                "pane.focus",
            );
        }

        // Cmd-Shift-1 through Cmd-Shift-4 to assign a session to a pane slot.
        for i in 1..=4u8 {
            bindings.insert(
                KeyBinding { modifiers: Modifiers::CMD_SHIFT, key: i.to_string() },
                "pane.assign",
            );
        }

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
