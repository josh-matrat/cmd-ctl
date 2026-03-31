//! Ticket provider trait and core types.

use serde::{Deserialize, Serialize};

/// Priority level for a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketPriority {
    Critical,
    High,
    Medium,
    Low,
    None,
}

impl TicketPriority {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Critical => "!!",
            Self::High => "!",
            Self::Medium => "-",
            Self::Low => ".",
            Self::None => " ",
        }
    }
}

/// Status of a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketStatus {
    Todo,
    InProgress,
    InReview,
    Done,
    Blocked,
    Custom(String),
}

impl TicketStatus {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Todo => "o",
            Self::InProgress => "*",
            Self::InReview => "~",
            Self::Done => "x",
            Self::Blocked => "!",
            Self::Custom(_) => "?",
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Todo => "Todo",
            Self::InProgress => "In Progress",
            Self::InReview => "In Review",
            Self::Done => "Done",
            Self::Blocked => "Blocked",
            Self::Custom(s) => s,
        }
    }
}

/// A ticket from any provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ticket {
    /// Provider-specific key (e.g., "PROJ-123", "NOTION-abc").
    pub key: String,
    /// Short title/summary.
    pub title: String,
    /// Full description (markdown or plain text).
    pub description: String,
    pub status: TicketStatus,
    pub priority: TicketPriority,
    /// Name of the provider (e.g., "jira", "notion").
    pub provider: String,
    /// URL to view the ticket in the browser.
    pub url: String,
    /// Assignee display name, if any.
    pub assignee: Option<String>,
    /// Labels/tags.
    pub labels: Vec<String>,
}

impl Ticket {
    /// Build a context prompt for feeding this ticket into a Claude session.
    pub fn to_context_prompt(&self) -> String {
        let mut prompt = format!(
            "I'm working on ticket {key}: {title}\n\
             Status: {status}\n\
             Priority: {priority:?}\n\
             URL: {url}\n",
            key = self.key,
            title = self.title,
            status = self.status.label(),
            priority = self.priority,
            url = self.url,
        );
        if !self.labels.is_empty() {
            prompt.push_str(&format!("Labels: {}\n", self.labels.join(", ")));
        }
        if !self.description.is_empty() {
            prompt.push_str(&format!("\nDescription:\n{}\n", self.description));
        }
        prompt.push_str("\nPlease help me work on this ticket.");
        prompt
    }
}

/// Trait that all ticket providers implement.
pub trait TicketProvider: Send + Sync {
    /// Provider name (e.g., "jira", "notion", "imperrium").
    fn name(&self) -> &str;

    /// Fetch assigned/relevant tickets for the configured user.
    fn fetch_tickets(&self) -> anyhow::Result<Vec<Ticket>>;

    /// Build a URL to view a specific ticket.
    fn ticket_url(&self, ticket: &Ticket) -> String {
        ticket.url.clone()
    }
}
