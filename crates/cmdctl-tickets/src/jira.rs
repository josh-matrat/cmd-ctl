//! Jira ticket provider.
//!
//! Uses the Jira REST API v3 with basic auth (email + API token).
//! Fetches tickets via JQL and maps them to the unified Ticket type.

use std::io::Write;
use std::process::Stdio;

use anyhow::{Context, Result};

use crate::config::JiraConfig;
use crate::provider::{Ticket, TicketPriority, TicketProvider, TicketStatus};

pub struct JiraProvider {
    config: JiraConfig,
}

impl JiraProvider {
    pub fn new(config: JiraConfig) -> Self {
        Self { config }
    }

    /// Build the JQL query. Uses the configured JQL or a sensible default.
    fn jql(&self) -> &str {
        self.config.jql.as_deref().unwrap_or(
            "assignee = currentUser() AND status != Done ORDER BY priority DESC, updated DESC"
        )
    }

    fn max_results(&self) -> u32 {
        self.config.max_results.unwrap_or(50)
    }

    /// Fetch tickets by shelling out to curl.
    /// This avoids pulling in reqwest (and its async runtime + TLS stack) as a dependency.
    /// The daemon runs on a background thread so blocking is fine.
    fn fetch_raw(&self) -> Result<String> {
        let url = format!(
            "{}/rest/api/3/search?jql={}&maxResults={}&fields=summary,status,priority,assignee,labels,description",
            self.config.url.trim_end_matches('/'),
            urlencoding(&self.jql()),
            self.max_results(),
        );

        let auth = format!("{}:{}", self.config.email, self.config.api_token);
        let auth_header = format!("Basic {}", base64_encode(auth.as_bytes()));

        let mut child = std::process::Command::new("curl")
            .args(["-sS", "--config", "-",
                   "-H", "Content-Type: application/json", &url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to execute curl for Jira API")?;

        // Pass auth header via stdin to avoid leaking in process args
        if let Some(mut stdin) = child.stdin.take() {
            let _ = write!(stdin, "header = \"Authorization: {}\"\n", auth_header);
        }

        let output = child.wait_with_output()
            .context("Failed to wait for curl process")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Jira API request failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn parse_response(&self, json: &str) -> Result<Vec<Ticket>> {
        // Minimal JSON parsing without serde_json — parse the essential fields.
        // We use a simple approach: find "issues" array and extract fields.
        let mut tickets = Vec::new();

        // Find the issues array
        let issues_start = match json.find("\"issues\"") {
            Some(pos) => pos,
            None => return Ok(tickets),
        };

        // Find the opening bracket of the array
        let arr_start = match json[issues_start..].find('[') {
            Some(pos) => issues_start + pos,
            None => return Ok(tickets),
        };

        // Split by issue objects — look for "key" fields
        let issues_section = &json[arr_start..];
        let mut depth = 0;
        let mut issue_start = None;
        let mut issue_strings = Vec::new();
        let chars: Vec<char> = issues_section.chars().collect();

        for (i, &ch) in chars.iter().enumerate() {
            match ch {
                '{' => {
                    if depth == 1 && issue_start.is_none() {
                        issue_start = Some(i);
                    }
                    depth += 1;
                }
                '}' => {
                    depth -= 1;
                    if depth == 1 {
                        if let Some(start) = issue_start.take() {
                            let s: String = chars[start..=i].iter().collect();
                            issue_strings.push(s);
                        }
                    }
                    if depth == 0 { break; }
                }
                '[' if depth == 0 => { depth = 1; }
                ']' if depth == 1 => { break; }
                _ => {}
            }
        }

        for issue_json in &issue_strings {
            if let Some(ticket) = self.parse_issue(issue_json) {
                tickets.push(ticket);
            }
        }

        Ok(tickets)
    }

    fn parse_issue(&self, json: &str) -> Option<Ticket> {
        let key = extract_string_field(json, "key")?;

        // Summary is inside "fields"
        let fields_start = json.find("\"fields\"")?;
        let fields = &json[fields_start..];

        let title = extract_string_field(fields, "summary").unwrap_or_default();
        let status_name = extract_nested_string(fields, "status", "name")
            .unwrap_or_else(|| "Unknown".to_string());
        let priority_name = extract_nested_string(fields, "priority", "name")
            .unwrap_or_else(|| "None".to_string());

        let assignee = extract_nested_string(fields, "assignee", "displayName");

        let status = match status_name.to_lowercase().as_str() {
            s if s.contains("done") || s.contains("closed") || s.contains("resolved") => TicketStatus::Done,
            s if s.contains("progress") || s.contains("active") => TicketStatus::InProgress,
            s if s.contains("review") => TicketStatus::InReview,
            s if s.contains("block") => TicketStatus::Blocked,
            s if s.contains("todo") || s.contains("open") || s.contains("new") || s.contains("backlog") => TicketStatus::Todo,
            _ => TicketStatus::Custom(status_name),
        };

        let priority = match priority_name.to_lowercase().as_str() {
            s if s.contains("critical") || s.contains("blocker") || s.contains("highest") => TicketPriority::Critical,
            s if s.contains("high") => TicketPriority::High,
            s if s.contains("medium") || s.contains("normal") => TicketPriority::Medium,
            s if s.contains("low") || s.contains("minor") => TicketPriority::Low,
            _ => TicketPriority::None,
        };

        let url = format!(
            "{}/browse/{}",
            self.config.url.trim_end_matches('/'),
            key
        );

        Some(Ticket {
            key,
            title,
            description: String::new(), // Skip full description for list view
            status,
            priority,
            provider: "jira".to_string(),
            url,
            assignee,
            labels: Vec::new(),
        })
    }
}

impl TicketProvider for JiraProvider {
    fn name(&self) -> &str {
        "jira"
    }

    fn fetch_tickets(&self) -> Result<Vec<Ticket>> {
        let json = self.fetch_raw()?;
        self.parse_response(&json)
    }
}

// ---------------------------------------------------------------------------
// Minimal helpers (avoid heavy dependencies)
// ---------------------------------------------------------------------------

fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Extract a string value for a top-level key like `"key": "PROJ-123"`.
fn extract_string_field(json: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let pos = json.find(&pattern)?;
    let after_key = &json[pos + pattern.len()..];
    // Skip whitespace and colon
    let colon_pos = after_key.find(':')?;
    let after_colon = after_key[colon_pos + 1..].trim_start();
    if after_colon.starts_with('"') {
        let start = 1;
        let end = after_colon[start..].find('"')?;
        Some(after_colon[start..start + end].to_string())
    } else {
        None
    }
}

/// Extract a nested string like `"status": { "name": "In Progress" }`.
fn extract_nested_string(json: &str, outer: &str, inner: &str) -> Option<String> {
    let pattern = format!("\"{}\"", outer);
    let pos = json.find(&pattern)?;
    let after = &json[pos..];
    let brace = after.find('{')?;
    let close = after[brace..].find('}')?;
    let obj = &after[brace..brace + close + 1];
    extract_string_field(obj, inner)
}
