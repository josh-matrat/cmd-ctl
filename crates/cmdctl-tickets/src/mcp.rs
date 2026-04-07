//! MCP (Model Context Protocol) client for discovering and fetching tickets
//! from MCP servers configured on the user's machine.
//!
//! Supports both stdio-based MCP servers (spawned as child processes) and
//! HTTP/HTTPS URL-based servers (Streamable HTTP transport).
//! Discovers servers from ~/.cmdctl/mcp.json, falling back to ~/.claude/settings.json.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::provider::{Ticket, TicketPriority, TicketStatus};

/// Transport configuration for an MCP server.
#[derive(Debug, Clone)]
pub enum McpTransport {
    /// Stdio-based server spawned as a child process.
    Stdio {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    /// HTTP/HTTPS URL-based server (Streamable HTTP transport).
    Http {
        url: String,
        headers: HashMap<String, String>,
    },
}

/// Per-server query configuration for ticket fetching.
#[derive(Debug, Clone, Default)]
pub struct McpQueryConfig {
    /// Board / project key to filter by (e.g., "PROJ", "MYBOARD").
    pub board: Option<String>,
    /// User identifier or email to filter assigned tickets.
    pub assignee: Option<String>,
    /// Explicit tool arguments to pass (overrides auto-built args).
    pub tool_args: HashMap<String, Value>,
}

/// Info about a discovered MCP server.
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    /// Server name from config (e.g., "atlassian", "notion").
    pub name: String,
    /// Transport configuration.
    pub transport: McpTransport,
    /// Query configuration for ticket fetching.
    pub query: McpQueryConfig,
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

/// Parse an MCP config file and extract server entries (both stdio and HTTP).
fn parse_mcp_config(path: &PathBuf) -> Option<Vec<McpServerInfo>> {
    let content = std::fs::read_to_string(path).ok()?;
    let json: Value = serde_json::from_str(&content).ok()?;

    let servers_obj = json.get("mcpServers")?.as_object()?;
    let mut servers = Vec::new();

    for (name, entry) in servers_obj {
        let transport = if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
            // HTTP/HTTPS URL-based server.
            let headers: HashMap<String, String> = entry
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            McpTransport::Http {
                url: url.to_string(),
                headers,
            }
        } else if let Some(command) = entry.get("command").and_then(|v| v.as_str()) {
            // Stdio-based server.
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

            McpTransport::Stdio { command: command.to_string(), args, env }
        } else {
            tracing::debug!("Skipping MCP server '{}': no 'url' or 'command' field", name);
            continue;
        };

        // Parse query config: board, assignee, toolArgs.
        let board = entry.get("board").and_then(|v| v.as_str()).map(String::from);
        let assignee = entry.get("assignee").and_then(|v| v.as_str()).map(String::from);
        let tool_args: HashMap<String, Value> = entry
            .get("toolArgs")
            .and_then(|v| v.as_object())
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        servers.push(McpServerInfo {
            name: name.clone(),
            transport,
            query: McpQueryConfig { board, assignee, tool_args },
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

// ---------------------------------------------------------------------------
// Transport trait — unifies stdio and HTTP MCP communication
// ---------------------------------------------------------------------------

/// Trait for MCP JSON-RPC communication over any transport.
trait McpClient {
    /// Send a JSON-RPC request and return the result.
    fn request(&mut self, method: &str, params: Value) -> Result<Value>;
    /// Send a JSON-RPC notification (no response expected).
    fn notify(&mut self, method: &str, params: Value) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Stdio transport
// ---------------------------------------------------------------------------

/// A running MCP server process with buffered I/O.
struct McpProcess {
    child: Child,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
}

impl McpProcess {
    fn spawn(name: &str, command: &str, args: &[String], env: &HashMap<String, String>) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server '{}': {} {:?}", name, command, args))?;

        let stdin = child.stdin.take().context("Failed to capture MCP server stdin")?;
        let stdout = child.stdout.take().context("Failed to capture MCP server stdout")?;
        let reader = BufReader::new(stdout);

        Ok(Self { child, stdin, reader })
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl McpClient for McpProcess {
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
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

// ---------------------------------------------------------------------------
// HTTP transport (curl-based, uses system native TLS)
// ---------------------------------------------------------------------------

/// MCP client that communicates over HTTP/HTTPS via curl.
/// Uses the same curl-based pattern as the Jira/Notion/Imperrium providers
/// to ensure native TLS compatibility.
struct McpHttpClient {
    url: String,
    headers: HashMap<String, String>,
    /// Session ID returned by the server, sent in subsequent requests.
    session_id: Option<String>,
    /// Temp file for curl cookie jar — persists auth cookies across requests.
    cookie_jar: std::path::PathBuf,
}

impl McpHttpClient {
    fn new(url: &str, headers: &HashMap<String, String>, server_name: &str) -> Self {
        // Stable cookie jar per server so OAuth tokens persist across syncs.
        let cookie_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join(".cmdctl")
            .join("mcp-cookies");
        let _ = std::fs::create_dir_all(&cookie_dir);
        // Sanitize server name for use as filename.
        let safe_name: String = server_name.chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let cookie_jar = cookie_dir.join(format!("{}.txt", safe_name));

        Self {
            url: url.to_string(),
            headers: headers.clone(),
            session_id: None,
            cookie_jar,
        }
    }

    /// POST a JSON body via curl, returning (response_headers, response_body).
    fn post(&mut self, json_body: &str, method: &str, timeout_secs: u32) -> Result<String> {
        let cookie_path = self.cookie_jar.to_string_lossy().to_string();
        let mut args: Vec<String> = vec![
            "-sS".into(),
            "-X".into(), "POST".into(),
            "--max-time".into(), timeout_secs.to_string(),
            "-H".into(), "Content-Type: application/json".into(),
            "-H".into(), "Accept: application/json, text/event-stream".into(),
            // Persist cookies across requests (OAuth tokens, session cookies).
            "-b".into(), cookie_path.clone(),
            "-c".into(), cookie_path,
            // Include response headers in output (separated by blank line).
            "-i".into(),
        ];

        for (k, v) in &self.headers {
            args.push("-H".into());
            args.push(format!("{}: {}", k, v));
        }

        if let Some(ref sid) = self.session_id {
            args.push("-H".into());
            args.push(format!("Mcp-Session-Id: {}", sid));
        }

        // Pass body via stdin to avoid shell escaping issues.
        args.push("-d".into());
        args.push("@-".into());
        args.push(self.url.clone());

        let mut child = Command::new("curl")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn curl for MCP method '{}'", method))?;

        // Write JSON body to stdin.
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(json_body.as_bytes())
                .with_context(|| format!("Failed to write body for MCP method '{}'", method))?;
        }

        let output = child.wait_with_output()
            .with_context(|| format!("curl failed for MCP method '{}'", method))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("curl error for '{}' (url: {}): {}", method, self.url, stderr.trim());
        }

        let raw = String::from_utf8_lossy(&output.stdout);

        // curl -i separates headers from body with a blank line (\r\n\r\n).
        let (header_section, body) = Self::split_headers_body(&raw);

        // Capture Mcp-Session-Id from response headers BEFORE checking status,
        // so auth error responses still establish a session for retry.
        for line in header_section.lines() {
            let lower = line.to_lowercase();
            if lower.starts_with("mcp-session-id:") {
                if let Some(val) = line.split_once(':').map(|(_, v)| v.trim()) {
                    self.session_id = Some(val.to_string());
                }
            }
        }

        // Check HTTP status from the first header line.
        if let Some(status_line) = header_section.lines().next() {
            if let Some(code_str) = status_line.split_whitespace().nth(1) {
                if let Ok(code) = code_str.parse::<u16>() {
                    if code >= 400 {
                        anyhow::bail!(
                            "MCP server returned HTTP {} for '{}' (url: {}): {}",
                            code, method, self.url, body.trim().chars().take(300).collect::<String>()
                        );
                    }
                }
            }
        }

        // Check if this is an SSE response (Content-Type: text/event-stream).
        let is_sse = header_section.lines().any(|l| {
            let lower = l.to_lowercase();
            lower.starts_with("content-type:") && lower.contains("text/event-stream")
        });

        if is_sse {
            Self::extract_json_from_sse(&body)
                .with_context(|| format!("Failed to parse SSE response for '{}'", method))
        } else {
            Ok(body.to_string())
        }
    }

    /// Split curl -i output into (headers, body) at the first blank line.
    /// Handles HTTP/1.1 100 Continue responses by skipping them.
    fn split_headers_body(raw: &str) -> (&str, &str) {
        // Find the blank line separating headers from body.
        // Handle both \r\n\r\n and \n\n.
        let split_pos = raw.find("\r\n\r\n")
            .map(|p| (p, p + 4))
            .or_else(|| raw.find("\n\n").map(|p| (p, p + 2)));

        match split_pos {
            Some((header_end, body_start)) => {
                let headers = &raw[..header_end];
                let body = &raw[body_start..];
                // If the first response is HTTP 100 Continue, skip to the next header block.
                if headers.contains("100 Continue") || headers.contains("100 continue") {
                    return Self::split_headers_body(body);
                }
                (headers, body)
            }
            None => ("", raw),
        }
    }

    /// Extract the first JSON-RPC response from SSE-formatted text.
    fn extract_json_from_sse(body: &str) -> Result<String> {
        let mut current_data = String::new();

        for line in body.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                current_data.push_str(data);
            } else if line.trim().is_empty() && !current_data.is_empty() {
                // End of SSE event — check if this is a JSON-RPC response.
                if let Ok(parsed) = serde_json::from_str::<Value>(&current_data) {
                    if parsed.get("id").is_some() || parsed.get("result").is_some() || parsed.get("error").is_some() {
                        return Ok(current_data);
                    }
                }
                current_data.clear();
            }
        }

        // Return whatever we have if there's pending data.
        if !current_data.is_empty() {
            return Ok(current_data);
        }

        anyhow::bail!("No JSON-RPC response found in SSE stream")
    }

    /// Parse a JSON-RPC response, extracting the result or error.
    fn parse_response(body: &str, method: &str) -> Result<Value> {
        let resp: Value = serde_json::from_str(body)
            .with_context(|| format!("Invalid JSON from MCP server for '{}': {}",
                method, &body[..body.len().min(200)]))?;

        if let Some(err) = resp.get("error") {
            let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
            let msg = err.get("message").and_then(|v| v.as_str()).unwrap_or("Unknown error");
            anyhow::bail!("MCP server error for '{}': {} (code {})", method, msg, code);
        }

        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl McpClient for McpHttpClient {
    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = next_id();
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        // Allow extra time for initialize (may trigger browser auth flow).
        let timeout = if method == "initialize" { 120 } else { 30 };

        let body = serde_json::to_string(&msg)?;
        let response_body = self.post(&body, method, timeout)
            .with_context(|| format!("Request failed for MCP method '{}'", method))?;

        Self::parse_response(&response_body, method)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let body = serde_json::to_string(&msg)?;
        // Notifications may return 202/204 — ignore errors.
        let _ = self.post(&body, method, 10);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Ticket fetching via MCP
// ---------------------------------------------------------------------------

/// Connect to an MCP server (stdio or HTTP), discover its tools, call a
/// ticket-listing tool, and return the results as Tickets.
pub fn fetch_tickets_via_mcp(server: &McpServerInfo) -> Result<Vec<Ticket>> {
    tracing::info!("Connecting to MCP server '{}'...", server.name);

    let mut client: Box<dyn McpClient> = match &server.transport {
        McpTransport::Stdio { command, args, env } => {
            Box::new(McpProcess::spawn(&server.name, command, args, env)?)
        }
        McpTransport::Http { url, headers } => {
            Box::new(McpHttpClient::new(url, headers, &server.name))
        }
    };

    // 1. Initialize handshake.
    let init_result = client.request("initialize", serde_json::json!({
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
    client.notify("notifications/initialized", serde_json::json!({}))?;

    // 3. List available tools.
    let tools_result = client.request("tools/list", serde_json::json!({}))?;
    let tools = tools_result
        .get("tools")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if tools.is_empty() {
        anyhow::bail!("MCP server '{}' returned no tools — check server configuration and auth", server.name);
    }

    let tool_names: Vec<&str> = tools.iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect();
    tracing::debug!("MCP '{}' has {} tool(s): {:?}", server.name, tools.len(), tool_names);

    // 4. Find a ticket-listing tool (returns name + full tool schema).
    let (tool_name, tool_schema) = match find_ticket_tool(&tools, &server.name) {
        Some(pair) => pair,
        None => {
            anyhow::bail!(
                "No ticket-listing tool found on MCP server '{}'. Available tools: {:?}",
                server.name, tool_names
            );
        }
    };

    // 5. Build arguments from the tool's inputSchema and server query config.
    let arguments = build_tool_args(&tool_name, &tool_schema, &server.query, &server.name);
    tracing::info!("Calling tool '{}' on MCP server '{}' with args: {}",
        tool_name, server.name, serde_json::to_string(&arguments).unwrap_or_default());

    // 6. Call the tool.
    let call_result = client.request("tools/call", serde_json::json!({
        "name": tool_name,
        "arguments": arguments,
    }))?;

    // 7. Parse the result into tickets.
    let tickets = parse_tool_result(&call_result, &server.name);

    tracing::info!("Got {} ticket(s) from MCP server '{}'", tickets.len(), server.name);

    Ok(tickets)
}

/// Heuristic to find a tool that lists tickets/issues/tasks.
/// Returns the tool name and its full schema (for inspecting inputSchema).
fn find_ticket_tool(tools: &[Value], server_name: &str) -> Option<(String, Value)> {
    // Keywords that suggest a listing operation.
    let list_keywords = ["list", "search", "query", "get_all", "fetch"];
    // Keywords that suggest ticket/issue content.
    let item_keywords = ["issue", "ticket", "task", "page", "card", "item", "bug", "story", "epic"];

    let extract = |tool: &Value| -> Option<(String, Value)> {
        let name = tool.get("name").and_then(|v| v.as_str())?;
        Some((name.to_string(), tool.clone()))
    };

    // First pass: find tools matching both a list keyword AND an item keyword.
    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let desc = tool.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let combined = format!("{} {}", name.to_lowercase(), desc.to_lowercase());

        let has_list = list_keywords.iter().any(|kw| combined.contains(kw));
        let has_item = item_keywords.iter().any(|kw| combined.contains(kw));

        if has_list && has_item {
            return extract(tool);
        }
    }

    // Second pass: match by server name context.
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
                return extract(tool);
            }
        }
    }

    // Last resort: pick the first tool that has any list keyword.
    for tool in tools {
        let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        if list_keywords.iter().any(|kw| name.contains(kw)) {
            return extract(tool);
        }
    }

    None
}

/// Build tool arguments from the tool's inputSchema and per-server query config.
///
/// Strategy:
/// 1. If the user supplied explicit `toolArgs`, use those directly.
/// 2. Otherwise, inspect the tool's inputSchema properties and build args
///    from the `board` and `assignee` config:
///    - `jql` / `query` param → build a JQL string from board + assignee
///    - `boardId` / `board` / `projectKey` param → pass the board value
///    - `assignee` / `accountId` / `user` param → pass the assignee value
///    - `maxResults` param → default to 50
fn build_tool_args(
    tool_name: &str,
    tool_schema: &Value,
    query: &McpQueryConfig,
    server_name: &str,
) -> Value {
    // If explicit toolArgs are configured, use them as-is.
    if !query.tool_args.is_empty() {
        return Value::Object(
            query.tool_args.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        );
    }

    let props = tool_schema
        .get("inputSchema")
        .and_then(|s| s.get("properties"))
        .and_then(|p| p.as_object());

    let prop_names: Vec<&str> = props
        .map(|p| p.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    let mut args = serde_json::Map::new();

    // Check if tool accepts a JQL / query string parameter.
    let jql_param = prop_names.iter().find(|p| {
        matches!(p.to_lowercase().as_str(), "jql" | "query" | "search_query" | "filter")
    });

    if let Some(&param_name) = jql_param {
        // Build JQL from board + assignee.
        let jql = build_jql(query.board.as_deref(), query.assignee.as_deref(), server_name);
        args.insert(param_name.to_string(), Value::String(jql));
    } else {
        // No JQL param — try to pass board/assignee as separate params.
        if let Some(board) = &query.board {
            if let Some(&param) = prop_names.iter().find(|p| {
                matches!(p.to_lowercase().as_str(),
                    "boardid" | "board" | "board_id" | "projectkey" | "project_key" | "project")
            }) {
                args.insert(param.to_string(), Value::String(board.clone()));
            }
        }

        if let Some(assignee) = &query.assignee {
            if let Some(&param) = prop_names.iter().find(|p| {
                matches!(p.to_lowercase().as_str(),
                    "assignee" | "accountid" | "account_id" | "user" | "userid" | "user_id")
            }) {
                args.insert(param.to_string(), Value::String(assignee.clone()));
            }
        }
    }

    // Add maxResults if the tool accepts it and we haven't set it.
    if let Some(&param) = prop_names.iter().find(|p| {
        matches!(p.to_lowercase().as_str(), "maxresults" | "max_results" | "limit")
    }) {
        if !args.contains_key(param) {
            args.insert(param.to_string(), Value::Number(50.into()));
        }
    }

    // If we still have no args but the tool has required params, log a warning.
    if args.is_empty() {
        let required = tool_schema
            .get("inputSchema")
            .and_then(|s| s.get("required"))
            .and_then(|r| r.as_array());
        if let Some(req) = required {
            if !req.is_empty() {
                tracing::warn!(
                    "MCP tool '{}' on '{}' has required params {:?} but no query config (board/assignee) to fill them. \
                     Add \"board\" and/or \"assignee\" to the MCP server config.",
                    tool_name, server_name, req
                );
            }
        }
    }

    Value::Object(args)
}

/// Build a JQL query string from optional board and assignee filters.
fn build_jql(board: Option<&str>, assignee: Option<&str>, server_name: &str) -> String {
    let mut clauses = Vec::new();

    if let Some(board) = board {
        clauses.push(format!("project = \"{}\"", board));
    }

    if let Some(assignee) = assignee {
        // Support "currentUser()" as a special JQL function.
        if assignee.contains("()") || assignee.contains("(") {
            clauses.push(format!("assignee = {}", assignee));
        } else {
            clauses.push(format!("assignee = \"{}\"", assignee));
        }
    }

    clauses.push("status != Done".to_string());

    let jql = format!("{} ORDER BY priority DESC, updated DESC", clauses.join(" AND "));
    tracing::debug!("Built JQL for '{}': {}", server_name, jql);
    jql
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
