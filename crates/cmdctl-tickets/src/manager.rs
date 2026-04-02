//! Ticket manager — owns providers, handles caching and refresh.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tracing;

use crate::config;
use crate::imperrium::ImperriumProvider;
use crate::jira::JiraProvider;
use crate::notion::NotionProvider;
use crate::provider::{Ticket, TicketProvider};

/// How often to auto-refresh tickets from providers.
const REFRESH_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes

pub struct TicketManager {
    providers: Vec<Box<dyn TicketProvider>>,
    cached_tickets: Vec<Ticket>,
    last_refresh: Option<Instant>,
    /// User-set title overrides keyed by ticket key. Applied after each refresh.
    title_overrides: HashMap<String, String>,
}

impl TicketManager {
    /// Create a new manager, loading provider config from disk.
    pub fn new() -> Self {
        let mut providers: Vec<Box<dyn TicketProvider>> = Vec::new();

        match config::load_config() {
            Ok(cfg) => {
                if let Some(jira_cfg) = cfg.jira {
                    tracing::info!("Loaded Jira ticket provider for {}", jira_cfg.url);
                    providers.push(Box::new(JiraProvider::new(jira_cfg)));
                }
                if let Some(notion_cfg) = cfg.notion {
                    tracing::info!("Loaded Notion ticket provider");
                    providers.push(Box::new(NotionProvider::new(notion_cfg)));
                }
                if let Some(imp_cfg) = cfg.imperrium {
                    tracing::info!("Loaded Imperrium ticket provider for {}", imp_cfg.url);
                    providers.push(Box::new(ImperriumProvider::new(imp_cfg)));
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load ticket provider config: {}", e);
            }
        }

        if providers.is_empty() {
            tracing::debug!("No ticket providers configured. Edit ~/.cmdctl/providers.toml to add one.");
        }

        Self {
            providers,
            cached_tickets: Vec::new(),
            last_refresh: None,
            title_overrides: HashMap::new(),
        }
    }

    /// Returns true if any providers are configured.
    pub fn has_providers(&self) -> bool {
        !self.providers.is_empty()
    }

    /// Get cached tickets, refreshing if stale.
    pub fn tickets(&mut self) -> &[Ticket] {
        let should_refresh = match self.last_refresh {
            None => true,
            Some(t) => t.elapsed() > REFRESH_INTERVAL,
        };
        if should_refresh {
            self.refresh();
        }
        &self.cached_tickets
    }

    /// Force a refresh from all providers.
    pub fn refresh(&mut self) {
        let mut all_tickets = Vec::new();
        for provider in &self.providers {
            match provider.fetch_tickets() {
                Ok(tickets) => {
                    tracing::info!("Fetched {} tickets from {}", tickets.len(), provider.name());
                    all_tickets.extend(tickets);
                }
                Err(e) => {
                    tracing::warn!("Failed to fetch tickets from {}: {}", provider.name(), e);
                }
            }
        }

        // Deduplicate tickets by key.
        let mut seen = HashSet::new();
        all_tickets.retain(|t| seen.insert(t.key.clone()));

        // Sort: blocked/in-progress first, then by priority.
        all_tickets.sort_by(|a, b| {
            let status_ord = |t: &Ticket| -> u8 {
                match &t.status {
                    crate::provider::TicketStatus::Blocked => 0,
                    crate::provider::TicketStatus::InProgress => 1,
                    crate::provider::TicketStatus::InReview => 2,
                    crate::provider::TicketStatus::Todo => 3,
                    crate::provider::TicketStatus::Done => 5,
                    crate::provider::TicketStatus::Custom(_) => 4,
                }
            };
            let prio_ord = |t: &Ticket| -> u8 {
                match t.priority {
                    crate::provider::TicketPriority::Critical => 0,
                    crate::provider::TicketPriority::High => 1,
                    crate::provider::TicketPriority::Medium => 2,
                    crate::provider::TicketPriority::Low => 3,
                    crate::provider::TicketPriority::None => 4,
                }
            };
            status_ord(a).cmp(&status_ord(b))
                .then(prio_ord(a).cmp(&prio_ord(b)))
        });

        self.cached_tickets = all_tickets;
        self.apply_title_overrides();
        self.last_refresh = Some(Instant::now());
    }

    /// Set a local title override for a ticket. Applied immediately and survives refreshes.
    pub fn update_title(&mut self, key: &str, title: String) {
        self.title_overrides.insert(key.to_string(), title.clone());
        if let Some(ticket) = self.cached_tickets.iter_mut().find(|t| t.key == key) {
            ticket.title = title;
        }
    }

    /// Apply stored title overrides to the cached tickets.
    fn apply_title_overrides(&mut self) {
        for ticket in &mut self.cached_tickets {
            if let Some(title) = self.title_overrides.get(&ticket.key) {
                ticket.title = title.clone();
            }
        }
    }

    /// Get a ticket by key.
    pub fn get_ticket(&self, key: &str) -> Option<&Ticket> {
        self.cached_tickets.iter().find(|t| t.key == key)
    }

    /// Create a new ticket via the first provider that supports creation.
    /// If `provider_name` is specified, only that provider is tried.
    pub fn create_ticket(
        &mut self,
        title: &str,
        description: &str,
        priority: &crate::provider::TicketPriority,
        provider_name: Option<&str>,
    ) -> anyhow::Result<Ticket> {
        let provider = if let Some(name) = provider_name {
            self.providers.iter()
                .find(|p| p.name() == name && p.supports_create())
                .ok_or_else(|| anyhow::anyhow!("Provider '{}' not found or doesn't support creation", name))?
        } else {
            self.providers.iter()
                .find(|p| p.supports_create())
                .ok_or_else(|| anyhow::anyhow!("No configured provider supports ticket creation"))?
        };

        let ticket = provider.create_ticket(title, description, priority)?;
        self.cached_tickets.insert(0, ticket.clone());
        Ok(ticket)
    }

    /// Update a ticket's status on its external provider and locally.
    pub fn update_ticket_status(&mut self, key: &str, status: crate::provider::TicketStatus) -> anyhow::Result<()> {
        let provider_name = self.cached_tickets.iter()
            .find(|t| t.key == key)
            .map(|t| t.provider.clone())
            .ok_or_else(|| anyhow::anyhow!("Ticket not found: {}", key))?;

        let provider = self.providers.iter()
            .find(|p| p.name() == provider_name && p.supports_status_update())
            .ok_or_else(|| anyhow::anyhow!("Provider '{}' doesn't support status updates", provider_name))?;

        provider.update_status(key, &status)?;

        // Update local cache.
        if let Some(ticket) = self.cached_tickets.iter_mut().find(|t| t.key == key) {
            ticket.status = status;
        }
        Ok(())
    }

    /// List the names of providers that support ticket creation.
    pub fn providers_with_create(&self) -> Vec<&str> {
        self.providers.iter()
            .filter(|p| p.supports_create())
            .map(|p| p.name())
            .collect()
    }
}
