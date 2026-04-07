//! MCP (Model Context Protocol) client for discovering and fetching tickets
//! from MCP servers configured on the user's machine.
//!
//! Supports stdio-based MCP servers (spawned as child processes).
//! Discovers servers from ~/.cmdctl/mcp.json, falling back to ~/.claude/settings.json.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::provider::{Ticket, TicketPriority, TicketStatus};

/// Info about a discovered MCP server.
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    /// Server name from config (e.g., "atlassian", "notion").
    pub name: String,
    /// Command to spawn the server.
    pub command: String,
    /// Arguments for the command.
    pub args: Vec<String>,
    /// Environment variables to set.
    pub env: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Discover MCP servers from config files.
/// Checks config files in priority order:
/// 1. ~/.cmdctl/mcp.json (CMD-CTL's own config)
/// 2. ~/.claude/.mcp.json (Claude Code MCP config)
/// 3. ~/.claude/settings.json (Claude Code settings, may contain mcpServers)
pub fn discover_mcp_servers() -> Vec<McpServerInfo> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let candidates = [
        home.join(".cmdctl").join("mcp.json"),
        home.join(".claude").join(".mcp.json"),
        home.join(".claude").join("settings.json"),
    ];

    for path in &candidates {
        if let Some(servers) = parse_mcp_config(path) {
            if !servers.is_empty() {
                tracing::info!("Discovered {} MCP server(s) from {}", servers.len(), path.display());
                return servers;
            }
        }
    }

    tracing::debug!("No MCP servers found in {:?}", candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
    Vec::new()
}

/// Parse an MCP config file and extract stdio-based server entries.
fn parse_mcp_config(path: &PathBuf) -> Option<Vec<McpServerInfo>> {
    let content = std::fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(&content).ok()?;

    let servers_obj = json.get("mcpServers")?.as_object()?;
    let mut servers = Vec::new();

    for (name, entry) in servers_obj {
        // Only support stdio-based servers (have "command", no "url").
        let command = match entry.get("command").and_then(|v| v.as_str()) {
            Some(cmd) => cmd.to_string(),
            None => continue, // Skip URL-based servers.
        };
        if entry.get("url").is_some() {
            continue;
        }

        let args: Vec<String> = entry
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let env: HashMap<String, String> = entry
            .get("env")
            .and_then(|v| v.as_object())
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        servers.push(McpServerInfo {
            name: name.clone(),
            command,
            args,
            env,
        });
    }

    Some(servers)
}

// ---------------------------------------------------------------------------
// MCP Client — JSON-RPC over stdio
// ---------------------------------------------------------------------------

/// Monotonically increasing request ID for JSON-RPC messages.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A running MCP server process with buffered I/O.
struct McpProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
}

impl McpProcess {
    fn spawn(server: &McpServerInfo) -> Result<Self> {
        let mut cmd = Command::new(&server.command);
        cmd.args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (k, v) in &server.env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server '{}': {} {:?}", server.name, server.command, server.args))?;

        let stdin = child.stdin.take().context("Failed to capture MCP server stdin")?;
        let stdout = child.stdout.take().context("Failed to capture MCP server stdout")?;
        let reader = BufReader::new(stdout);

        Ok(Self { child, stdin, reader })
    }

    /// Send a JSON-RPC request and return the response.
    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = next_id();
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;

        // Read lines until we get a response matching our ID.
        // Skip notifications (no "id" field).
        loop {
            let mut buf = String::new();
            let bytes_read = self.reader.read_line(&mut buf)?;
            if bytes_read == 0 {
                anyhow::bail!("MCP server closed stdout before responding to '{}'", method);
            }
            let buf = buf.trim();
            if buf.is_empty() {
                continue;
            }

            let resp: Value = match serde_json::from_str(buf) {
                Ok(v) => v,
                Err(_) => continue, // Skip non-JSON lines (e.g., logging).
            };

            // Skip notifications (no "id" field).
            if resp.get("id").is_none() {
                continue;
            }

            // Check if this is our response.
            let resp_id = resp.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            if resp_id != id {
                continue; // Not our response, skip.
            }

            // Check for errors.
            if let Some(err) = resp.get("error") {
                let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
                anyhow::bail!("MCP server error for '{}': {}", method, msg);
            }

            return Ok(resp.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&msg)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;
        Ok(())
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

// ---------------------------------------------------------------------------
// Ticket fetching via MCP
// ---------------------------------------------------------------------------

/// Spawn an MCP server, discover its tools, call a ticket-listing tool,
/// and return the results as Tickets.
pub fn fetch_tickets_via_mcp(server: &McpServerInfo) -> Result<Vec<Ticket>> {
    tracing::info!("Connecting to MCP server '{}'...", server.name);

    let mut proc = McpProcess::spawn(server)?;

    // 1. Initialize handshake.
    let init_result = proc.request("initialize", serde_json::json!({
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "cmdctl",
            "version": "0.1.0"
        }
    }))?;

    tracing::debug!("MCP '{}' initialized: {:?}",
        server.name,
        init_result.get("serverInfo").and_then(|v| v.get("name")).and_then(|v| v.as_str())
    );

    // 2. Send initialized notification.
    proc.notify("notifications/initialized", serde_json::json!({}))?;

    // 3. List available tools.
    let tools_result = proc.request("tools/list", serde_json::json!({}))?;
    let tools = tools_result
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if tools.is_empty() {
        tracing::warn!("MCP server '{}' has no tools", server.name);
        return Ok(Vec::new());
    }

    tracing::debug!("MCP '{}' has {} tool(s): {:?}",
        server.name,
        tools.len(),
        tools.iter().filter_map(|t| t.get("name").and_then(|v| v.as_str())).collect::<Vec<_>>()
    );

    // 4. Find a ticket-listing tool.
    let tool = find_ticket_tool(&tools, &server.name);
    let tool_name = match tool {
        Some(name) => name,
        None => {
            tracing::warn!("No ticket-listing tool found on MCP server '{}'", server.name);
            return Ok(Vec::new());
        }
    };

    tracing::info!("Calling tool '{}' on MCP server '{}'", tool_name, server.name);

    // 5. Call the tool.
    let call_result = proc.request("tools/call", serde_json::json!({
        "name": tool_name,
        "arguments": {}
    }))?;

    // 6. Parse the result into tickets.
    let tickets = parse_tool_result(&call_result, &server.name);

    tracing::info!("Got {} ticket(s) from MCP server '{}'", tickets.len(), server.name);

    Ok(tickets)
}

/// Heuristic to find a tool that lists tickets/issues/tasks.
fn find_ticket_tool(tools: &[Value], server_name: &str) -> Option<String> {
    // Keywords that suggest a listing operation.
    let list_keywords = ["list", "search", "query", "get_all", "fetch"];
    // Keywords that suggest ticket/issue content.
    let item_keywords = ["issue", "ticket", "task", "page", "card", "item", "bug", "story", "epic"];

    // First pass: find tools matching both a list keyword AND an item keyword.
    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let desc = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let combined = format!("{} {}", name.to_lowercase(), desc.to_lowercase());

        let has_list = list_keywords.iter().any(|kw| combined.contains(kw));
        let has_item = item_keywords.iter().any(|kw| combined.contains(kw));

        if has_list && has_item {
            return Some(name.to_string());
        }
    }

    // Second pass: match by server name context.
    // E.g., for "atlassian" server, look for tools with "issue" or "jira" in the name.
    let server_lower = server_name.to_lowercase();
    let server_hints: &[&str] = match server_lower.as_str() {
        s if s.contains("atlassian") || s.contains("jira") => &["issue", "jira", "search"],
        s if s.contains("notion") => &["page", "database", "query"],
        s if s.contains("imperrium") => &["page", "task", "list"],
        s if s.contains("linear") => &["issue", "list"],
        s if s.contains("github") => &["issue", "list"],
        _ => &[],
    };

    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        if server_hints.iter().any(|hint| name.contains(hint)) {
            let has_list = list_keywords.iter().any(|kw| name.contains(kw));
            if has_list {
                return Some(tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());
            }
        }
    }

    // Last resort: pick the first tool that has any list keyword.
    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        if list_keywords.iter().any(|kw| name.contains(kw)) {
            return Some(tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());
        }
    }

    None
}

/// Parse the result from a tools/call response into Ticket structs.
fn parse_tool_result(result: &Value, provider_name: &str) -> Vec<Ticket> {
    let provider = format!("mcp:{}", provider_name);
    let mut tickets = Vec::new();

    // MCP tool results come as content blocks.
    let content = match result.get("content").and_then(|v| v.as_array()) {
        Some(c) => c,
        None => return tickets,
    };

    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if block_type != "text" {
            continue;
        }
        let text = match block.get("text").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => continue,
        };

        // Try to parse the text as JSON (array of objects or single object).
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            let items = if parsed.is_array() {
                parsed.as_array().cloned().unwrap_or_default()
            } else if parsed.is_object() {
                // Some tools wrap results: { "issues": [...] } or { "results": [...] }
                extract_array_from_object(&parsed)
            } else {
                continue;
            };

            for item in &items {
                if let Some(ticket) = value_to_ticket(item, &provider) {
                    tickets.push(ticket);
                }
            }
        }
    }

    // Also check structuredContent if present.
    if tickets.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            let items = if structured.is_array() {
                structured.as_array().cloned().unwrap_or_default()
            } else if structured.is_object() {
                extract_array_from_object(structured)
            } else {
                Vec::new()
            };
            for item in &items {
                if let Some(ticket) = value_to_ticket(item, &provider) {
                    tickets.push(ticket);
                }
            }
        }
    }

    tickets
}

/// Extract the first array value from an object (e.g., { "issues": [...] }).
fn extract_array_from_object(obj: &Value) -> Vec<Value> {
    if let Some(map) = obj.as_object() {
        for value in map.values() {
            if let Some(arr) = value.as_array() {
                return arr.clone();
            }
        }
    }
    vec![obj.clone()]
}

/// Best-effort conversion of a JSON object to a Ticket.
/// Handles various field naming conventions from different MCP servers.
fn value_to_ticket(v: &Value, provider: &str) -> Option<Ticket> {
    let obj = v.as_object()?;

    // Key: try "key", "id", "number", "issue_key".
    let key = obj.get("key").or(obj.get("id")).or(obj.get("number")).or(obj.get("issue_key"))
        .and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })?;

    // Title: try "title", "summary", "name", "subject".
    let title = obj.get("title").or(obj.get("summary")).or(obj.get("name")).or(obj.get("subject"))
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled")
        .to_string();

    // Description.
    let description = obj.get("description").or(obj.get("body")).or(obj.get("content"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Status.
    let status_str = obj.get("status")
        .and_then(|v| {
            // Handle both string and nested { "name": "..." } formats.
            v.as_str().map(String::from)
                .or_else(|| v.get("name").and_then(|n| n.as_str()).map(String::from))
        })
        .unwrap_or_default();
    let status = parse_status(&status_str);

    // Priority.
    let priority_str = obj.get("priority")
        .and_then(|v| {
            v.as_str().map(String::from)
                .or_else(|| v.get("name").and_then(|n| n.as_str()).map(String::from))
        })
        .unwrap_or_default();
    let priority = parse_priority(&priority_str);

    // URL.
    let url = obj.get("url").or(obj.get("self")).or(obj.get("html_url")).or(obj.get("web_url"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Assignee.
    let assignee = obj.get("assignee")
        .and_then(|v| {
            v.as_str().map(String::from)
                .or_else(|| v.get("displayName").and_then(|n| n.as_str()).map(String::from))
                .or_else(|| v.get("name").and_then(|n| n.as_str()).map(String::from))
        });

    // Labels.
    let labels = obj.get("labels")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str().map(String::from)
                        .or_else(|| v.get("name").and_then(|n| n.as_str()).map(String::from))
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Ticket {
        key,
        title,
        description,
        status,
        priority,
        provider: provider.to_string(),
        url,
        assignee,
        labels,
    })
}

fn parse_status(s: &str) -> TicketStatus {
    match s.to_lowercase().as_str() {
        "todo" | "to do" | "open" | "new" | "backlog" | "not started" => TicketStatus::Todo,
        "in progress" | "in_progress" | "active" | "doing" | "started" => TicketStatus::InProgress,
        "in review" | "in_review" | "review" | "pending review" => TicketStatus::InReview,
        "done" | "closed" | "complete" | "completed" | "resolved" => TicketStatus::Done,
        "blocked" | "on hold" | "hold" | "waiting" => TicketStatus::Blocked,
        "" => TicketStatus::Todo,
        other => TicketStatus::Custom(other.to_string()),
    }
}

fn parse_priority(s: &str) -> TicketPriority {
    match s.to_lowercase().as_str() {
        "critical" | "blocker" | "highest" | "urgent" | "p0" => TicketPriority::Critical,
        "high" | "p1" | "important" => TicketPriority::High,
        "medium" | "normal" | "p2" | "default" => TicketPriority::Medium,
        "low" | "minor" | "p3" => TicketPriority::Low,
        "none" | "trivial" | "p4" | "" => TicketPriority::None,
        _ => TicketPriority::Medium,
    }
}
