//! Notion ticket provider.
//!
//! Queries a Notion database via the Notion API and maps entries to tickets.

use anyhow::{Context, Result};

use crate::config::NotionConfig;
use crate::provider::{Ticket, TicketPriority, TicketProvider, TicketStatus};

pub struct NotionProvider {
    config: NotionConfig,
}

impl NotionProvider {
    pub fn new(config: NotionConfig) -> Self {
        Self { config }
    }

    fn title_prop(&self) -> &str {
        self.config.title_property.as_deref().unwrap_or("Name")
    }

    fn status_prop(&self) -> &str {
        self.config.status_property.as_deref().unwrap_or("Status")
    }

    fn priority_prop(&self) -> &str {
        self.config.priority_property.as_deref().unwrap_or("Priority")
    }

    fn fetch_raw(&self) -> Result<String> {
        let url = format!(
            "https://api.notion.com/v1/databases/{}/query",
            self.config.database_id
        );

        // Build a filter body if assignee is set.
        let body = if let Some(ref assignee) = self.config.assignee {
            format!(
                r#"{{"filter":{{"property":"Assignee","people":{{"contains":"{}"}}}}}}"#,
                assignee
            )
        } else {
            "{}".to_string()
        };

        let output = std::process::Command::new("curl")
            .args([
                "-sS", "-X", "POST",
                "-H", &format!("Authorization: Bearer {}", self.config.api_token),
                "-H", "Content-Type: application/json",
                "-H", "Notion-Version: 2022-06-28",
                "-d", &body,
                &url,
            ])
            .output()
            .context("Failed to execute curl for Notion API")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Notion API request failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn parse_response(&self, json: &str) -> Result<Vec<Ticket>> {
        let mut tickets = Vec::new();

        // Find the "results" array.
        let results_start = match json.find("\"results\"") {
            Some(pos) => pos,
            None => return Ok(tickets),
        };
        let arr_start = match json[results_start..].find('[') {
            Some(pos) => results_start + pos,
            None => return Ok(tickets),
        };

        // Extract each page object at depth 1.
        let section = &json[arr_start..];
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
            if let Some(ticket) = self.parse_page(obj) {
                tickets.push(ticket);
            }
        }

        Ok(tickets)
    }

    fn parse_page(&self, json: &str) -> Option<Ticket> {
        let id = extract_string_field(json, "id")?;

        let url = extract_string_field(json, "url")
            .unwrap_or_else(|| format!("https://notion.so/{}", id.replace('-', "")));

        // Extract title from properties.
        let title = self.extract_title_from_properties(json)
            .unwrap_or_else(|| "Untitled".to_string());

        let status_name = self.extract_select_property(json, self.status_prop())
            .unwrap_or_else(|| "Unknown".to_string());
        let priority_name = self.extract_select_property(json, self.priority_prop())
            .unwrap_or_else(|| "None".to_string());

        let status = match status_name.to_lowercase().as_str() {
            s if s.contains("done") || s.contains("complete") => TicketStatus::Done,
            s if s.contains("progress") || s.contains("doing") => TicketStatus::InProgress,
            s if s.contains("review") => TicketStatus::InReview,
            s if s.contains("block") => TicketStatus::Blocked,
            _ => TicketStatus::Todo,
        };

        let priority = match priority_name.to_lowercase().as_str() {
            s if s.contains("critical") || s.contains("urgent") => TicketPriority::Critical,
            s if s.contains("high") => TicketPriority::High,
            s if s.contains("medium") => TicketPriority::Medium,
            s if s.contains("low") => TicketPriority::Low,
            _ => TicketPriority::None,
        };

        // Use a short ID for the key.
        let short_id = if id.len() > 8 { &id[..8] } else { &id };
        let key = format!("NOTION-{}", short_id);

        Some(Ticket {
            key,
            title,
            description: String::new(),
            status,
            priority,
            provider: "notion".to_string(),
            url,
            assignee: None,
            labels: Vec::new(),
        })
    }

    fn extract_title_from_properties(&self, json: &str) -> Option<String> {
        // Look for the title property name, then find "plain_text" inside.
        let prop_pattern = format!("\"{}\"", self.title_prop());
        let pos = json.find(&prop_pattern)?;
        let after = &json[pos..];
        // Find "plain_text" in the title property object.
        let pt_pos = after.find("\"plain_text\"")?;
        // Don't search too far — stay within reason.
        if pt_pos > 500 { return None; }
        let pt_after = &after[pt_pos..];
        let colon = pt_after.find(':')?;
        let value_start = pt_after[colon + 1..].trim_start();
        if value_start.starts_with('"') {
            let end = value_start[1..].find('"')?;
            Some(value_start[1..1 + end].to_string())
        } else {
            None
        }
    }

    fn extract_select_property(&self, json: &str, prop_name: &str) -> Option<String> {
        let pattern = format!("\"{}\"", prop_name);
        let pos = json.find(&pattern)?;
        let after = &json[pos..];
        // Find "name" within the select object (within a reasonable range).
        let name_pos = after[pattern.len()..].find("\"name\"")?;
        if name_pos > 300 { return None; }
        let name_after = &after[pattern.len() + name_pos..];
        let colon = name_after.find(':')?;
        let value_start = name_after[colon + 1..].trim_start();
        if value_start.starts_with('"') {
            let end = value_start[1..].find('"')?;
            Some(value_start[1..1 + end].to_string())
        } else {
            None
        }
    }
}

impl TicketProvider for NotionProvider {
    fn name(&self) -> &str {
        "notion"
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
