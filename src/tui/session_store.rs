use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::{client::ChatMessage, config::AppConfig};

use super::state::{AppMode, FileAttachment, PlanItem};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub saved_at_epoch: u64,
    #[serde(default)]
    pub title: String,
    pub model: String,
    pub mode: AppMode,
    #[serde(default)]
    pub active_skills: Vec<String>,
    #[serde(default)]
    pub lsp_auto_run: bool,
    #[serde(default)]
    pub plan_items: Vec<PlanItem>,
    #[serde(default)]
    pub attachments: Vec<FileAttachment>,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub modified_epoch: u64,
    pub saved_at_epoch: u64,
    pub title: String,
    pub model: String,
    pub mode: AppMode,
    pub message_count: usize,
}

pub fn save_session(
    config: &AppConfig,
    model: &str,
    mode: AppMode,
    active_skills: &[String],
    lsp_auto_run: bool,
    plan_items: &[PlanItem],
    attachments: &[FileAttachment],
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
        title: derive_title(messages),
        model: model.to_string(),
        mode,
        active_skills: active_skills.to_vec(),
        lsp_auto_run,
        plan_items: plan_items.to_vec(),
        attachments: attachments.to_vec(),
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
    active_skills: &[String],
    lsp_auto_run: bool,
    plan_items: &[PlanItem],
    attachments: &[FileAttachment],
    messages: &[ChatMessage],
    path: Option<&str>,
) -> Result<PathBuf> {
    let path = match path {
        Some(path) => resolve_user_path(path)?,
        None => default_session_path(config, "conversation", "md"),
    };
    ensure_parent_dir(&path)?;
    let mut output = format!("# MiMo TUI conversation\n\n- Model: {model}\n- Mode: {mode}\n");
    if !attachments.is_empty() {
        output.push_str("- Attachments:\n");
        for attachment in attachments {
            output.push_str(&format!("  - {}\n", attachment.path));
        }
    }
    if !plan_items.is_empty() {
        output.push_str("- Plan checklist:\n");
        for item in plan_items {
            let marker = if item.done { "x" } else { " " };
            output.push_str(&format!("  - [{}] {}\n", marker, item.text));
        }
    }
    if !active_skills.is_empty() {
        output.push_str("- Active skills:\n");
        for skill in active_skills {
            output.push_str(&format!("  - {skill}\n"));
        }
    }
    output.push_str(&format!(
        "- Auto diagnostics: {}\n",
        if lsp_auto_run { "on" } else { "off" }
    ));
    output.push('\n');
    for message in messages {
        let role = match message.role {
            crate::client::Role::System => "System",
            crate::client::Role::User => "You",
            crate::client::Role::Assistant => "MiMo",
            crate::client::Role::Tool => "Tool",
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
            let path = entry.path();
            let modified = entry
                .metadata()
                .ok()?
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            let contents = fs::read_to_string(&path).ok()?;
            let session = serde_json::from_str::<SavedSession>(&contents).ok()?;
            Some(SessionEntry {
                path,
                modified_epoch: modified,
                saved_at_epoch: session.saved_at_epoch,
                title: if session.title.trim().is_empty() {
                    derive_title(&session.messages)
                } else {
                    session.title
                },
                model: session.model,
                mode: session.mode,
                message_count: session.messages.len(),
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

fn derive_title(messages: &[ChatMessage]) -> String {
    let title = messages
        .iter()
        .find_map(|message| match message.role {
            crate::client::Role::User => {
                Some(message.content.lines().next().unwrap_or_default().trim())
            }
            _ => None,
        })
        .filter(|title| !title.is_empty())
        .unwrap_or("Untitled session");

    let mut title = title.chars().take(60).collect::<String>();
    if title.is_empty() {
        title = "Untitled session".to_string();
    }
    title
}
