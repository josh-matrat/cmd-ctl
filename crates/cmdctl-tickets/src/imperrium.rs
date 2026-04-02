//! Imperrium ticket provider (Matrat proprietary).
//!
//! Connects to the Imperrium API. Since this is a proprietary system,
//! the implementation provides the structure with configurable endpoints.

use std::io::Write;
use std::process::Stdio;

use anyhow::{Context, Result};

use crate::config::ImperriumConfig;
use crate::provider::{Ticket, TicketPriority, TicketProvider, TicketStatus};

pub struct ImperriumProvider {
    config: ImperriumConfig,
}

impl ImperriumProvider {
    pub fn new(config: ImperriumConfig) -> Self {
        Self { config }
    }

    fn fetch_raw(&self) -> Result<String> {
        let mut url = format!(
            "{}/api/v1/tickets",
            self.config.url.trim_end_matches('/')
        );

        // Add query parameters.
        let mut params = Vec::new();
        if let Some(ref project) = self.config.project {
            params.push(format!("project={}", project));
        }
        if let Some(ref user) = self.config.user {
            params.push(format!("assignee={}", user));
        }
        params.push("status=open".to_string());

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let mut child = std::process::Command::new("curl")
            .args([
                "-sS", "--config", "-",
                "-H", "Content-Type: application/json",
                &url,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to execute curl for Imperrium API")?;

        // Pass auth header via stdin to avoid leaking in process args
        if let Some(mut stdin) = child.stdin.take() {
            let _ = write!(stdin, "header = \"Authorization: Bearer {}\"\n", self.config.api_token);
        }

        let output = child.wait_with_output()
            .context("Failed to wait for curl process")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Imperrium API request failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn parse_response(&self, json: &str) -> Result<Vec<Ticket>> {
        let mut tickets = Vec::new();

        // Look for a "tickets" or "data" or "items" array.
        let arr_key = if json.contains("\"tickets\"") {
            "\"tickets\""
        } else if json.contains("\"data\"") {
            "\"data\""
        } else if json.contains("\"items\"") {
            "\"items\""
        } else {
            return Ok(tickets);
        };

        let arr_pos = match json.find(arr_key) {
            Some(pos) => pos,
            None => return Ok(tickets),
        };
        let bracket = match json[arr_pos..].find('[') {
            Some(pos) => arr_pos + pos,
            None => return Ok(tickets),
        };

        let section = &json[bracket..];
        let mut depth = 0;
        let mut obj_start = None;
        let mut objects = Vec::new();
        let chars: Vec<char> = section.chars().collect();

        for (i, &ch) in chars.iter().enumerate() {
            match ch {
                '{' => {
                    if depth == 1 && obj_start.is_none() { obj_start = Some(i); }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth == 1 {
                        if let Some(start) = obj_start.take() {
                            let s: String = chars[start..=i].iter().collect();
                            objects.push(s);
                        }
                    }
                    if depth == 0 { break; }
                }
                '[' if depth == 0 => { depth = 1; }
                ']' if depth == 1 => { break; }
                _ => {}
            }
        }

        for obj in &objects {
            if let Some(ticket) = self.parse_ticket(obj) {
                tickets.push(ticket);
            }
        }

        Ok(tickets)
    }

    fn parse_ticket(&self, json: &str) -> Option<Ticket> {
        let key = extract_string_field(json, "key")
            .or_else(|| extract_string_field(json, "id"))?;

        let title = extract_string_field(json, "title")
            .or_else(|| extract_string_field(json, "summary"))
            .unwrap_or_default();

        let description = extract_string_field_escaped(json, "description")
            .unwrap_or_default();

        let status_str = extract_string_field(json, "status")
            .unwrap_or_else(|| "unknown".to_string());
        let priority_str = extract_string_field(json, "priority")
            .unwrap_or_else(|| "none".to_string());

        let assignee = extract_string_field(json, "assignee");

        let status = match status_str.to_lowercase().as_str() {
            s if s.contains("done") || s.contains("closed") || s.contains("resolved") => TicketStatus::Done,
            s if s.contains("progress") || s.contains("active") => TicketStatus::InProgress,
            s if s.contains("review") => TicketStatus::InReview,
            s if s.contains("block") => TicketStatus::Blocked,
            _ => TicketStatus::Todo,
        };

        let priority = match priority_str.to_lowercase().as_str() {
            s if s.contains("critical") || s.contains("urgent") => TicketPriority::Critical,
            s if s.contains("high") => TicketPriority::High,
            s if s.contains("medium") || s.contains("normal") => TicketPriority::Medium,
            s if s.contains("low") => TicketPriority::Low,
            _ => TicketPriority::None,
        };

        let url = extract_string_field(json, "url")
            .unwrap_or_else(|| {
                format!("{}/tickets/{}", self.config.url.trim_end_matches('/'), key)
            });

        Some(Ticket {
            key,
            title,
            description,
            status,
            priority,
            provider: "imperrium".to_string(),
            url,
            assignee,
            labels: Vec::new(),
        })
    }
}

impl TicketProvider for ImperriumProvider {
    fn name(&self) -> &str {
        "imperrium"
    }

    fn fetch_tickets(&self) -> Result<Vec<Ticket>> {
        let json = self.fetch_raw()?;
        self.parse_response(&json)
    }
}

fn extract_string_field(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let pos = json.find(&pattern)?;
    let after_key = &json[pos + pattern.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = after_key[colon_pos + 1..].trim_start();
    if after_colon.starts_with('"') {
        let end = after_colon[1..].find('"')?;
        Some(after_colon[1..1 + end].to_string())
    } else {
        None
    }
}

/// Like `extract_string_field` but handles JSON escape sequences (for multi-line descriptions).
fn extract_string_field_escaped(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let pos = json.find(&pattern)?;
    let after_key = &json[pos + pattern.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = after_key[colon_pos + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let mut result = String::new();
    let mut escaped = false;
    for ch in after_colon[1..].chars() {
        if escaped {
            match ch {
                'n' => result.push('\n'),
                'r' => {}
                't' => result.push('\t'),
                '"' => result.push('"'),
                '\\' => result.push('\\'),
                '/' => result.push('/'),
                _ => { result.push('\\'); result.push(ch); }
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(result);
        } else {
            result.push(ch);
        }
    }
    None
}
