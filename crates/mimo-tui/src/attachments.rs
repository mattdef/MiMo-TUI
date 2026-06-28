use std::{
    cmp::Ordering,
    env,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use dirs::home_dir;
use mimo_protocol::ChatMessage;
use mimo_state::FileAttachment;
use mimo_tools::summarize_directory;
use mimo_tui_core::input::InputBuffer;

const ATTACHMENT_SUGGESTION_LIMIT: usize = 50;
const ATTACHMENT_PREVIEW_MAX_LINES: usize = 50;
const ATTACHMENT_PREVIEW_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachmentEntryKind {
    File,
    Directory,
}

impl AttachmentEntryKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "dir",
        }
    }

    pub(crate) fn preview_label(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentQuery {
    pub(crate) token_start: usize,
    pub(crate) token_end: usize,
    pub(crate) raw: String,
}

impl AttachmentQuery {
    pub(crate) fn from_input(input: &InputBuffer) -> Option<Self> {
        let (token_start, token_end) = input.current_token_bounds();
        if token_start == token_end {
            return None;
        }

        let token = slice_chars(input.as_str(), token_start, token_end);
        let raw = token.strip_prefix('@')?.to_string();
        Some(Self {
            token_start,
            token_end,
            raw,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AttachmentSearchResults {
    pub(crate) suggestions: Vec<AttachmentSuggestion>,
    pub(crate) status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachmentSuggestion {
    pub(crate) display_path: String,
    pub(crate) resolved_path: PathBuf,
    pub(crate) kind: AttachmentEntryKind,
    pub(crate) score: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AttachmentPreview {
    pub(crate) metadata: Vec<String>,
    pub(crate) lines: Vec<String>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AttachmentPickerState {
    pub(crate) query: Option<AttachmentQuery>,
    pub(crate) dismissed_query: Option<AttachmentQuery>,
    pub(crate) open: bool,
    pub(crate) selected: usize,
    pub(crate) scroll: u16,
    pub(crate) suggestions: Vec<AttachmentSuggestion>,
    pub(crate) preview: Option<AttachmentPreview>,
    pub(crate) status: Option<String>,
}

impl AttachmentPickerState {
    pub(crate) fn is_visible(&self) -> bool {
        self.open && self.query.is_some()
    }

    pub(crate) fn selected_suggestion(&self) -> Option<&AttachmentSuggestion> {
        self.suggestions.get(self.selected)
    }

    pub(crate) fn clamp_selected(&mut self) {
        if self.suggestions.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }

        self.selected = self.selected.min(self.suggestions.len().saturating_sub(1));
    }
}

pub(crate) fn current_attachment_query(input: &InputBuffer) -> Option<AttachmentQuery> {
    AttachmentQuery::from_input(input)
}

pub(crate) fn discover_attachment_suggestions(
    query: &AttachmentQuery,
    base_dir: &Path,
) -> AttachmentSearchResults {
    let (search_root, fragment) = resolve_attachment_search_root(base_dir, &query.raw);
    let read_dir = match fs::read_dir(&search_root) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            return AttachmentSearchResults {
                suggestions: Vec::new(),
                status: Some(format!(
                    "Attachments unavailable for {}: {error}",
                    search_root.display()
                )),
            };
        }
    };

    let mut suggestions = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(score) = fuzzy_score(&name, &fragment) else {
            continue;
        };

        let kind = if file_type.is_dir() {
            AttachmentEntryKind::Directory
        } else {
            AttachmentEntryKind::File
        };
        let resolved_path = entry.path();
        let display_path = display_path_from_base(base_dir, &resolved_path);
        suggestions.push(AttachmentSuggestion {
            display_path,
            resolved_path,
            kind,
            score,
        });
    }

    suggestions.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| match (left.kind, right.kind) {
                (AttachmentEntryKind::Directory, AttachmentEntryKind::File) => Ordering::Less,
                (AttachmentEntryKind::File, AttachmentEntryKind::Directory) => Ordering::Greater,
                _ => Ordering::Equal,
            })
            .then_with(|| {
                left.display_path
                    .to_lowercase()
                    .cmp(&right.display_path.to_lowercase())
            })
            .then_with(|| left.display_path.cmp(&right.display_path))
    });

    let status = if suggestions.is_empty() {
        Some("No matching attachments".to_string())
    } else if suggestions.len() > ATTACHMENT_SUGGESTION_LIMIT {
        suggestions.truncate(ATTACHMENT_SUGGESTION_LIMIT);
        Some(format!(
            "Showing top {} matches",
            ATTACHMENT_SUGGESTION_LIMIT
        ))
    } else {
        None
    };

    AttachmentSearchResults {
        suggestions,
        status,
    }
}

pub(crate) fn build_attachment_preview(suggestion: &AttachmentSuggestion) -> AttachmentPreview {
    match suggestion.kind {
        AttachmentEntryKind::File => build_file_preview(suggestion),
        AttachmentEntryKind::Directory => build_directory_preview(suggestion),
    }
}

pub fn try_attach_from_input(
    input: &mut InputBuffer,
    attachments: &mut Vec<FileAttachment>,
) -> Result<Option<String>> {
    let Some(query) = current_attachment_query(input) else {
        return Ok(None);
    };
    if query.raw.is_empty() {
        return Ok(None);
    }

    let base_dir = env::current_dir().context("failed to determine current directory")?;
    let resolved = resolve_attachment_path(&base_dir, &query.raw)?;
    let display_path = display_path_from_base(&base_dir, &resolved);
    confirm_attachment(
        input,
        attachments,
        query.token_start,
        query.token_end,
        resolved,
        display_path,
    )
}

pub(crate) fn try_attach_from_query_if_exists(
    input: &mut InputBuffer,
    attachments: &mut Vec<FileAttachment>,
    base_dir: &Path,
    query: &AttachmentQuery,
) -> Result<Option<String>> {
    if query.raw.is_empty() {
        return Ok(None);
    }

    let Ok(resolved) = resolve_attachment_path(base_dir, &query.raw) else {
        return Ok(None);
    };
    let display_path = display_path_from_base(base_dir, &resolved);
    confirm_attachment(
        input,
        attachments,
        query.token_start,
        query.token_end,
        resolved,
        display_path,
    )
}

pub(crate) fn try_attach_suggestion(
    input: &mut InputBuffer,
    attachments: &mut Vec<FileAttachment>,
    query: &AttachmentQuery,
    suggestion: &AttachmentSuggestion,
) -> Result<Option<String>> {
    confirm_attachment(
        input,
        attachments,
        query.token_start,
        query.token_end,
        suggestion.resolved_path.clone(),
        suggestion.display_path.clone(),
    )
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

fn confirm_attachment(
    input: &mut InputBuffer,
    attachments: &mut Vec<FileAttachment>,
    start: usize,
    end: usize,
    resolved: PathBuf,
    display_path: String,
) -> Result<Option<String>> {
    if !resolved.exists() {
        bail!("attachment path does not exist: {display_path}");
    }

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

fn build_file_preview(suggestion: &AttachmentSuggestion) -> AttachmentPreview {
    let mut preview = AttachmentPreview {
        metadata: vec![format!("Path: {}", suggestion.display_path)],
        lines: Vec::new(),
        note: None,
    };

    let metadata = match fs::metadata(&suggestion.resolved_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            preview.metadata.push("Type: file".to_string());
            preview.note = Some(format!("Preview unavailable: {error}"));
            return preview;
        }
    };
    preview
        .metadata
        .push(format!("Size: {}", format_bytes(metadata.len())));

    let file = match File::open(&suggestion.resolved_path) {
        Ok(file) => file,
        Err(error) => {
            preview.metadata.push("Type: file".to_string());
            preview.note = Some(format!("Preview unavailable: {error}"));
            return preview;
        }
    };

    let mut bytes =
        Vec::with_capacity(metadata.len().min(ATTACHMENT_PREVIEW_MAX_BYTES as u64) as usize);
    let mut limited = file.take(ATTACHMENT_PREVIEW_MAX_BYTES as u64);
    if let Err(error) = limited.read_to_end(&mut bytes) {
        preview.metadata.push("Type: file".to_string());
        preview.note = Some(format!("Preview unavailable: {error}"));
        return preview;
    }

    let bytes_truncated = metadata.len() > ATTACHMENT_PREVIEW_MAX_BYTES as u64;
    let extension_label = suggestion
        .resolved_path
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| !ext.is_empty())
        .map(|extension| format!("text ({extension})"))
        .unwrap_or_else(|| "text".to_string());

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        Err(error) if bytes_truncated && error.error_len().is_none() && error.valid_up_to() > 0 => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .expect("valid UTF-8 prefix should decode")
                .to_string()
        }
        Err(_) => {
            preview.metadata.push("Type: binary/non-UTF-8".to_string());
            preview.note = Some("Preview unavailable: binary/non-UTF-8 content".to_string());
            return preview;
        }
    };

    preview.metadata.push(format!("Type: {extension_label}"));

    let mut lines = text.lines();
    for _ in 0..ATTACHMENT_PREVIEW_MAX_LINES {
        let Some(line) = lines.next() else {
            break;
        };
        preview.lines.push(line.to_string());
    }

    let mut note_parts = Vec::new();
    if lines.next().is_some() {
        note_parts.push(format!(
            "first {} lines shown",
            ATTACHMENT_PREVIEW_MAX_LINES
        ));
    }
    if bytes_truncated {
        note_parts.push(format!(
            "{} KiB read limit",
            ATTACHMENT_PREVIEW_MAX_BYTES / 1024
        ));
    }
    if !note_parts.is_empty() {
        preview.note = Some(format!("Preview truncated ({})", note_parts.join(" · ")));
    }

    preview
}

fn build_directory_preview(suggestion: &AttachmentSuggestion) -> AttachmentPreview {
    let mut preview = AttachmentPreview {
        metadata: vec![
            format!("Path: {}", suggestion.display_path),
            format!("Type: {}", suggestion.kind.preview_label()),
        ],
        lines: Vec::new(),
        note: None,
    };

    let entries = match fs::read_dir(&suggestion.resolved_path) {
        Ok(entries) => entries,
        Err(error) => {
            preview.note = Some(format!("Preview unavailable: {error}"));
            return preview;
        }
    };

    let mut entries = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().ok()?.is_dir();
            Some((name, is_dir))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.0
            .to_lowercase()
            .cmp(&right.0.to_lowercase())
            .then_with(|| left.0.cmp(&right.0))
    });

    let display_count = entries.len().min(ATTACHMENT_PREVIEW_MAX_LINES);
    preview.metadata.push(format!(
        "Entries: {}{}",
        display_count,
        if entries.len() > ATTACHMENT_PREVIEW_MAX_LINES {
            "+"
        } else {
            ""
        }
    ));

    for (name, is_dir) in entries.iter().take(ATTACHMENT_PREVIEW_MAX_LINES) {
        preview
            .lines
            .push(format!("- {name}{}", if *is_dir { "/" } else { "" }));
    }

    if entries.len() > ATTACHMENT_PREVIEW_MAX_LINES {
        preview.note = Some(format!(
            "Directory listing truncated after {} entries",
            ATTACHMENT_PREVIEW_MAX_LINES
        ));
    }

    preview
}

fn resolve_attachment_search_root(base_dir: &Path, raw: &str) -> (PathBuf, String) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (base_dir.to_path_buf(), String::new());
    }

    if raw == "~" {
        return (
            home_dir().unwrap_or_else(|| PathBuf::from("~")),
            String::new(),
        );
    }

    if let Some(rest) = raw.strip_prefix("~/") {
        let home = home_dir().unwrap_or_else(|| PathBuf::from("~"));
        return resolve_search_root_from_base(home.as_path(), rest);
    }

    resolve_search_root_from_base(base_dir, raw)
}

fn resolve_search_root_from_base(base_dir: &Path, raw: &str) -> (PathBuf, String) {
    if raw.is_empty() {
        return (base_dir.to_path_buf(), String::new());
    }

    if raw.ends_with(['/', '\\']) {
        let trimmed = raw.trim_end_matches(['/', '\\']);
        if trimmed.is_empty() {
            return (
                PathBuf::from(std::path::MAIN_SEPARATOR.to_string()),
                String::new(),
            );
        }

        let path = Path::new(trimmed);
        let search_root = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        };
        return (search_root, String::new());
    }

    resolve_path_fragment(base_dir, Path::new(raw))
}

fn resolve_path_fragment(base_dir: &Path, path: &Path) -> (PathBuf, String) {
    if path.as_os_str().is_empty() {
        return (base_dir.to_path_buf(), String::new());
    }

    if path.file_name().is_none() {
        let search_root = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        };
        return (search_root, String::new());
    }

    let fragment = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_string();
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let search_root = match parent {
        Some(parent) if parent.is_absolute() => parent.to_path_buf(),
        Some(parent) => base_dir.join(parent),
        None => base_dir.to_path_buf(),
    };

    (search_root, fragment)
}

fn resolve_attachment_path(base_dir: &Path, path: &str) -> Result<PathBuf> {
    let path = path.trim();
    if path.is_empty() {
        bail!("attachment path cannot be empty");
    }

    let resolved = if let Some(stripped) = path.strip_prefix("~/") {
        home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(stripped)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base_dir.join(path)
    };

    if !resolved.exists() {
        bail!("attachment path does not exist: {path}");
    }

    Ok(resolved)
}

fn display_path_from_base(base_dir: &Path, path: &Path) -> String {
    path.strip_prefix(base_dir)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i64> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }

    let candidate_lower = candidate.to_lowercase();
    let query_lower = query.to_lowercase();
    if candidate_lower == query_lower {
        return Some(1_000_000 - candidate_lower.chars().count() as i64);
    }

    let candidate_chars = candidate_lower.chars().collect::<Vec<_>>();
    let mut positions = Vec::with_capacity(query_lower.chars().count());
    let mut search_start = 0usize;

    for needle in query_lower.chars() {
        let mut found = None;
        for (index, ch) in candidate_chars.iter().enumerate().skip(search_start) {
            if *ch == needle {
                found = Some(index);
                break;
            }
        }

        let index = found?;
        positions.push(index);
        search_start = index + 1;
    }

    let first = positions[0];
    let last = *positions.last().unwrap();
    let span = last.saturating_sub(first) + 1;
    let gaps = span.saturating_sub(positions.len());

    let prefix_bonus = if candidate_lower.starts_with(&query_lower) {
        100_000
    } else {
        0
    };
    let contiguous_bonus = if gaps == 0 { 20_000 } else { 0 };
    let first_bonus = (100usize.saturating_sub(first.min(100)) as i64) * 100;
    let gap_penalty = gaps as i64 * 50;
    let length_penalty = candidate_chars.len() as i64;

    Some(prefix_bonus + contiguous_bonus + first_bonus - gap_penalty - length_penalty)
}

fn slice_chars(value: &str, start: usize, end: usize) -> String {
    value
        .chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

fn summarize_attachment(path: &Path) -> Result<String> {
    if path.is_dir() {
        return summarize_directory(path, 2, 30);
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn detects_active_and_inactive_queries() {
        let active = InputBuffer::from("hello @src/lib");
        let query = current_attachment_query(&active).expect("query should be active");
        assert_eq!(query.token_start, 6);
        assert_eq!(query.token_end, 14);
        assert_eq!(query.raw, "src/lib");

        let bare = InputBuffer::from("@");
        let query = current_attachment_query(&bare).expect("bare @ should still be active");
        assert_eq!(query.token_start, 0);
        assert_eq!(query.token_end, 1);
        assert!(query.raw.is_empty());

        let inactive = InputBuffer::from("hello world");
        assert!(current_attachment_query(&inactive).is_none());
    }

    #[test]
    fn discovers_files_and_directories_and_ranks_matches() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::create_dir(tempdir.path().join("folder")).expect("folder should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("file should be written");
        fs::write(tempdir.path().join("snap.txt"), "snap").expect("file should be written");

        let bare_query = AttachmentQuery {
            token_start: 0,
            token_end: 1,
            raw: String::new(),
        };
        let bare_results = discover_attachment_suggestions(&bare_query, tempdir.path());
        assert!(
            bare_results
                .suggestions
                .iter()
                .any(|suggestion| suggestion.kind == AttachmentEntryKind::Directory)
        );
        assert!(
            bare_results
                .suggestions
                .iter()
                .any(|suggestion| suggestion.kind == AttachmentEntryKind::File)
        );

        let ranked_query = AttachmentQuery {
            token_start: 0,
            token_end: 3,
            raw: "ap".to_string(),
        };
        let ranked_results = discover_attachment_suggestions(&ranked_query, tempdir.path());
        assert_eq!(
            ranked_results
                .suggestions
                .first()
                .map(|suggestion| suggestion.display_path.as_str()),
            Some("apple.txt")
        );
        assert!(ranked_results.status.is_none());
    }

    #[test]
    fn previews_text_directories_and_binary_content() {
        let tempdir = tempdir().expect("tempdir should be created");
        let text_path = tempdir.path().join("notes.txt");
        let text_contents = (1..=60)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&text_path, text_contents).expect("text file should be written");

        let dir_path = tempdir.path().join("docs");
        fs::create_dir(&dir_path).expect("directory should be created");
        fs::write(dir_path.join("alpha.md"), "alpha").expect("file should be written");
        fs::write(dir_path.join("beta.md"), "beta").expect("file should be written");

        let binary_path = tempdir.path().join("blob.bin");
        fs::write(&binary_path, [0xff, 0xfe, 0xfd]).expect("binary file should be written");

        let text_preview = build_attachment_preview(&AttachmentSuggestion {
            display_path: "notes.txt".to_string(),
            resolved_path: text_path,
            kind: AttachmentEntryKind::File,
            score: 0,
        });
        assert!(
            text_preview
                .metadata
                .iter()
                .any(|line| line.contains("Path: notes.txt"))
        );
        assert!(
            text_preview
                .metadata
                .iter()
                .any(|line| line.contains("Type: text"))
        );
        assert_eq!(text_preview.lines.len(), ATTACHMENT_PREVIEW_MAX_LINES);
        assert!(
            text_preview
                .note
                .as_deref()
                .expect("text preview should be truncated")
                .contains("Preview truncated")
        );

        let dir_preview = build_attachment_preview(&AttachmentSuggestion {
            display_path: "docs".to_string(),
            resolved_path: dir_path,
            kind: AttachmentEntryKind::Directory,
            score: 0,
        });
        assert!(
            dir_preview
                .metadata
                .iter()
                .any(|line| line.contains("Type: directory"))
        );
        assert!(
            dir_preview
                .metadata
                .iter()
                .any(|line| line.contains("Entries: 2"))
        );
        assert!(
            dir_preview
                .lines
                .iter()
                .any(|line| line.contains("alpha.md"))
        );
        assert!(dir_preview.note.is_none());

        let binary_preview = build_attachment_preview(&AttachmentSuggestion {
            display_path: "blob.bin".to_string(),
            resolved_path: binary_path,
            kind: AttachmentEntryKind::File,
            score: 0,
        });
        assert!(
            binary_preview
                .note
                .as_deref()
                .expect("binary preview should mention the problem")
                .contains("binary/non-UTF-8")
        );
    }

    #[test]
    fn previews_truncated_utf8_text_as_text() {
        let tempdir = tempdir().expect("tempdir should be created");
        let text_path = tempdir.path().join("unicode.txt");
        let text_contents = "€".repeat((ATTACHMENT_PREVIEW_MAX_BYTES / 3) + 1);
        fs::write(&text_path, text_contents).expect("unicode file should be written");

        let preview = build_attachment_preview(&AttachmentSuggestion {
            display_path: "unicode.txt".to_string(),
            resolved_path: text_path,
            kind: AttachmentEntryKind::File,
            score: 0,
        });

        assert!(
            preview
                .metadata
                .iter()
                .any(|line| line.contains("Type: text"))
        );
        assert!(
            preview
                .note
                .as_deref()
                .expect("unicode preview should be truncated")
                .contains("64 KiB read limit")
        );
        assert!(
            preview
                .lines
                .first()
                .expect("unicode preview should have content")
                .contains('€')
        );
    }

    #[test]
    fn attaches_exact_paths_without_changing_public_behavior() {
        let tempdir = tempdir().expect("tempdir should be created");
        let file_path = tempdir.path().join("report.txt");
        fs::write(&file_path, "hello world").expect("file should be written");

        let mut input = InputBuffer::from(format!("@{}", file_path.display()));
        let mut attachments = Vec::new();

        let status = try_attach_from_input(&mut input, &mut attachments)
            .expect("exact attachment should succeed")
            .expect("status should be returned");

        assert_eq!(
            status,
            format!("Attached workspace context: {}", file_path.display())
        );
        assert_eq!(input.as_str(), "");
        assert_eq!(
            attachments,
            vec![FileAttachment::new(file_path.display().to_string())]
        );
    }

    #[test]
    fn attaches_selected_suggestions_and_rejects_duplicates() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::create_dir(tempdir.path().join("folder")).expect("folder should be created");

        let query = AttachmentQuery {
            token_start: 0,
            token_end: 4,
            raw: "fol".to_string(),
        };
        let results = discover_attachment_suggestions(&query, tempdir.path());
        let suggestion = results
            .suggestions
            .iter()
            .find(|suggestion| suggestion.kind == AttachmentEntryKind::Directory)
            .cloned()
            .expect("directory suggestion should exist");

        let mut input = InputBuffer::from("@fol");
        let mut attachments = Vec::new();

        let status = try_attach_suggestion(&mut input, &mut attachments, &query, &suggestion)
            .expect("selected suggestion should attach")
            .expect("status should be returned");

        assert_eq!(
            status,
            format!("Attached workspace context: {}", suggestion.display_path)
        );
        assert_eq!(input.as_str(), "");
        assert_eq!(attachments.len(), 1);

        let mut duplicate_input = InputBuffer::from("@fol");
        let duplicate_status =
            try_attach_suggestion(&mut duplicate_input, &mut attachments, &query, &suggestion)
                .expect("duplicate suggestion should be handled")
                .expect("duplicate status should be returned");

        assert_eq!(
            duplicate_status,
            format!("Attachment already added: {}", suggestion.display_path)
        );
        assert_eq!(duplicate_input.as_str(), "");
        assert_eq!(attachments.len(), 1);
    }
}
