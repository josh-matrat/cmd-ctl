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

    fn supports_create(&self) -> bool {
        self.config.project_key.is_some()
    }

    fn supports_status_update(&self) -> bool {
        true
    }

    fn create_ticket(&self, title: &str, description: &str, priority: &TicketPriority) -> Result<Ticket> {
        let project_key = self.config.project_key.as_deref()
            .context("Jira project_key not configured — add project_key to [jira] in providers.toml")?;
        let issue_type = self.config.issue_type.as_deref().unwrap_or("Task");

        let priority_name = match priority {
            TicketPriority::Critical => "Highest",
            TicketPriority::High => "High",
            TicketPriority::Medium => "Medium",
            TicketPriority::Low => "Low",
            TicketPriority::None => "Medium",
        };

        // Escape JSON strings to prevent injection.
        let title_escaped = json_escape(title);
        let desc_escaped = json_escape(description);
        let project_escaped = json_escape(project_key);
        let type_escaped = json_escape(issue_type);
        let priority_escaped = json_escape(priority_name);

        let body = format!(
            r#"{{"fields":{{"project":{{"key":"{project_escaped}"}},"summary":"{title_escaped}","description":{{"type":"doc","version":1,"content":[{{"type":"paragraph","content":[{{"type":"text","text":"{desc_escaped}"}}]}}]}},"issuetype":{{"name":"{type_escaped}"}},"priority":{{"name":"{priority_escaped}"}}}}}}"#
        );

        let url = format!(
            "{}/rest/api/3/issue",
            self.config.url.trim_end_matches('/')
        );

        let auth = format!("{}:{}", self.config.email, self.config.api_token);
        let auth_header = format!("Basic {}", base64_encode(auth.as_bytes()));

        let mut child = std::process::Command::new("curl")
            .args(["-sS", "-X", "POST", "--config", "-",
                   "-H", "Content-Type: application/json",
                   "-d", &body, &url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to execute curl for Jira create")?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = write!(stdin, "header = \"Authorization: {}\"\n", auth_header);
        }

        let output = child.wait_with_output()
            .context("Failed to wait for curl process")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Jira create failed: {}", stderr);
        }

        let response = String::from_utf8_lossy(&output.stdout);
        let key = extract_string_field(&response, "key")
            .context("Failed to parse created ticket key from Jira response")?;

        let ticket_url = format!("{}/browse/{}", self.config.url.trim_end_matches('/'), key);

        Ok(Ticket {
            key,
            title: title.to_string(),
            description: description.to_string(),
            status: TicketStatus::Todo,
            priority: *priority,
            provider: "jira".to_string(),
            url: ticket_url,
            assignee: None,
            labels: Vec::new(),
        })
    }

    fn update_status(&self, key: &str, status: &TicketStatus) -> Result<()> {
        let target_name = status.label().to_lowercase();

        // 1. Fetch available transitions for this ticket.
        let transitions_url = format!(
            "{}/rest/api/3/issue/{}/transitions",
            self.config.url.trim_end_matches('/'),
            urlencoding(key),
        );

        let auth = format!("{}:{}", self.config.email, self.config.api_token);
        let auth_header = format!("Basic {}", base64_encode(auth.as_bytes()));

        let mut child = std::process::Command::new("curl")
            .args(["-sS", "--config", "-",
                   "-H", "Content-Type: application/json", &transitions_url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to fetch Jira transitions")?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = write!(stdin, "header = \"Authorization: {}\"\n", auth_header);
        }

        let output = child.wait_with_output()?;
        let json = String::from_utf8_lossy(&output.stdout);

        // 2. Find the transition ID matching the target status.
        let transition_id = find_transition_id(&json, &target_name)
            .with_context(|| format!("No Jira transition found for status '{}' on {}", status.label(), key))?;

        // 3. Execute the transition.
        let body = format!(r#"{{"transition":{{"id":"{}"}}}}"#, json_escape(&transition_id));

        let mut child = std::process::Command::new("curl")
            .args(["-sS", "-X", "POST", "--config", "-",
                   "-H", "Content-Type: application/json",
                   "-d", &body, &transitions_url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to execute Jira transition")?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = write!(stdin, "header = \"Authorization: {}\"\n", auth_header);
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Jira transition failed: {}", stderr);
        }

        Ok(())
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

/// Escape a string for safe inclusion in a JSON string literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < '\x20' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Find a Jira transition ID whose name contains the target status (case-insensitive).
fn find_transition_id(json: &str, target_lower: &str) -> Option<String> {
    // The response has: "transitions": [{"id": "31", "name": "Done", ...}, ...]
    // We parse it minimally by scanning for transition objects.
    let transitions_start = json.find("\"transitions\"")?;
    let section = &json[transitions_start..];
    let arr_start = section.find('[')?;
    let arr_section = &section[arr_start..];

    let mut depth = 0;
    let mut obj_start = None;
    let chars: Vec<char> = arr_section.chars().collect();

    for (i, &ch) in chars.iter().enumerate() {
        match ch {
            '{' => {
                if depth == 1 && obj_start.is_none() {
                    obj_start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 1 {
                    if let Some(start) = obj_start.take() {
                        let obj: String = chars[start..=i].iter().collect();
                        if let Some(name) = extract_string_field(&obj, "name") {
                            if name.to_lowercase().contains(target_lower) {
                                return extract_string_field(&obj, "id");
                            }
                        }
                    }
                }
                if depth == 0 { break; }
            }
            '[' if depth == 0 => { depth = 1; }
            ']' if depth == 1 => { break; }
            _ => {}
        }
    }
    None
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
