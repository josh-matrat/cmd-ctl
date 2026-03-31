use std::collections::VecDeque;
use std::time::Instant;

/// Detects when an agent/session is blocked waiting for input.
pub struct BlockDetector {
    last_output: Instant,
    recent_lines: VecDeque<String>,
    current_line: String,
    max_lines: usize,
}

#[derive(Debug, Clone)]
pub enum BlockedReason {
    Quiescence { idle_secs: u64 },
    PatternMatch { pattern: String },
}

impl BlockDetector {
    pub fn new() -> Self {
        Self {
            last_output: Instant::now(),
            recent_lines: VecDeque::with_capacity(20),
            current_line: String::new(),
            max_lines: 20,
        }
    }

    /// Feed new output data to the detector.
    pub fn on_output(&mut self, data: &[u8]) {
        self.last_output = Instant::now();

        // Track recent lines for pattern matching.
        for &byte in data {
            if byte == b'\n' {
                let line = std::mem::take(&mut self.current_line);
                if self.recent_lines.len() >= self.max_lines {
                    self.recent_lines.pop_front();
                }
                self.recent_lines.push_back(line);
            } else if byte != b'\r' {
                self.current_line.push(byte as char);
            }
        }
    }

    /// Check if the session appears blocked. Call periodically.
    pub fn check(&self, quiescence_timeout_secs: u64) -> Option<BlockedReason> {
        let idle = self.last_output.elapsed().as_secs();
        if idle >= quiescence_timeout_secs {
            return Some(BlockedReason::Quiescence { idle_secs: idle });
        }
        None
    }
}

impl Default for BlockDetector {
    fn default() -> Self {
        Self::new()
    }
}
