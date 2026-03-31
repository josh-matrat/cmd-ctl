//! Configuration for ticket providers, loaded from ~/.cmdctl/providers.toml.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Top-level config file structure.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub jira: Option<JiraConfig>,
    #[serde(default)]
    pub notion: Option<NotionConfig>,
    #[serde(default)]
    pub imperrium: Option<ImperriumConfig>,
    /// Generic extra providers for future extensibility.
    #[serde(flatten)]
    pub extra: HashMap<String, toml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraConfig {
    /// Jira instance URL (e.g., "https://mycompany.atlassian.net").
    pub url: String,
    /// User email for authentication.
    pub email: String,
    /// API token (Atlassian API token).
    pub api_token: String,
    /// JQL query to fetch tickets. Defaults to assigned open tickets.
    pub jql: Option<String>,
    /// Max results to fetch.
    pub max_results: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotionConfig {
    /// Notion integration token.
    pub api_token: String,
    /// Database ID containing tickets.
    pub database_id: String,
    /// Property name for ticket title. Defaults to "Name".
    pub title_property: Option<String>,
    /// Property name for status. Defaults to "Status".
    pub status_property: Option<String>,
    /// Property name for priority. Defaults to "Priority".
    pub priority_property: Option<String>,
    /// Filter to only show tickets assigned to this person.
    pub assignee: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImperriumConfig {
    /// Imperrium API endpoint.
    pub url: String,
    /// API key or bearer token.
    pub api_token: String,
    /// Project or workspace identifier.
    pub project: Option<String>,
    /// User identifier for filtering assigned tickets.
    pub user: Option<String>,
}

/// Default config file path.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cmdctl")
        .join("providers.toml")
}

/// Load config from disk. Returns default (empty) config if file doesn't exist.
pub fn load_config() -> Result<ProvidersConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(ProvidersConfig::default());
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let config: ProvidersConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(config)
}

/// Write a default example config file if none exists.
pub fn write_example_config() -> Result<PathBuf> {
    let path = config_path();
    if path.exists() {
        return Ok(path);
    }
    let example = r#"# CMD CTL Ticket Provider Configuration
# Uncomment and configure the providers you use.

# [jira]
# url = "https://yourcompany.atlassian.net"
# email = "you@company.com"
# api_token = "your-api-token"
# jql = "assignee = currentUser() AND status != Done ORDER BY priority DESC"
# max_results = 50

# [notion]
# api_token = "secret_..."
# database_id = "your-database-id"
# title_property = "Name"
# status_property = "Status"
# priority_property = "Priority"

# [imperrium]
# url = "https://api.imperrium.example.com"
# api_token = "your-token"
# project = "my-project"
# user = "your-user-id"
"#;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, example)?;
    Ok(path)
}
