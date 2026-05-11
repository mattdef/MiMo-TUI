use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Url, blocking::Client};
use serde_json::{Value, json};

use crate::tui::project_context;

use super::{ApprovalRequirement, ToolContext, ToolKind, ToolResult, ToolSpec};

const MAX_TEXT_FILE_BYTES: u64 = 256 * 1024;

pub struct FindPathsTool;
pub struct SearchTextTool;
pub struct ProjectSummaryTool;
pub struct GitStatusTool;
pub struct GitDiffTool;
pub struct GitLogTool;
pub struct WebFetchTool;
pub struct ApplyPatchTool;
pub struct RunDiagnosticsTool;
pub struct RunTestsTool;

impl ToolSpec for FindPathsTool {
    fn name(&self) -> &'static str {
        "find_paths"
    }

    fn description(&self) -> &'static str {
        "Find files and directories in the workspace by case-insensitive substring match."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Substring to match against relative workspace paths"
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory to search from (default: .)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches to return (default: 50)"
                }
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Find paths matching {}",
            input
                .get("pattern")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let pattern = required_str(&input, "pattern")?.to_ascii_lowercase();
        let root = resolve_search_root(&input, context)?;
        let max_results = bounded_usize(&input, "max_results", 50, 200);
        let mut matches = Vec::new();
        collect_paths(
            &root,
            &context.workspace_root,
            &pattern,
            max_results,
            &mut matches,
        )?;
        Ok(ToolResult::new(
            serde_json::to_string_pretty(&matches).context("failed to encode path matches")?,
            format!("Found {} matching paths", matches.len()),
        ))
    }
}

impl ToolSpec for SearchTextTool {
    fn name(&self) -> &'static str {
        "search_text"
    }

    fn description(&self) -> &'static str {
        "Search UTF-8 text files in the workspace for a literal query."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Literal text to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory to search from (default: .)"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Whether to match case-sensitively (default: false)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches to return (default: 50)"
                }
            },
            "required": ["query"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Search text for {}",
            input
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let query = required_str(&input, "query")?;
        let root = resolve_search_root(&input, context)?;
        let case_sensitive = input
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let max_results = bounded_usize(&input, "max_results", 50, 200);
        let mut matches = Vec::new();
        search_text_recursive(
            &root,
            &context.workspace_root,
            query,
            case_sensitive,
            max_results,
            &mut matches,
        )?;
        Ok(ToolResult::new(
            serde_json::to_string_pretty(&matches).context("failed to encode search matches")?,
            format!("Found {} text matches", matches.len()),
        ))
    }
}

impl ToolSpec for ProjectSummaryTool {
    fn name(&self) -> &'static str {
        "project_summary"
    }

    fn description(&self) -> &'static str {
        "Summarize a workspace directory tree with bounded depth and entry count."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional directory to summarize (default: .)"
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum recursion depth (default: 2)"
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum number of entries to include (default: 60)"
                }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Project
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Summarize project path {}",
            input.get("path").and_then(Value::as_str).unwrap_or(".")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
        let resolved = context.resolve_path(path)?;
        if !resolved.is_dir() {
            bail!("{} is not a directory", resolved.display());
        }
        let max_depth = input
            .get("max_depth")
            .and_then(Value::as_u64)
            .unwrap_or(2)
            .clamp(1, 6) as usize;
        let max_entries = input
            .get("max_entries")
            .and_then(Value::as_u64)
            .unwrap_or(60)
            .clamp(1, 500) as usize;
        let summary = project_context::summarize_path(&resolved, max_depth, max_entries)?;
        Ok(ToolResult::new(
            summary,
            format!(
                "Summarized {}",
                relative_display(&context.workspace_root, &resolved)
            ),
        ))
    }
}

impl ToolSpec for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn description(&self) -> &'static str {
        "Show git status for the current workspace."
    }

    fn input_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Git
    }

    fn summarize(&self, _input: &Value) -> String {
        "Show git status".to_string()
    }

    fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult> {
        let output = run_command(
            context.workspace_root.as_path(),
            "git",
            &["--no-pager", "status", "--short", "--branch"],
        )?;
        Ok(ToolResult::new(output, "Loaded git status"))
    }
}

impl ToolSpec for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn description(&self) -> &'static str {
        "Show a git diff for the current workspace or a specific path."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional file or directory path to diff"
                },
                "staged": {
                    "type": "boolean",
                    "description": "Show staged diff instead of unstaged diff"
                }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Git
    }

    fn summarize(&self, input: &Value) -> String {
        let scope = input
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("workspace");
        format!("Show git diff for {scope}")
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let staged = input
            .get("staged")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut args = vec!["--no-pager".to_string(), "diff".to_string()];
        if staged {
            args.push("--cached".to_string());
        }
        if let Some(path) = input.get("path").and_then(Value::as_str) {
            let resolved = context.resolve_path(path)?;
            args.push("--".to_string());
            args.push(relative_display(&context.workspace_root, &resolved));
        }
        let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = run_command(context.workspace_root.as_path(), "git", &borrowed)?;
        Ok(ToolResult::new(output, "Loaded git diff"))
    }
}

impl ToolSpec for GitLogTool {
    fn name(&self) -> &'static str {
        "git_log"
    }

    fn description(&self) -> &'static str {
        "Show recent git commits for the current workspace."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of commits to show (default: 10)"
                }
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Git
    }

    fn summarize(&self, _input: &Value) -> String {
        "Show recent git commits".to_string()
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let limit = bounded_usize(&input, "limit", 10, 50);
        let limit = limit.to_string();
        let output = run_command(
            context.workspace_root.as_path(),
            "git",
            &["--no-pager", "log", "--decorate", "--oneline", "-n", &limit],
        )?;
        Ok(ToolResult::new(output, "Loaded git log"))
    }
}

impl ToolSpec for WebFetchTool {
    fn name(&self) -> &'static str {
        "web_fetch"
    }

    fn description(&self) -> &'static str {
        "Fetch a URL over HTTP(S) and return the response body as text."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "HTTP or HTTPS URL to fetch"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum number of response characters to return (default: 12000)"
                }
            },
            "required": ["url"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Network
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Fetch URL {}",
            input
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult> {
        let url = required_str(&input, "url")?;
        let parsed = Url::parse(url).with_context(|| format!("invalid URL: {url}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            bail!("only http(s) URLs are supported");
        }
        let max_chars = bounded_usize(&input, "max_chars", 12_000, 50_000);
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .context("failed to create HTTP client")?;
        let response = client
            .get(parsed.clone())
            .send()
            .with_context(|| format!("failed to fetch {parsed}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            bail!(
                "request failed with {status}: {}",
                truncate_chars(&body, 500)
            );
        }
        let body = response
            .text()
            .with_context(|| format!("failed to decode response from {parsed}"))?;
        Ok(ToolResult::new(
            truncate_chars(&body, max_chars),
            format!("Fetched {parsed}"),
        ))
    }
}

impl ToolSpec for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn description(&self) -> &'static str {
        "Apply a unified diff patch inside the workspace using git apply."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch text"
                }
            },
            "required": ["patch"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::FileWrite
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, _input: &Value) -> String {
        "Apply a unified diff patch".to_string()
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let patch = required_str(&input, "patch")?;
        run_command_with_stdin(
            context.workspace_root.as_path(),
            "git",
            &["apply", "--check", "--whitespace=nowarn", "-"],
            patch,
        )?;
        let preview = run_command_with_stdin(
            context.workspace_root.as_path(),
            "git",
            &["apply", "--stat", "--summary", "--whitespace=nowarn", "-"],
            patch,
        )
        .unwrap_or_else(|_| "Patch applied successfully.".to_string());
        run_command_with_stdin(
            context.workspace_root.as_path(),
            "git",
            &["apply", "--whitespace=nowarn", "-"],
            patch,
        )?;
        Ok(ToolResult::new(preview, "Applied patch"))
    }
}

impl ToolSpec for RunDiagnosticsTool {
    fn name(&self) -> &'static str {
        "run_diagnostics"
    }

    fn description(&self) -> &'static str {
        "Run the default project diagnostics command (or a custom one) inside the workspace."
    }

    fn input_schema(&self) -> Value {
        command_runner_schema(
            "Diagnostic command to run; defaults to cargo check when Cargo.toml is present",
        )
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, _input: &Value) -> String {
        "Run project diagnostics".to_string()
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        run_named_shell_command(context, input, "diagnostics", "cargo check")
    }
}

impl ToolSpec for RunTestsTool {
    fn name(&self) -> &'static str {
        "run_tests"
    }

    fn description(&self) -> &'static str {
        "Run the default project test command (or a custom one) inside the workspace."
    }

    fn input_schema(&self) -> Value {
        command_runner_schema(
            "Test command to run; defaults to cargo test when Cargo.toml is present",
        )
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, _input: &Value) -> String {
        "Run project tests".to_string()
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        run_named_shell_command(context, input, "tests", "cargo test")
    }
}

fn command_runner_schema(command_description: &str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": command_description
            },
            "cwd": {
                "type": "string",
                "description": "Optional working directory relative to the workspace"
            },
            "timeout_ms": {
                "type": "integer",
                "description": "Timeout in milliseconds for foreground execution (default: 120000)"
            },
            "background": {
                "type": "boolean",
                "description": "Run in the background and return a task id"
            }
        }
    })
}

fn run_named_shell_command(
    context: &ToolContext,
    input: Value,
    label: &str,
    default_command: &str,
) -> Result<ToolResult> {
    let timeout_ms = input
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(120_000);
    let background = input
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cwd = input.get("cwd").and_then(Value::as_str).unwrap_or(".");
    let cwd = context.resolve_path(cwd)?;
    if !cwd.is_dir() {
        bail!("{} is not a directory", cwd.display());
    }
    let command = match input.get("command").and_then(Value::as_str) {
        Some(command) if !command.trim().is_empty() => command.trim().to_string(),
        _ => default_project_command(&cwd, default_command)?,
    };

    let result = context
        .shell_manager
        .lock()
        .map_err(|_| anyhow!("shell manager is unavailable"))?
        .execute(&command, Some(&cwd), timeout_ms, background, None)?;
    Ok(ToolResult::new(
        shell_result_output(&result),
        format!("Ran {label} command"),
    ))
}

fn default_project_command(cwd: &Path, fallback: &str) -> Result<String> {
    if cwd.join("Cargo.toml").exists() {
        return Ok(fallback.to_string());
    }
    bail!(
        "no default command is available in {}; provide an explicit command",
        cwd.display()
    )
}

fn shell_result_output(result: &super::shell::ShellResult) -> String {
    let mut output = format!(
        "status: {:?}\nexit_code: {}\nduration_ms: {}",
        result.status,
        result
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string()),
        result.duration_ms
    );
    if let Some(task_id) = &result.task_id {
        output.push_str("\ntask_id: ");
        output.push_str(task_id);
    }
    if !result.stdout.trim().is_empty() {
        output.push_str("\n\nstdout:\n");
        output.push_str(&result.stdout);
    }
    if !result.stderr.trim().is_empty() {
        output.push_str("\n\nstderr:\n");
        output.push_str(&result.stderr);
    }
    output
}

fn resolve_search_root(input: &Value, context: &ToolContext) -> Result<PathBuf> {
    let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
    let resolved = context.resolve_path(path)?;
    if !resolved.is_dir() {
        bail!("{} is not a directory", resolved.display());
    }
    Ok(resolved)
}

fn collect_paths(
    root: &Path,
    workspace_root: &Path,
    pattern: &str,
    max_results: usize,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if matches.len() >= max_results {
        return Ok(());
    }

    for entry in read_dir_sorted(root)? {
        let path = entry.path();
        let relative = relative_display(workspace_root, &path);
        if relative.to_ascii_lowercase().contains(pattern) {
            let file_type = entry.file_type()?;
            matches.push(json!({
                "path": relative,
                "is_dir": file_type.is_dir(),
                "is_file": file_type.is_file(),
            }));
            if matches.len() >= max_results {
                return Ok(());
            }
        }

        if entry.file_type()?.is_dir() {
            collect_paths(&path, workspace_root, pattern, max_results, matches)?;
            if matches.len() >= max_results {
                return Ok(());
            }
        }
    }

    Ok(())
}

fn search_text_recursive(
    root: &Path,
    workspace_root: &Path,
    query: &str,
    case_sensitive: bool,
    max_results: usize,
    matches: &mut Vec<Value>,
) -> Result<()> {
    if matches.len() >= max_results {
        return Ok(());
    }

    for entry in read_dir_sorted(root)? {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            search_text_recursive(
                &path,
                workspace_root,
                query,
                case_sensitive,
                max_results,
                matches,
            )?;
            if matches.len() >= max_results {
                return Ok(());
            }
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        let metadata = entry.metadata()?;
        if metadata.len() > MAX_TEXT_FILE_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for (line_number, line) in content.lines().enumerate() {
            let found = if case_sensitive {
                line.contains(query)
            } else {
                line.to_ascii_lowercase()
                    .contains(&query.to_ascii_lowercase())
            };
            if found {
                matches.push(json!({
                    "path": relative_display(workspace_root, &path),
                    "line": line_number + 1,
                    "text": truncate_chars(line.trim(), 240),
                }));
                if matches.len() >= max_results {
                    return Ok(());
                }
            }
        }
    }

    Ok(())
}

fn read_dir_sorted(path: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(path)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| !name.starts_with('.') && name != "target")
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

fn run_command(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .current_dir(cwd)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);
    Ok(combined.trim().to_string())
}

fn run_command_with_stdin(cwd: &Path, program: &str, args: &[&str], stdin: &str) -> Result<String> {
    let mut child = Command::new(program)
        .current_dir(cwd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;
    if let Some(handle) = child.stdin.as_mut() {
        handle
            .write_all(stdin.as_bytes())
            .with_context(|| format!("failed to write stdin for {program}"))?;
    }
    let output = child
        .wait_with_output()
        .with_context(|| format!("failed to wait for {program}"))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = format!("{}{}", stdout, stderr);
    Ok(combined.trim().to_string())
}

fn required_str<'a>(input: &'a Value, key: &str) -> Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("missing required field '{key}'"))
}

fn bounded_usize(input: &Value, field: &str, default: usize, upper_bound: usize) -> usize {
    input
        .get(field)
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(default)
        .clamp(1, upper_bound)
}

fn relative_display(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let truncated = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::{FindPathsTool, SearchTextTool};
    use crate::tools::{ToolContext, ToolSpec};

    #[test]
    fn finds_matching_paths() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
        fs::write(workspace.path().join("src/main.rs"), "fn main() {}").expect("write");
        let context = ToolContext::new(workspace.path());

        let result = FindPathsTool
            .execute(json!({ "pattern": "main" }), &context)
            .expect("find result");
        assert!(result.content.contains("src/main.rs"));
    }

    #[test]
    fn searches_text_content() {
        let workspace = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(workspace.path().join("src")).expect("mkdir");
        fs::write(
            workspace.path().join("src/lib.rs"),
            "fn main() {}\n// TODO: improve search\n",
        )
        .expect("write");
        let context = ToolContext::new(workspace.path());

        let result = SearchTextTool
            .execute(json!({ "query": "todo" }), &context)
            .expect("search result");
        assert!(result.content.contains("\"line\": 2"));
    }
}
