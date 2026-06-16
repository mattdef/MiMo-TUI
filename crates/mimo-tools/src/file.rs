use std::fs;
use std::io::{Read, Write};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::{
    ApprovalRequirement, FileSnapshot, ToolContext, ToolKind, ToolResult, ToolSpec,
    relative_display, required_str,
};

pub struct ReadFileTool;
pub struct ListDirTool;
pub struct WriteFileTool;
pub struct EditFileTool;

impl ToolSpec for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 file from the current workspace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root or absolute within it"
                }
            },
            "required": ["path"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::FileRead
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Read file {}",
            input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let path = required_str(&input, "path")?;
        let resolved = context.resolve_path(path)?;
        let bytes = read_file_safe(&resolved)?;
        let content = String::from_utf8(bytes)
            .map_err(|_| anyhow::anyhow!("{} is not valid UTF-8 text", resolved.display()))?;
        Ok(ToolResult::new(
            content,
            format!(
                "Read {}",
                relative_display(&context.workspace_root, &resolved)
            ),
        ))
    }
}

impl ToolSpec for ListDirTool {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List directory contents within the current workspace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path relative to the workspace root (default: .)"
                }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::FileRead
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "List directory {}",
            input.get("path").and_then(Value::as_str).unwrap_or(".")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let resolved = context.resolve_path(path)?;
        if !resolved.is_dir() {
            bail!("{} is not a directory", resolved.display());
        }

        let mut entries = fs::read_dir(&resolved)
            .with_context(|| format!("failed to read {}", resolved.display()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .with_context(|| format!("failed to read {}", resolved.display()))?;
        entries.sort_by_key(|entry| entry.file_name());

        let data = entries
            .into_iter()
            .map(|entry| {
                let file_type = entry.file_type()?;
                Ok(json!({
                    "name": entry.file_name().to_string_lossy().to_string(),
                    "path": relative_display(&context.workspace_root, &entry.path()),
                    "is_dir": file_type.is_dir(),
                    "is_file": file_type.is_file(),
                    "is_symlink": file_type.is_symlink(),
                }))
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(ToolResult::new(
            serde_json::to_string_pretty(&data).context("failed to encode directory listing")?,
            format!(
                "Listed {}",
                relative_display(&context.workspace_root, &resolved)
            ),
        ))
    }
}

impl ToolSpec for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create or overwrite a UTF-8 text file inside the workspace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root or absolute within it"
                },
                "content": {
                    "type": "string",
                    "description": "UTF-8 text to write"
                }
            },
            "required": ["path", "content"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::FileWrite
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Write file {}",
            input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let path = required_str(&input, "path")?;
        let content = required_str(&input, "content")?;
        let resolved = context.resolve_path(path)?;
        let existed = resolved.exists();
        let previous = if existed {
            Some(
                fs::read_to_string(&resolved)
                    .with_context(|| format!("failed to read {}", resolved.display()))?,
            )
        } else {
            None
        };

        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        write_file_safe(&resolved, content)
            .with_context(|| format!("failed to write {}", resolved.display()))?;

        let display = relative_display(&context.workspace_root, &resolved);
        let _ = context.record_workspace_snapshot(
            format!("write_file {display}"),
            vec![FileSnapshot {
                path: display.clone(),
                previous_content: previous.clone(),
            }],
        )?;
        Ok(ToolResult::new(
            render_diff(&display, previous.as_deref().unwrap_or_default(), content),
            format!("Wrote {}", display),
        ))
    }
}

impl ToolSpec for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Edit a UTF-8 text file by exact search and replace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the workspace root or absolute within it"
                },
                "search": {
                    "type": "string",
                    "description": "Exact text to find"
                },
                "replace": {
                    "type": "string",
                    "description": "Replacement text"
                }
            },
            "required": ["path", "search", "replace"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::FileWrite
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Edit file {}",
            input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let path = required_str(&input, "path")?;
        let search = required_str(&input, "search")?;
        let replace = required_str(&input, "replace")?;
        let resolved = context.resolve_path(path)?;
        let existing = fs::read_to_string(&resolved)
            .with_context(|| format!("failed to read {}", resolved.display()))?;
        let count = existing.matches(search).count();
        if count == 0 {
            bail!("search text not found in {}", resolved.display());
        }
        if count > 1 {
            bail!(
                "search text matches {count} times in {}; provide a unique occurrence",
                resolved.display()
            );
        }

        let updated = existing.replacen(search, replace, 1);
        write_file_safe(&resolved, &updated)
            .with_context(|| format!("failed to write {}", resolved.display()))?;

        let display = relative_display(&context.workspace_root, &resolved);
        let _ = context.record_workspace_snapshot(
            format!("edit_file {display}"),
            vec![FileSnapshot {
                path: display.clone(),
                previous_content: Some(existing.clone()),
            }],
        )?;
        Ok(ToolResult::new(
            render_diff(&display, &existing, &updated),
            format!("Edited {} ({count} replacements)", display),
        ))
    }
}

/// Reads a file while refusing to follow a symlink on the final component.
///
/// This is defense-in-depth: `resolve_path` already rejects paths that
/// contain symlinks, but a race could still replace the target with a symlink.
///
/// # Platform support
///
/// - **Unix (Linux, macOS):** Uses `O_NOFOLLOW` to refuse symlinks.
/// - **Windows:** No equivalent flag is available; behaves like `fs::read`.
fn read_file_safe(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {}", path.display()))?;
    Ok(bytes)
}

fn write_file_safe(path: &std::path::Path, content: &str) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = opts
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn render_diff(path: &str, before: &str, after: &str) -> String {
    if before == after {
        return format!("--- {path}\n+++ {path}\n(no changes)");
    }

    let mut output = format!("--- {path}\n+++ {path}\n");
    for line in before.lines() {
        output.push('-');
        output.push_str(line);
        output.push('\n');
    }
    for line in after.lines() {
        output.push('+');
        output.push_str(line);
        output.push('\n');
    }
    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
    use crate::{ToolContext, ToolSpec};

    #[cfg(unix)]
    #[test]
    fn read_file_safe_rejects_symlink_paths() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("tempdir");
        let target = workspace.path().join("target.txt");
        let link = workspace.path().join("link.txt");
        fs::write(&target, "hello").expect("write");
        symlink(&target, &link).expect("symlink");

        let error = super::read_file_safe(&link).expect_err("symlink read should fail");
        assert!(error.to_string().contains("failed to open"));
    }

    #[cfg(unix)]
    #[test]
    fn read_file_tool_rejects_symlink_inside_workspace() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().expect("tempdir");
        let target = workspace.path().join("target.txt");
        let link = workspace.path().join("link.txt");
        fs::write(&target, "hello").expect("write");
        symlink(&target, &link).expect("symlink");
        let context = ToolContext::new(workspace.path());

        let error = ReadFileTool
            .execute(json!({ "path": "link.txt" }), &context)
            .expect_err("read_file should reject symlink path");
        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn reads_utf8_file() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::write(workspace.path().join("hello.txt"), "hello").expect("write");
        let context = ToolContext::new(workspace.path());

        let result = ReadFileTool
            .execute(json!({ "path": "hello.txt" }), &context)
            .expect("read result");
        assert_eq!(result.content, "hello");
    }

    #[test]
    fn lists_directory_entries() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
        fs::write(workspace.path().join("README.md"), "hello").expect("write");
        let context = ToolContext::new(workspace.path());

        let result = ListDirTool
            .execute(json!({}), &context)
            .expect("list result");
        assert!(result.content.contains("\"README.md\""));
        assert!(result.content.contains("\"src\""));
    }

    #[test]
    fn writes_file_and_returns_diff() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(workspace.path());

        let result = WriteFileTool
            .execute(
                json!({ "path": "notes.txt", "content": "hello\nworld" }),
                &context,
            )
            .expect("write result");
        assert!(result.content.contains("+++ notes.txt"));
        assert_eq!(
            fs::read_to_string(workspace.path().join("notes.txt")).expect("read"),
            "hello\nworld"
        );
    }

    #[test]
    fn edits_existing_file() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::write(workspace.path().join("notes.txt"), "hello world").expect("write");
        let context = ToolContext::new(workspace.path());

        let result = EditFileTool
            .execute(
                json!({ "path": "notes.txt", "search": "world", "replace": "MiMo" }),
                &context,
            )
            .expect("edit result");
        assert!(result.summary.contains("Edited"));
        assert_eq!(
            fs::read_to_string(workspace.path().join("notes.txt")).expect("read"),
            "hello MiMo"
        );
    }

    #[test]
    fn edit_file_rejects_multiple_matches() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::write(workspace.path().join("notes.txt"), "foo bar foo").expect("write");
        let context = ToolContext::new(workspace.path());

        let error = EditFileTool
            .execute(
                json!({ "path": "notes.txt", "search": "foo", "replace": "baz" }),
                &context,
            )
            .expect_err("edit should fail with multiple matches");
        assert!(error.to_string().contains("2 times"));
    }
}
