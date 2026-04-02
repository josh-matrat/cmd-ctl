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

#[derive(Clone, Serialize, Deserialize)]
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
    /// Project key for creating new tickets (e.g., "PROJ"). Required for ticket creation.
    pub project_key: Option<String>,
    /// Default issue type for new tickets. Defaults to "Task".
    pub issue_type: Option<String>,
}

impl std::fmt::Debug for JiraConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraConfig")
            .field("url", &self.url)
            .field("email", &self.email)
            .field("api_token", &"***")
            .field("jql", &self.jql)
            .field("max_results", &self.max_results)
            .field("project_key", &self.project_key)
            .field("issue_type", &self.issue_type)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
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
    /// Filter to only show tickets assigned to this person (Notion user ID).
    pub assignee: Option<String>,
    /// User email — resolved to a Notion user ID for assignee filtering.
    pub user_email: Option<String>,
}

impl std::fmt::Debug for NotionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotionConfig")
            .field("api_token", &"***")
            .field("database_id", &self.database_id)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Serialize, Deserialize)]
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

impl std::fmt::Debug for ImperriumConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImperriumConfig")
            .field("url", &self.url)
            .field("api_token", &"***")
            .field("project", &self.project)
            .field("user", &self.user)
            .finish()
    }
}

/// Default config file path.
pub fn config_path() -> PathBuf {
    dirs::home_dir()
        .expect("HOME directory not set — cannot determine config path")
        .join(".cmdctl")
        .join("providers.toml")
}

/// Load config from disk. Returns default (empty) config if file doesn't exist.
pub fn load_config() -> Result<ProvidersConfig> {
    let path = config_path();
    if !path.exists() {
        return Ok(ProvidersConfig::default());
    }
    // Warn if the config file (which may contain API tokens) is readable by others.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mode = meta.permissions().mode();
            if mode & 0o077 != 0 {
                tracing::warn!(
                    "providers.toml has overly permissive mode {:o} — should be 0600. \
                     Run: chmod 600 {}",
                    mode & 0o777,
                    path.display()
                );
            }
        }
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let config: ProvidersConfig = toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

/// Validate security-sensitive config values.
fn validate_config(config: &ProvidersConfig) -> Result<()> {
    if let Some(ref jira) = config.jira {
        require_https(&jira.url, "Jira")?;
    }
    if let Some(ref imperrium) = config.imperrium {
        require_https(&imperrium.url, "Imperrium")?;
    }
    if let Some(ref notion) = config.notion {
        if notion.database_id.contains('/') || notion.database_id.contains("..") {
            anyhow::bail!("Notion database_id contains invalid characters");
        }
    }
    Ok(())
}

fn require_https(url: &str, provider: &str) -> Result<()> {
    if !url.is_empty() && !url.starts_with("https://") {
        anyhow::bail!(
            "{} URL must use HTTPS to protect credentials (got: {})",
            provider,
            url
        );
    }
    Ok(())
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
# project_key = "PROJ"
# issue_type = "Task"

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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    std::fs::write(&path, example)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}
