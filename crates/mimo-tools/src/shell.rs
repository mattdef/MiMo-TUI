use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::spec::CancellationFlag;

use super::{ApprovalRequirement, ToolContext, ToolKind, ToolResult, ToolSpec, required_str};

const MAX_OUTPUT_BUFFER_BYTES: usize = 8 * 1024 * 1024;
const TRUNCATED_OUTPUT_NOTICE: &[u8] = b"\n... output truncated ...\n";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShellStatus {
    Running,
    Completed,
    Failed,
    Killed,
    TimedOut,
}

#[derive(Debug, Clone)]
pub struct ShellResult {
    pub task_id: Option<String>,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShellJobSnapshot {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub status: ShellStatus,
    pub exit_code: Option<i32>,
    pub elapsed_ms: u64,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub stdin_available: bool,
}

pub type SharedShellManager = Arc<Mutex<ShellManager>>;

pub fn new_shared_shell_manager(workspace: PathBuf) -> SharedShellManager {
    Arc::new(Mutex::new(ShellManager::new(workspace)))
}

struct BackgroundShell {
    id: String,
    command: String,
    cwd: PathBuf,
    status: ShellStatus,
    exit_code: Option<i32>,
    started_at: Instant,
    stdout_buffer: Arc<Mutex<Vec<u8>>>,
    stderr_buffer: Arc<Mutex<Vec<u8>>>,
    stdout_cursor: usize,
    stderr_cursor: usize,
    stdin: Option<ChildStdin>,
    child: Option<Child>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
}

impl BackgroundShell {
    fn poll(&mut self) -> Result<bool> {
        if self.status != ShellStatus::Running {
            return Ok(true);
        }

        let Some(child) = self.child.as_mut() else {
            return Ok(true);
        };
        match child.try_wait().context("failed to poll child process")? {
            Some(status) => {
                self.exit_code = status.code();
                self.status = if status.success() {
                    ShellStatus::Completed
                } else {
                    ShellStatus::Failed
                };
                self.collect_output();
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn collect_output(&mut self) {
        if let Some(handle) = self.stdout_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        self.stdin = None;
        self.child = None;
    }

    fn write_stdin(&mut self, input: &str, close: bool) -> Result<()> {
        if let Some(stdin) = self.stdin.as_mut() {
            if !input.is_empty() {
                stdin
                    .write_all(input.as_bytes())
                    .context("failed to write to stdin")?;
                stdin.flush().ok();
            }
        } else if !input.is_empty() {
            bail!("stdin is not available for {}", self.id);
        }

        if close {
            self.stdin = None;
        }

        Ok(())
    }

    fn full_output(&self) -> (String, String) {
        let stdout = self
            .stdout_buffer
            .lock()
            .map(|data| String::from_utf8_lossy(&data).to_string())
            .unwrap_or_else(|poison| String::from_utf8_lossy(&poison.into_inner()).to_string());
        let stderr = self
            .stderr_buffer
            .lock()
            .map(|data| String::from_utf8_lossy(&data).to_string())
            .unwrap_or_else(|poison| String::from_utf8_lossy(&poison.into_inner()).to_string());
        (stdout, stderr)
    }

    fn take_delta(&mut self) -> (String, String) {
        let stdout = take_buffer_delta(&self.stdout_buffer, &mut self.stdout_cursor);
        let stderr = take_buffer_delta(&self.stderr_buffer, &mut self.stderr_cursor);
        (
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
        )
    }

    fn snapshot(&self) -> ShellJobSnapshot {
        let (stdout, stderr) = self.full_output();
        ShellJobSnapshot {
            id: self.id.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            status: self.status,
            exit_code: self.exit_code,
            elapsed_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout_tail: tail(&stdout, 1200),
            stderr_tail: tail(&stderr, 1200),
            stdin_available: self.stdin.is_some() && self.status == ShellStatus::Running,
        }
    }

    fn kill(&mut self) -> Result<ShellResult> {
        if let Some(child) = self.child.as_mut() {
            kill_child_process(child).context("failed to kill process")?;
            let status = child.wait().ok();
            self.exit_code = status.and_then(|exit| exit.code());
        }
        self.status = ShellStatus::Killed;
        self.collect_output();
        let (stdout, stderr) = self.full_output();
        Ok(ShellResult {
            task_id: Some(self.id.clone()),
            status: self.status,
            exit_code: self.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

impl Drop for BackgroundShell {
    fn drop(&mut self) {
        if self.status == ShellStatus::Running
            && let Some(child) = self.child.as_mut()
        {
            let _ = kill_child_process(child);
            let _ = child.wait();
        }
        self.collect_output();
    }
}

pub struct ShellManager {
    processes: BTreeMap<String, BackgroundShell>,
    default_workspace: PathBuf,
    next_id: AtomicU64,
}

impl ShellManager {
    pub fn new(default_workspace: PathBuf) -> Self {
        Self {
            processes: BTreeMap::new(),
            default_workspace,
            next_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn execute(
        &mut self,
        command: &str,
        working_dir: Option<&Path>,
        timeout_ms: u64,
        background: bool,
        stdin: Option<&str>,
        cancellation: Option<CancellationFlag>,
    ) -> Result<ShellResult> {
        let work_dir = working_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_workspace.clone());
        let timeout = Duration::from_millis(timeout_ms.clamp(100, 600_000));

        if background {
            return self.spawn_background(command, &work_dir, stdin);
        }

        let started_at = Instant::now();
        let (
            mut child,
            stdout_buffer,
            stderr_buffer,
            stdout_thread,
            stderr_thread,
            mut child_stdin,
        ) = spawn_command(command, &work_dir)?;

        if let Some(input) = stdin
            && let Some(stdin_handle) = child_stdin.as_mut()
        {
            stdin_handle
                .write_all(input.as_bytes())
                .context("failed to write to stdin")?;
            stdin_handle.flush().ok();
        }
        drop(child_stdin);

        loop {
            if let Some(status) = child.try_wait().context("failed to poll child process")? {
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                let stdout = collect_locked_output(&stdout_buffer);
                let stderr = collect_locked_output(&stderr_buffer);
                return Ok(ShellResult {
                    task_id: None,
                    status: if status.success() {
                        ShellStatus::Completed
                    } else {
                        ShellStatus::Failed
                    },
                    exit_code: status.code(),
                    stdout,
                    stderr,
                    duration_ms: u64::try_from(started_at.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                });
            }

            if started_at.elapsed() >= timeout {
                kill_child_process(&mut child).context("failed to kill timed out process")?;
                let status = child.wait().ok();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                let stdout = collect_locked_output(&stdout_buffer);
                let stderr = collect_locked_output(&stderr_buffer);
                return Ok(ShellResult {
                    task_id: None,
                    status: ShellStatus::TimedOut,
                    exit_code: status.and_then(|exit| exit.code()),
                    stdout,
                    stderr,
                    duration_ms: u64::try_from(started_at.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                });
            }

            if cancellation
                .as_ref()
                .is_some_and(CancellationFlag::is_cancelled)
            {
                kill_child_process(&mut child).context("failed to kill canceled process")?;
                let status = child.wait().ok();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                let stdout = collect_locked_output(&stdout_buffer);
                let stderr = collect_locked_output(&stderr_buffer);
                return Ok(ShellResult {
                    task_id: None,
                    status: ShellStatus::Killed,
                    exit_code: status.and_then(|exit| exit.code()),
                    stdout,
                    stderr,
                    duration_ms: u64::try_from(started_at.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                });
            }

            thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn wait(&mut self, task_id: &str, wait: bool, timeout_ms: u64) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("shell task {task_id} not found"))?;

        if wait && shell.status == ShellStatus::Running {
            let timeout = Duration::from_millis(timeout_ms.clamp(100, 600_000));
            let deadline = Instant::now() + timeout;

            loop {
                if shell.poll()? {
                    break;
                }
                let has_new_output = shell
                    .stdout_buffer
                    .lock()
                    .map(|data| data.len() > shell.stdout_cursor)
                    .unwrap_or_else(|poison| poison.into_inner().len() > shell.stdout_cursor)
                    || shell
                        .stderr_buffer
                        .lock()
                        .map(|data| data.len() > shell.stderr_cursor)
                        .unwrap_or_else(|poison| poison.into_inner().len() > shell.stderr_cursor);
                if has_new_output || Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        } else {
            shell.poll()?;
        }

        let (stdout, stderr) = shell.take_delta();
        Ok(ShellResult {
            task_id: Some(shell.id.clone()),
            status: shell.status,
            exit_code: shell.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(shell.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    pub fn write_stdin(
        &mut self,
        task_id: &str,
        input: &str,
        close_stdin: bool,
    ) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("shell task {task_id} not found"))?;
        shell.write_stdin(input, close_stdin)?;
        shell.poll()?;
        let (stdout, stderr) = shell.take_delta();
        Ok(ShellResult {
            task_id: Some(shell.id.clone()),
            status: shell.status,
            exit_code: shell.exit_code,
            stdout,
            stderr,
            duration_ms: u64::try_from(shell.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    pub fn cancel(&mut self, task_id: &str) -> Result<ShellResult> {
        let shell = self
            .processes
            .get_mut(task_id)
            .ok_or_else(|| anyhow!("shell task {task_id} not found"))?;
        shell.kill()
    }

    pub fn list_jobs(&mut self) -> Result<Vec<ShellJobSnapshot>> {
        self.prune_completed();
        let ids = self.processes.keys().cloned().collect::<Vec<_>>();
        let mut jobs = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(shell) = self.processes.get_mut(&id) {
                shell.poll()?;
                jobs.push(shell.snapshot());
            }
        }
        Ok(jobs)
    }

    fn prune_completed(&mut self) {
        self.processes
            .retain(|_, shell| shell.status == ShellStatus::Running);
    }

    fn spawn_background(
        &mut self,
        command: &str,
        working_dir: &Path,
        stdin: Option<&str>,
    ) -> Result<ShellResult> {
        let id = format!("shell-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let started_at = Instant::now();
        let (child, stdout_buffer, stderr_buffer, stdout_thread, stderr_thread, child_stdin) =
            spawn_command(command, working_dir)?;

        let mut shell = BackgroundShell {
            id: id.clone(),
            command: command.to_string(),
            cwd: working_dir.to_path_buf(),
            status: ShellStatus::Running,
            exit_code: None,
            started_at,
            stdout_buffer,
            stderr_buffer,
            stdout_cursor: 0,
            stderr_cursor: 0,
            stdin: child_stdin,
            child: Some(child),
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
        };

        if let Some(input) = stdin {
            shell.write_stdin(input, false)?;
        }

        self.processes.insert(id.clone(), shell);
        Ok(ShellResult {
            task_id: Some(id),
            status: ShellStatus::Running,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 0,
        })
    }
}

pub struct ExecShellTool;
pub struct ShellWaitTool;
pub struct ShellInteractTool;
pub struct ShellCancelTool;

impl ToolSpec for ExecShellTool {
    fn name(&self) -> &'static str {
        "exec_shell"
    }

    fn description(&self) -> &'static str {
        "Run a shell command inside the workspace. Use background=true for long-running commands."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds for foreground execution (default: 120000)"
                },
                "background": {
                    "type": "boolean",
                    "description": "Run in the background and return a task id"
                },
                "stdin": {
                    "type": "string",
                    "description": "Optional initial stdin payload"
                },
                "cwd": {
                    "type": "string",
                    "description": "Optional working directory relative to the workspace"
                },
                "tty": {
                    "type": "boolean",
                    "description": "Pseudo-terminal mode (not supported yet)"
                }
            },
            "required": ["command"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Run shell command: {}",
            input
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let command = required_str(&input, "command")?;
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(120_000);
        let background = input
            .get("background")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let stdin = input.get("stdin").and_then(Value::as_str);
        let tty = input.get("tty").and_then(Value::as_bool).unwrap_or(false);
        if tty {
            bail!("TTY mode is not supported yet");
        }

        let cwd = match input.get("cwd").and_then(Value::as_str) {
            Some(path) => {
                let resolved = context.resolve_path(path)?;
                if !resolved.is_dir() {
                    bail!("{} is not a directory", resolved.display());
                }
                Some(resolved)
            }
            None => None,
        };

        let result = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?
            .execute(
                command,
                cwd.as_deref(),
                timeout_ms,
                background,
                stdin,
                Some(context.cancellation.clone()),
            )?;

        Ok(ToolResult::new(
            render_shell_result(&result, timeout_ms),
            shell_result_summary(&result, command),
        ))
    }
}

impl ToolSpec for ShellWaitTool {
    fn name(&self) -> &'static str {
        "exec_shell_wait"
    }

    fn description(&self) -> &'static str {
        "Wait for a background shell command and return new output."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task id returned by exec_shell"
                },
                "wait": {
                    "type": "boolean",
                    "description": "Wait for new output or completion before returning (default: true)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "How long to wait for new output or completion (default: 5000)"
                }
            },
            "required": ["task_id"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Wait for shell task {}",
            input
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let task_id = required_str(&input, "task_id")?;
        let wait = input.get("wait").and_then(Value::as_bool).unwrap_or(true);
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(5_000);
        let result = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?
            .wait(task_id, wait, timeout_ms)?;

        Ok(ToolResult::new(
            render_shell_wait_result(&result),
            shell_result_summary(&result, task_id),
        ))
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }
}

impl ToolSpec for ShellInteractTool {
    fn name(&self) -> &'static str {
        "exec_shell_interact"
    }

    fn description(&self) -> &'static str {
        "Send stdin to a background shell command and return new output."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task id returned by exec_shell"
                },
                "input": {
                    "type": "string",
                    "description": "Input to write to stdin"
                },
                "close_stdin": {
                    "type": "boolean",
                    "description": "Close stdin after writing"
                }
            },
            "required": ["task_id"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Send input to shell task {}",
            input
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let task_id = required_str(&input, "task_id")?;
        let interaction = input.get("input").and_then(Value::as_str).unwrap_or("");
        let close_stdin = input
            .get("close_stdin")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let result = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?
            .write_stdin(task_id, interaction, close_stdin)?;

        Ok(ToolResult::new(
            render_shell_wait_result(&result),
            shell_result_summary(&result, task_id),
        ))
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }
}

impl ToolSpec for ShellCancelTool {
    fn name(&self) -> &'static str {
        "exec_shell_cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel a running background shell command."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "Task id returned by exec_shell"
                }
            },
            "required": ["task_id"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Shell
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Prompt
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "Cancel shell task {}",
            input
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("<missing>")
        )
    }

    fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult> {
        let task_id = required_str(&input, "task_id")?;
        let result = context
            .shell_manager
            .lock()
            .map_err(|_| anyhow!("shell manager lock poisoned"))?
            .cancel(task_id)?;

        Ok(ToolResult::new(
            render_shell_wait_result(&result),
            shell_result_summary(&result, task_id),
        ))
    }
}

fn shell_result_summary(result: &ShellResult, label: &str) -> String {
    match result.status {
        ShellStatus::Running => format!("Started background shell task for {label}"),
        ShellStatus::Completed => format!(
            "Shell command completed for {label} (exit {:?}, {} ms)",
            result.exit_code, result.duration_ms
        ),
        ShellStatus::Failed => format!(
            "Shell command failed for {label} (exit {:?}, {} ms)",
            result.exit_code, result.duration_ms
        ),
        ShellStatus::Killed => format!(
            "Shell task canceled for {label} (exit {:?}, {} ms)",
            result.exit_code, result.duration_ms
        ),
        ShellStatus::TimedOut => format!(
            "Shell command timed out for {label} after {} ms",
            result.duration_ms
        ),
    }
}

fn render_shell_result(result: &ShellResult, timeout_ms: u64) -> String {
    match result.status {
        ShellStatus::Running => format!(
            "Background shell task started: {}",
            result.task_id.as_deref().unwrap_or("<unknown>")
        ),
        ShellStatus::TimedOut => format!(
            "Command timed out after {timeout_ms}ms.\n\nSTDOUT:\n{}\n\nSTDERR:\n{}",
            trimmed_output(&result.stdout),
            trimmed_output(&result.stderr)
        ),
        ShellStatus::Completed if result.stdout.is_empty() && result.stderr.is_empty() => {
            "(no output)".to_string()
        }
        ShellStatus::Completed if result.stderr.is_empty() => trimmed_output(&result.stdout),
        ShellStatus::Completed | ShellStatus::Failed | ShellStatus::Killed => format!(
            "STDOUT:\n{}\n\nSTDERR:\n{}",
            trimmed_output(&result.stdout),
            trimmed_output(&result.stderr)
        ),
    }
}

fn render_shell_wait_result(result: &ShellResult) -> String {
    let mut output = String::new();
    if !result.stdout.is_empty() {
        output.push_str("STDOUT:\n");
        output.push_str(&trimmed_output(&result.stdout));
    }
    if !result.stderr.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("STDERR:\n");
        output.push_str(&trimmed_output(&result.stderr));
    }
    if output.is_empty() {
        output = match result.status {
            ShellStatus::Running => "Background shell task is still running.".to_string(),
            ShellStatus::Completed => {
                "Background shell task completed with no new output.".to_string()
            }
            ShellStatus::Failed => "Background shell task failed with no new output.".to_string(),
            ShellStatus::Killed => "Background shell task was canceled.".to_string(),
            ShellStatus::TimedOut => "Background shell task wait timed out.".to_string(),
        };
    }
    output
}

fn trimmed_output(text: &str) -> String {
    truncate_output(text, 12_000)
}

fn truncate_output(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n... truncated {} chars ...", count - max_chars)
}

fn tail(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    text.chars()
        .skip(count.saturating_sub(max_chars))
        .collect::<String>()
}

fn bounded_append(buffer: &mut Vec<u8>, chunk: &[u8]) {
    if buffer.len() >= MAX_OUTPUT_BUFFER_BYTES {
        return;
    }
    let remaining = MAX_OUTPUT_BUFFER_BYTES.saturating_sub(buffer.len());
    if chunk.len() <= remaining {
        buffer.extend_from_slice(chunk);
    } else {
        buffer.extend_from_slice(&chunk[..remaining]);
        buffer.extend_from_slice(TRUNCATED_OUTPUT_NOTICE);
    }
}

fn take_buffer_delta(buffer: &Arc<Mutex<Vec<u8>>>, cursor: &mut usize) -> Vec<u8> {
    let data = buffer.lock().unwrap_or_else(|poison| poison.into_inner());
    let delta = data.get(*cursor..).unwrap_or_default().to_vec();
    *cursor = data.len();
    delta
}

type SpawnedProcess = (
    Child,
    Arc<Mutex<Vec<u8>>>,
    Arc<Mutex<Vec<u8>>>,
    thread::JoinHandle<()>,
    thread::JoinHandle<()>,
    Option<ChildStdin>,
);

fn collect_locked_output(buffer: &Arc<Mutex<Vec<u8>>>) -> String {
    let data = buffer
        .lock()
        .map(|data| data.clone())
        .unwrap_or_else(|poison| (*poison.into_inner()).clone());
    String::from_utf8_lossy(&data).to_string()
}

fn spawn_command(command: &str, working_dir: &Path) -> Result<SpawnedProcess> {
    let mut cmd = shell_command(command);
    cmd.current_dir(working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to execute shell command: {command}"))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().context("failed to capture stdout")?;
    let stderr = child.stderr.take().context("failed to capture stderr")?;
    let stdout_buffer = Arc::new(Mutex::new(Vec::new()));
    let stderr_buffer = Arc::new(Mutex::new(Vec::new()));

    let stdout_thread = reader_thread(stdout, Arc::clone(&stdout_buffer));
    let stderr_thread = reader_thread(stderr, Arc::clone(&stderr_buffer));

    Ok((
        child,
        stdout_buffer,
        stderr_buffer,
        stdout_thread,
        stderr_thread,
        stdin,
    ))
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut cmd = Command::new(shell);
        cmd.arg("-c").arg(command);
        cmd
    }
}

fn reader_thread<R>(mut reader: R, buffer: Arc<Mutex<Vec<u8>>>) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(bytes) => {
                    if let Ok(mut output) = buffer.lock() {
                        bounded_append(&mut output, &chunk[..bytes]);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

#[cfg(unix)]
fn kill_child_process(child: &mut Child) -> std::io::Result<()> {
    let pid = child.id() as libc::pid_t;
    let result = unsafe { libc::kill(-pid, libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            child.kill()
        }
    }
}

#[cfg(not(unix))]
fn kill_child_process(child: &mut Child) -> std::io::Result<()> {
    child.kill()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::{
        ExecShellTool, MAX_OUTPUT_BUFFER_BYTES, ShellCancelTool, ShellInteractTool, ShellWaitTool,
        TRUNCATED_OUTPUT_NOTICE, bounded_append,
    };
    use crate::{ToolContext, ToolSpec};

    #[test]
    fn shell_output_buffer_is_bounded() {
        let mut buffer = Vec::new();
        let big = vec![b'x'; MAX_OUTPUT_BUFFER_BYTES + 1024];
        bounded_append(&mut buffer, &big);
        assert!(buffer.len() <= MAX_OUTPUT_BUFFER_BYTES + TRUNCATED_OUTPUT_NOTICE.len());
        assert!(
            buffer
                .windows(TRUNCATED_OUTPUT_NOTICE.len())
                .any(|w| w == TRUNCATED_OUTPUT_NOTICE)
        );
    }

    #[test]
    fn runs_foreground_shell_command() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(workspace.path());
        let result = ExecShellTool
            .execute(json!({ "command": "printf 'hello'" }), &context)
            .expect("shell result");
        assert!(result.content.contains("hello"));
    }

    #[test]
    fn supports_background_wait_interact_and_cancel() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(workspace.path());

        let started = ExecShellTool
            .execute(
                json!({
                    "command": "read line; printf 'got:%s' \"$line\"; sleep 10",
                    "background": true
                }),
                &context,
            )
            .expect("start result");
        let task_id = started
            .content
            .split(": ")
            .last()
            .expect("task id")
            .trim()
            .to_string();

        let interacted = ShellInteractTool
            .execute(
                json!({ "task_id": task_id, "input": "MiMo\n", "close_stdin": true }),
                &context,
            )
            .expect("interact result");
        assert!(interacted.content.contains("got:MiMo") || interacted.content.contains("running"));

        let started = ExecShellTool
            .execute(
                json!({
                    "command": "sleep 10",
                    "background": true
                }),
                &context,
            )
            .expect("start result");
        let task_id = started
            .content
            .split(": ")
            .last()
            .expect("task id")
            .trim()
            .to_string();

        let waited = ShellWaitTool
            .execute(json!({ "task_id": task_id, "wait": false }), &context)
            .expect("wait result");
        assert!(
            waited.content.contains("running")
                || waited.content.contains("STDOUT")
                || waited.content.contains("STDERR")
        );

        let canceled = ShellCancelTool
            .execute(json!({ "task_id": task_id }), &context)
            .expect("cancel result");
        assert!(canceled.summary.contains("canceled") || canceled.summary.contains("Cancel"));
    }

    #[test]
    fn cancels_foreground_shell_command() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let context = ToolContext::new(workspace.path());
        let cancellable = context.child_operation();
        let cancel_handle = cancellable.clone();
        #[cfg(windows)]
        let command = "ping -n 10 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "sleep 10";

        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel_handle.cancel();
        });

        let result = ExecShellTool
            .execute(
                json!({ "command": command, "timeout_ms": 5_000 }),
                &cancellable,
            )
            .expect("shell result");
        assert!(result.summary.contains("canceled"));
    }
}
