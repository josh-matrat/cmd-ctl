//! File synchronization between `~/.cmdctl/knowledge/` markdown files and SQLite.
//!
//! Parses YAML frontmatter from markdown files to extract metadata,
//! then upserts into the knowledge store.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

use crate::store::{KnowledgeEntry, KnowledgeStore};

/// Base directory for knowledge files.
pub fn knowledge_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".cmdctl")
        .join("knowledge")
}

/// Ensure the knowledge directory structure exists.
pub fn ensure_dirs() -> Result<()> {
    let base = knowledge_dir();
    fs::create_dir_all(base.join("global"))?;
    fs::create_dir_all(base.join("projects"))?;
    Ok(())
}

/// Perform a full sync: scan all markdown files and upsert into the store.
pub fn full_sync(store: &KnowledgeStore) -> Result<SyncStats> {
    ensure_dirs()?;
    let base = knowledge_dir();
    let mut stats = SyncStats::default();

    let files = collect_markdown_files(&base)?;
    let file_paths: Vec<String> = files.iter()
        .map(|f| f.to_string_lossy().to_string())
        .collect();

    for file in &files {
        match sync_file(store, file, &base) {
            Ok(SyncAction::Inserted) => stats.inserted += 1,
            Ok(SyncAction::Updated) => stats.updated += 1,
            Ok(SyncAction::Unchanged) => stats.unchanged += 1,
            Err(e) => {
                tracing::warn!("Failed to sync {}: {}", file.display(), e);
                stats.errors += 1;
            }
        }
    }

    // Remove entries whose source files no longer exist.
    let existing = store.list(None)?;
    for entry in &existing {
        if entry.source == "file" {
            if let Some(fp) = &entry.file_path {
                if !file_paths.contains(fp) {
                    let _ = store.remove(&entry.id);
                    stats.removed += 1;
                }
            }
        }
    }

    tracing::info!(
        "Knowledge sync: {} inserted, {} updated, {} unchanged, {} removed, {} errors",
        stats.inserted, stats.updated, stats.unchanged, stats.removed, stats.errors
    );

    Ok(stats)
}

/// Sync files that have changed since the given timestamp.
pub fn incremental_sync(store: &KnowledgeStore, since: SystemTime) -> Result<SyncStats> {
    let base = knowledge_dir();
    let mut stats = SyncStats::default();

    let files = collect_markdown_files(&base)?;
    for file in &files {
        let modified = fs::metadata(file)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        if modified > since {
            match sync_file(store, file, &base) {
                Ok(SyncAction::Inserted) => stats.inserted += 1,
                Ok(SyncAction::Updated) => stats.updated += 1,
                Ok(SyncAction::Unchanged) => stats.unchanged += 1,
                Err(e) => {
                    tracing::warn!("Failed to sync {}: {}", file.display(), e);
                    stats.errors += 1;
                }
            }
        }
    }

    Ok(stats)
}

/// Write a knowledge entry as a markdown file in the knowledge directory.
pub fn write_knowledge_file(entry: &KnowledgeEntry) -> Result<PathBuf> {
    let base = knowledge_dir();
    let subdir = if entry.scope == "global" {
        base.join("global")
    } else {
        // Use last component of scope path as project name.
        let project_name = Path::new(&entry.scope)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "misc".to_string());
        let dir = base.join("projects").join(&project_name);
        fs::create_dir_all(&dir)?;
        dir
    };

    let filename = slugify(&entry.title);
    let path = subdir.join(format!("{}.md", filename));

    let mut content = String::new();
    content.push_str("---\n");
    content.push_str(&format!("title: {}\n", entry.title));
    content.push_str(&format!("scope: {}\n", entry.scope));
    if !entry.tags.is_empty() {
        content.push_str(&format!("tags: {}\n", entry.tags));
    }
    content.push_str("---\n\n");
    content.push_str(&entry.content);
    if !entry.content.ends_with('\n') {
        content.push('\n');
    }

    fs::write(&path, &content)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct SyncStats {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub errors: usize,
}

enum SyncAction {
    Inserted,
    Updated,
    Unchanged,
}

fn sync_file(store: &KnowledgeStore, path: &Path, base: &Path) -> Result<SyncAction> {
    let raw = fs::read_to_string(path)?;
    let file_path_str = path.to_string_lossy().to_string();
    let (frontmatter, body) = parse_frontmatter(&raw);

    let title = frontmatter.get("title")
        .cloned()
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Untitled".to_string())
        });

    let scope = frontmatter.get("scope")
        .cloned()
        .unwrap_or_else(|| infer_scope(path, base));

    let tags = frontmatter.get("tags")
        .cloned()
        .unwrap_or_default();

    // Check if already exists and content matches.
    if let Some(existing) = store.get_by_file_path(&file_path_str)? {
        if existing.content == body && existing.title == title && existing.tags == tags && existing.scope == scope {
            return Ok(SyncAction::Unchanged);
        }
        // Update in place.
        store.update(&existing.id, Some(&title), Some(&body), Some(&tags))?;
        return Ok(SyncAction::Updated);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let entry = KnowledgeEntry {
        id,
        title,
        content: body,
        scope,
        tags,
        source: "file".to_string(),
        file_path: Some(file_path_str),
        created_at: String::new(),
        updated_at: String::new(),
    };
    store.insert(&entry)?;
    Ok(SyncAction::Inserted)
}

/// Infer scope from file position relative to the knowledge base directory.
fn infer_scope(path: &Path, base: &Path) -> String {
    let relative = path.strip_prefix(base).unwrap_or(path);
    let components: Vec<_> = relative.components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();

    if components.first().map(|s| s.as_str()) == Some("global") {
        return "global".to_string();
    }

    // projects/<name>/file.md → scope = <name>
    if components.first().map(|s| s.as_str()) == Some("projects") {
        if let Some(project_name) = components.get(1) {
            return project_name.clone();
        }
    }

    "global".to_string()
}

/// Parse YAML frontmatter delimited by `---`.
fn parse_frontmatter(raw: &str) -> (std::collections::HashMap<String, String>, String) {
    let mut map = std::collections::HashMap::new();

    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return (map, raw.to_string());
    }

    // Find closing ---
    let after_first = &trimmed[3..].trim_start_matches('\r');
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    if let Some(end_idx) = after_first.find("\n---") {
        let frontmatter_block = &after_first[..end_idx];
        let body_start = end_idx + 4; // "\n---"
        let body = after_first[body_start..].trim_start_matches(['\r', '\n']).to_string();

        for line in frontmatter_block.lines() {
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim().to_string();
                if !key.is_empty() {
                    map.insert(key, value);
                }
            }
        }

        (map, body)
    } else {
        (map, raw.to_string())
    }
}

fn collect_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_recursive(dir, &mut files)?;
    Ok(files)
}

fn collect_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // Skip symbolic links to prevent traversal outside the knowledge directory.
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            collect_recursive(&path, files)?;
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            files.push(path);
        }
    }
    Ok(())
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
