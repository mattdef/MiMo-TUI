mod extra;
mod file;
mod history;
mod registry;
mod shell;
mod spec;

pub use extra::{
    ApplyPatchTool, FindPathsTool, GitDiffTool, GitLogTool, GitStatusTool, ProjectSummaryTool,
    RunDiagnosticsTool, RunTestsTool, SearchTextTool, WebFetchTool,
};
pub use file::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
pub use history::FileSnapshot;
pub use registry::{ToolInvocation, ToolRegistry, ToolRegistryBuilder};
pub use shell::{
    ExecShellTool, ShellCancelTool, ShellInteractTool, ShellResult, ShellStatus, ShellWaitTool,
};
pub use spec::{ApprovalRequirement, ToolContext, ToolKind, ToolResult, ToolSpec};

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde_json::Value;

/// Returns the current working directory, falling back to `"."` on error.
pub fn default_workspace_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| ".".into())
}

/// Produces a human-readable directory tree rooted at the current working
/// directory. `max_depth` and `max_entries` bound the output size.
pub fn summarize_workspace(max_depth: usize, max_entries: usize) -> Result<String> {
    let cwd = std::env::current_dir().context("failed to determine current directory")?;
    summarize_path_tree(&cwd, max_depth, max_entries)
}

/// Produces a human-readable directory tree rooted at `path`.
pub fn summarize_directory(path: &Path, max_depth: usize, max_entries: usize) -> Result<String> {
    summarize_path_tree(path, max_depth, max_entries)
}

fn summarize_path_tree(root: &Path, max_depth: usize, max_entries: usize) -> Result<String> {
    let mut lines = vec![format!("Workspace: {}", root.display())];
    let mut entries_left = max_entries;
    collect_tree_entries(root, 0, max_depth, &mut entries_left, &mut lines)?;
    if entries_left == 0 {
        lines.push("... output truncated ...".to_string());
    }
    Ok(lines.join("\n"))
}

fn collect_tree_entries(
    path: &Path,
    depth: usize,
    max_depth: usize,
    entries_left: &mut usize,
    lines: &mut Vec<String>,
) -> Result<()> {
    if depth >= max_depth || *entries_left == 0 {
        return Ok(());
    }
    let mut entries = fs::read_dir(path)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| !name.starts_with('.'))
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if *entries_left == 0 {
            break;
        }
        let file_type = entry.file_type()?;
        let indent = "  ".repeat(depth);
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let suffix = if file_type.is_dir() { "/" } else { "" };
        lines.push(format!("{indent}- {name}{suffix}"));
        *entries_left = entries_left.saturating_sub(1);
        if file_type.is_dir() {
            collect_tree_entries(&entry.path(), depth + 1, max_depth, entries_left, lines)?;
        }
    }
    Ok(())
}

pub(crate) fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("missing required field '{key}'"))
}

pub(crate) fn relative_display(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

pub(crate) fn truncate_chars(value: &str, limit: usize) -> String {
    let truncated = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        format!("{truncated}...")
    } else {
        truncated
    }
}
