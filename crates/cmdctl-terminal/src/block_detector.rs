use std::collections::VecDeque;
use std::time::{Duration, Instant};

use regex::Regex;

/// Detects when an agent/session is blocked waiting for input.
pub struct BlockDetector {
    last_output: Instant,
    recent_lines: VecDeque<String>,
    current_line: String,
    max_lines: usize,
    patterns: Vec<BlockPattern>,
    /// Once blocked, stays blocked until new output arrives.
    blocked: Option<BlockedReason>,
}

struct BlockPattern {
    regex: Regex,
    description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockedReason {
    /// No output for a while.
    Quiescence { idle_secs: u64 },
    /// Output matched a known "waiting for input" pattern.
    PatternMatch { description: String },
}

impl BlockDetector {
    pub fn new(agent_type: &str) -> Self {
        let patterns = match agent_type {
            "claude" => claude_patterns(),
            _ => shell_patterns(),
        };

        Self {
            last_output: Instant::now(),
            recent_lines: VecDeque::with_capacity(20),
            current_line: String::new(),
            max_lines: 20,
            patterns,
            blocked: None,
        }
    }

    /// Feed new output data to the detector. Clears blocked state.
    pub fn on_output(&mut self, data: &[u8]) {
        self.last_output = Instant::now();
        self.blocked = None;

        for &byte in data {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.current_line);
                if self.recent_lines.len() >= self.max_lines {
                    self.recent_lines.pop_front();
                }
                self.recent_lines.push_back(line);
            } else if byte != b'\r' {
                // Filter ANSI escape sequences for cleaner matching.
                if byte >= 0x20 || byte == b'\t' {
                    self.current_line.push(byte as char);
                }
            }
        }
    }

    /// Check if the session appears blocked. Call periodically (~1s interval).
    pub fn check(&mut self, quiescence_timeout_secs: u64) -> Option<&BlockedReason> {
        if self.blocked.is_some() {
            return self.blocked.as_ref();
        }

        // Check pattern match on the current (incomplete) line.
        // This catches prompts that don't end with a newline.
        if !self.current_line.is_empty() {
            let idle = self.last_output.elapsed();
            // Only pattern-match after a short delay (agent might still be writing).
            if idle >= Duration::from_millis(500) {
                for pattern in &self.patterns {
                    if pattern.regex.is_match(&self.current_line) {
                        self.blocked = Some(BlockedReason::PatternMatch {
                            description: pattern.description.clone(),
                        });
                        return self.blocked.as_ref();
                    }
                }
            }
        }

        // Also check the last completed line.
        if let Some(last_line) = self.recent_lines.back() {
            let idle = self.last_output.elapsed();
            if idle >= Duration::from_millis(500) {
                for pattern in &self.patterns {
                    if pattern.regex.is_match(last_line) {
                        self.blocked = Some(BlockedReason::PatternMatch {
                            description: pattern.description.clone(),
                        });
                        return self.blocked.as_ref();
                    }
                }
            }
        }

        // Quiescence check.
        let idle = self.last_output.elapsed().as_secs();
        if idle >= quiescence_timeout_secs {
            self.blocked = Some(BlockedReason::Quiescence { idle_secs: idle });
            return self.blocked.as_ref();
        }

        None
    }

    /// Whether the session is currently considered blocked.
    pub fn is_blocked(&self) -> bool {
        self.blocked.is_some()
    }

    pub fn blocked_reason(&self) -> Option<&BlockedReason> {
        self.blocked.as_ref()
    }
}

/// Patterns for Claude Code waiting for user input.
fn claude_patterns() -> Vec<BlockPattern> {
    vec![
        bp(r"^\s*❯\s*$", "Claude Code prompt"),
        bp(r"^\s*>\s*$", "Input prompt"),
        bp(r"\?\s*\(y/n\)", "Yes/no prompt"),
        bp(r"\?\s*\[Y/n\]", "Confirm prompt"),
        bp(r"\?\s*\[y/N\]", "Confirm prompt"),
        bp(r"Press Enter to continue", "Press enter"),
        bp(r"Do you want to proceed", "Proceed prompt"),
        bp(r"Would you like to", "Choice prompt"),
        bp(r"\? ›", "Selection prompt"),
        bp(r"\? …", "Input prompt"),
        bp(r"permission", "Permission request"),
    ]
}

/// Generic patterns for shell sessions.
fn shell_patterns() -> Vec<BlockPattern> {
    vec![
        bp(r"\$\s*$", "Shell prompt"),
        bp(r"#\s*$", "Root prompt"),
        bp(r"❯\s*$", "Shell prompt"),
        bp(r"password:", "Password prompt"),
        bp(r"Password:", "Password prompt"),
        bp(r"\[Y/n\]", "Confirm prompt"),
        bp(r"\(yes/no\)", "Confirm prompt"),
    ]
}

fn bp(pattern: &str, description: &str) -> BlockPattern {
    BlockPattern {
        regex: Regex::new(pattern).unwrap(),
        description: description.to_string(),
    }
}
