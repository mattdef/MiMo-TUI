use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{client::ChatMessage, config::AppConfig};

use super::AppMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub saved_at_epoch: u64,
    pub model: String,
    pub mode: AppMode,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub modified_epoch: u64,
}

pub fn save_session(
    config: &AppConfig,
    model: &str,
    mode: AppMode,
    messages: &[ChatMessage],
    path: Option<&str>,
) -> Result<PathBuf> {
    let path = match path {
        Some(path) => resolve_user_path(path)?,
        None => default_session_path(config, "session", "json"),
    };
    ensure_parent_dir(&path)?;
    let session = SavedSession {
        saved_at_epoch: unix_timestamp(),
        model: model.to_string(),
        mode,
        messages: messages.to_vec(),
    };
    let contents = serde_json::to_string_pretty(&session)?;
    fs::write(&path, contents).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn load_session(config: &AppConfig, path: Option<&str>) -> Result<(SavedSession, PathBuf)> {
    let path = match path {
        Some(path) => resolve_user_path(path)?,
        None => latest_session_path(config)?,
    };
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let session = serde_json::from_str::<SavedSession>(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok((session, path))
}

pub fn export_markdown(
    config: &AppConfig,
    model: &str,
    mode: AppMode,
    messages: &[ChatMessage],
    path: Option<&str>,
) -> Result<PathBuf> {
    let path = match path {
        Some(path) => resolve_user_path(path)?,
        None => default_session_path(config, "conversation", "md"),
    };
    ensure_parent_dir(&path)?;
    let mut output = format!("# MiMo TUI conversation\n\n- Model: {model}\n- Mode: {mode}\n\n");
    for message in messages {
        let role = match message.role {
            crate::client::Role::System => "System",
            crate::client::Role::User => "You",
            crate::client::Role::Assistant => "MiMo",
        };
        output.push_str(&format!("## {role}\n\n{}\n\n", message.content.trim()));
    }
    fs::write(&path, output).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn list_sessions(config: &AppConfig) -> Result<Vec<SessionEntry>> {
    let dir = sessions_dir(config);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter_map(|entry| {
            let modified = entry
                .metadata()
                .ok()?
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            Some(SessionEntry {
                path: entry.path(),
                modified_epoch: modified,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.modified_epoch.cmp(&left.modified_epoch));
    Ok(entries)
}

fn latest_session_path(config: &AppConfig) -> Result<PathBuf> {
    list_sessions(config)?
        .into_iter()
        .next()
        .map(|entry| entry.path)
        .context("no saved sessions found")
}

fn default_session_path(config: &AppConfig, prefix: &str, extension: &str) -> PathBuf {
    sessions_dir(config).join(format!("{prefix}-{}.{}", unix_timestamp(), extension))
}

fn sessions_dir(config: &AppConfig) -> PathBuf {
    config
        .config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("sessions")
}

fn resolve_user_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        bail!("path cannot be empty");
    }
    Ok(PathBuf::from(trimmed))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    Ok(())
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
