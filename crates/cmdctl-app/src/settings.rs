//! Application settings — load, save, and build UI field list.
//!
//! General settings are stored at `~/.cmdctl/settings.toml`.
//! Ticket provider settings are stored at `~/.cmdctl/providers.toml`.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config structs — settings.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default)]
    pub general: GeneralSettings,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeneralSettings {
    #[serde(default = "default_font_name")]
    pub font_name: String,
    #[serde(default = "default_font_size")]
    pub font_size: f64,
    #[serde(default = "default_shell")]
    pub default_shell: String,
    #[serde(default)]
    pub claude_dangerously_skip_permissions: bool,
}

fn default_font_name() -> String { "Menlo".to_string() }
fn default_font_size() -> f64 { 13.0 }
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            font_name: default_font_name(),
            font_size: default_font_size(),
            default_shell: default_shell(),
            claude_dangerously_skip_permissions: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Config structs — providers.toml (mirrors cmdctl-tickets types for read/write)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jira: Option<JiraConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notion: Option<NotionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imperrium: Option<ImperriumConfig>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct JiraConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jql: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,
}

impl std::fmt::Debug for JiraConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraConfig")
            .field("url", &self.url)
            .field("email", &self.email)
            .field("api_token", &"***")
            .finish_non_exhaustive()
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct NotionConfig {
    #[serde(default)]
    pub api_token: String,
    #[serde(default)]
    pub database_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct ImperriumConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl std::fmt::Debug for ImperriumConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImperriumConfig")
            .field("url", &self.url)
            .field("api_token", &"***")
            .field("project", &self.project)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// File paths
// ---------------------------------------------------------------------------

fn base_dir() -> PathBuf {
    dirs::home_dir()
        .expect("HOME directory not set — cannot determine config path")
        .join(".cmdctl")
}

pub fn settings_path() -> PathBuf { base_dir().join("settings.toml") }
fn providers_path() -> PathBuf { base_dir().join("providers.toml") }

// ---------------------------------------------------------------------------
// Load / save
// ---------------------------------------------------------------------------

pub fn load_app_settings() -> AppSettings {
    let path = settings_path();
    if !path.exists() { return AppSettings::default(); }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn load_providers() -> ProvidersConfig {
    let path = providers_path();
    if !path.exists() { return ProvidersConfig::default(); }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_app_settings(settings: &AppSettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    std::fs::write(&path, toml::to_string_pretty(settings)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn save_providers(config: &ProvidersConfig) -> Result<()> {
    let path = providers_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    std::fs::write(&path, toml::to_string_pretty(config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Settings UI model
// ---------------------------------------------------------------------------

/// A row in the settings list — either a section header or an editable field.
pub enum SettingsRow {
    Section(String),
    Field {
        key: &'static str,
        label: &'static str,
        value: String,
        secret: bool,
    },
}

impl SettingsRow {
    pub fn is_field(&self) -> bool { matches!(self, Self::Field { .. }) }
}

/// Build the full settings row list from config files on disk.
pub fn build_rows() -> Vec<SettingsRow> {
    let app = load_app_settings();
    let prov = load_providers();
    let jira = prov.jira.unwrap_or_default();
    let notion = prov.notion.unwrap_or_default();
    let imp = prov.imperrium.unwrap_or_default();

    vec![
        SettingsRow::Section("GENERAL".into()),
        SettingsRow::Field { key: "general.font_name", label: "Font Name", value: app.general.font_name, secret: false },
        SettingsRow::Field { key: "general.font_size", label: "Font Size", value: app.general.font_size.to_string(), secret: false },
        SettingsRow::Field { key: "general.default_shell", label: "Default Shell", value: app.general.default_shell, secret: false },
        SettingsRow::Field { key: "general.claude_dangerously_skip_permissions", label: "Claude: Skip Permissions", value: app.general.claude_dangerously_skip_permissions.to_string(), secret: false },

        SettingsRow::Section("JIRA".into()),
        SettingsRow::Field { key: "jira.url", label: "URL", value: jira.url, secret: false },
        SettingsRow::Field { key: "jira.email", label: "Email", value: jira.email, secret: false },
        SettingsRow::Field { key: "jira.api_token", label: "API Token", value: jira.api_token, secret: true },
        SettingsRow::Field { key: "jira.jql", label: "JQL Filter", value: jira.jql.unwrap_or_default(), secret: false },
        SettingsRow::Field { key: "jira.max_results", label: "Max Results", value: jira.max_results.map(|n| n.to_string()).unwrap_or_default(), secret: false },

        SettingsRow::Section("NOTION".into()),
        SettingsRow::Field { key: "notion.api_token", label: "API Token", value: notion.api_token, secret: true },
        SettingsRow::Field { key: "notion.database_id", label: "Database ID", value: notion.database_id, secret: false },
        SettingsRow::Field { key: "notion.user_email", label: "User Email", value: notion.user_email.unwrap_or_default(), secret: false },

        SettingsRow::Section("IMPERRIUM".into()),
        SettingsRow::Field { key: "imperrium.url", label: "URL", value: imp.url, secret: false },
        SettingsRow::Field { key: "imperrium.api_token", label: "API Token", value: imp.api_token, secret: true },
        SettingsRow::Field { key: "imperrium.project", label: "Project", value: imp.project.unwrap_or_default(), secret: false },
        SettingsRow::Field { key: "imperrium.user", label: "User", value: imp.user.unwrap_or_default(), secret: false },
    ]
}

/// Find the index of the first Field row (for initial selection).
pub fn first_field_index(rows: &[SettingsRow]) -> usize {
    rows.iter().position(|r| r.is_field()).unwrap_or(0)
}

/// Save all field values back to both config files.
pub fn save_rows(rows: &[SettingsRow]) -> Result<()> {
    let val = |rows: &[SettingsRow], key: &str| -> String {
        rows.iter()
            .find_map(|r| match r {
                SettingsRow::Field { key: k, value, .. } if *k == key => Some(value.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let opt = |rows: &[SettingsRow], key: &str| -> Option<String> {
        let v = val(rows, key);
        if v.is_empty() { None } else { Some(v) }
    };

    // General settings
    let font_size_str = val(rows, "general.font_size");
    let font_size: f64 = font_size_str.parse().unwrap_or(default_font_size());
    let claude_skip = val(rows, "general.claude_dangerously_skip_permissions");
    let claude_dangerously_skip_permissions = claude_skip == "true";
    let app = AppSettings {
        general: GeneralSettings {
            font_name: val(rows, "general.font_name"),
            font_size,
            default_shell: val(rows, "general.default_shell"),
            claude_dangerously_skip_permissions,
        },
    };
    save_app_settings(&app)?;

    // Provider settings
    let jira_url = val(rows, "jira.url");
    let jira_token = val(rows, "jira.api_token");
    let jira = if jira_url.is_empty() && jira_token.is_empty() {
        None
    } else {
        Some(JiraConfig {
            url: jira_url,
            email: val(rows, "jira.email"),
            api_token: jira_token,
            jql: opt(rows, "jira.jql"),
            max_results: opt(rows, "jira.max_results").and_then(|s| s.parse().ok()),
        })
    };

    let notion_token = val(rows, "notion.api_token");
    let notion_db = val(rows, "notion.database_id");
    let notion = if notion_token.is_empty() && notion_db.is_empty() {
        None
    } else {
        Some(NotionConfig {
            api_token: notion_token,
            database_id: notion_db,
            user_email: opt(rows, "notion.user_email"),
        })
    };

    let imp_url = val(rows, "imperrium.url");
    let imp_token = val(rows, "imperrium.api_token");
    let imperrium = if imp_url.is_empty() && imp_token.is_empty() {
        None
    } else {
        Some(ImperriumConfig {
            url: imp_url,
            api_token: imp_token,
            project: opt(rows, "imperrium.project"),
            user: opt(rows, "imperrium.user"),
        })
    };

    save_providers(&ProvidersConfig { jira, notion, imperrium })?;
    Ok(())
}
