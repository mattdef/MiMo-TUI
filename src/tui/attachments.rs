use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, bail};

use crate::client::ChatMessage;

use super::{input::InputBuffer, project_context, state::FileAttachment};

pub fn try_attach_from_input(
    input: &mut InputBuffer,
    attachments: &mut Vec<FileAttachment>,
) -> Result<Option<String>> {
    let (start, end) = input.current_token_bounds();
    if start == end {
        return Ok(None);
    }

    let token = slice_chars(input.as_str(), start, end);
    let Some(raw_path) = token.strip_prefix('@') else {
        return Ok(None);
    };
    let raw_path = raw_path.trim();
    if raw_path.is_empty() {
        return Ok(None);
    }

    let resolved = resolve_attachment_path(raw_path)?;
    let display_path = display_path(&resolved);
    if attachments
        .iter()
        .any(|attachment| attachment.path == display_path)
    {
        input.replace_char_range(start, end, "");
        return Ok(Some(format!("Attachment already added: {display_path}")));
    }

    attachments.push(FileAttachment::new(display_path.clone()));
    input.replace_char_range(start, end, "");
    Ok(Some(format!("Attached workspace context: {display_path}")))
}

pub fn pop_last_attachment(attachments: &mut Vec<FileAttachment>) -> Option<FileAttachment> {
    attachments.pop()
}

pub fn attachment_messages(attachments: &[FileAttachment]) -> Result<Vec<ChatMessage>> {
    if attachments.is_empty() {
        return Ok(Vec::new());
    }

    let mut context = String::from(
        "Attached workspace context. Use it as read-only project context unless the user asks to modify those files explicitly.\n\n",
    );

    for attachment in attachments {
        let path = PathBuf::from(&attachment.path);
        let summary = summarize_attachment(&path)?;
        context.push_str(&format!("Path: {}\n{}\n\n", attachment.path, summary));
    }

    Ok(vec![ChatMessage::system(context)])
}

pub fn attachment_preview(attachments: &[FileAttachment]) -> String {
    if attachments.is_empty() {
        return "No attachments".to_string();
    }

    let mut preview = attachments
        .iter()
        .take(3)
        .map(|attachment| attachment.path.clone())
        .collect::<Vec<_>>()
        .join(", ");
    if attachments.len() > 3 {
        preview.push_str(&format!(" (+{} more)", attachments.len() - 3));
    }
    preview
}

fn summarize_attachment(path: &Path) -> Result<String> {
    if path.is_dir() {
        return project_context::summarize_path(path, 2, 30);
    }

    let contents = fs::read_to_string(path)?;
    let mut lines = contents.lines().take(200).collect::<Vec<_>>().join("\n");
    if contents.lines().count() > 200 || lines.chars().count() > 4_000 {
        lines.truncate(
            lines
                .char_indices()
                .nth(4_000)
                .map(|(index, _)| index)
                .unwrap_or(lines.len()),
        );
        lines.push_str("\n... truncated ...");
    }
    Ok(format!("File contents:\n```text\n{lines}\n```"))
}

fn resolve_attachment_path(path: &str) -> Result<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        bail!("attachment path cannot be empty");
    }

    let resolved = if let Some(stripped) = path.strip_prefix("~/") {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(stripped)
    } else {
        env::current_dir()?.join(path)
    };

    if !resolved.exists() {
        bail!("attachment path does not exist: {path}");
    }

    Ok(resolved)
}

fn display_path(path: &Path) -> String {
    let cwd = env::current_dir().ok();
    if let Some(cwd) = cwd
        && let Ok(relative) = path.strip_prefix(&cwd)
    {
        return relative.display().to_string();
    }
    path.display().to_string()
}

fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}
