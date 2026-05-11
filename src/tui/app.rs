use std::{
    collections::BTreeMap,
    env,
    process::Command,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde_json::to_string_pretty;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::task::JoinHandle;

use crate::{
    agent::{self, AgentStatus},
    client::{ChatMessage, MimoClient, Role},
    config::{AppConfig, known_mimo_models, normalize_base_url, normalize_model_name},
    tools::{ToolContext, ToolInvocation, ToolRegistry, ToolRegistryBuilder},
};

use super::{
    attachments, command_palette,
    commands::{
        self, CommandParseError, ConfigCommand, JobsCommand, MemoryCommand, ModeName, PlanCommand,
        SlashCommand, TaskCommand,
    },
    input::InputBuffer,
    keybindings, markdown, memory_store, project_context, session_picker, session_store,
    slash_menu,
    state::{AppMode, FileAttachment, PlanItem},
    task_store,
    tooling::{ApprovalMode, ToolRequest, ToolRuntime, ToolStatus},
};

pub enum AppEvent {
    Delta(String),
    Finished(Result<(), String>),
    ModelsLoaded(Result<Vec<String>, String>),
    Status(String),
    ApprovalRequested(ToolRequest, oneshot::Sender<bool>),
    ToolStarted(ToolRequest),
    ToolFinished(ToolRequest),
    TaskStarted {
        id: String,
        routed_model: String,
    },
    TaskProgress {
        id: String,
        line: String,
    },
    TaskOutputDelta {
        id: String,
        delta: String,
    },
    TaskFinished {
        id: String,
        result: Result<(), String>,
    },
}

impl From<ModeName> for AppMode {
    fn from(value: ModeName) -> Self {
        match value {
            ModeName::Agent => Self::Agent,
            ModeName::Plan => Self::Plan,
            ModeName::Yolo => Self::Yolo,
        }
    }
}

#[derive(Debug, Default)]
struct HelpState {
    open: bool,
    filter: InputBuffer,
    scroll: u16,
}

#[derive(Debug, Default)]
struct ModelPickerState {
    open: bool,
    models: Vec<String>,
    selected: usize,
    scroll: u16,
    loading: bool,
}

#[derive(Debug, Default)]
struct SessionPickerState {
    open: bool,
    entries: Vec<session_store::SessionEntry>,
    selected: usize,
    scroll: u16,
}

#[derive(Debug, Default)]
struct CommandPaletteState {
    open: bool,
    filter: InputBuffer,
    selected: usize,
    scroll: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DraftBrowserKind {
    History,
    Stash,
}

#[derive(Debug)]
struct DraftBrowserState {
    open: bool,
    kind: DraftBrowserKind,
    selected: usize,
    scroll: u16,
}

impl Default for DraftBrowserState {
    fn default() -> Self {
        Self {
            open: false,
            kind: DraftBrowserKind::History,
            selected: 0,
            scroll: 0,
        }
    }
}

#[derive(Debug, Default)]
struct MessagePagerState {
    open: bool,
    title: String,
    lines: Vec<Line<'static>>,
    scroll: u16,
}

pub struct App {
    config: AppConfig,
    messages: Vec<ChatMessage>,
    tool_context: ToolContext,
    tool_registry: ToolRegistry,
    input: InputBuffer,
    status: String,
    streaming: bool,
    assistant_index: Option<usize>,
    scroll: u16,
    conversation_view_height: u16,
    input_scroll: u16,
    help: HelpState,
    model_picker: ModelPickerState,
    session_picker: SessionPickerState,
    command_palette: CommandPaletteState,
    draft_browser: DraftBrowserState,
    message_pager: MessagePagerState,
    discovered_models: Vec<String>,
    last_prompt: Option<String>,
    mode: AppMode,
    stream_task: Option<JoinHandle<()>>,
    model_load_task: Option<JoinHandle<()>>,
    draft_history: Vec<String>,
    draft_stash: Vec<String>,
    attachments: Vec<FileAttachment>,
    plan_items: Vec<PlanItem>,
    tool_runtime: ToolRuntime,
    approval_mode_shared: Arc<Mutex<ApprovalMode>>,
    slash_menu_selected: usize,
    tasks: Vec<task_store::SavedTask>,
    task_handles: BTreeMap<String, JoinHandle<()>>,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let workspace_root = env::current_dir().unwrap_or_else(|_| ".".into());
        let tool_context = ToolContext::new(workspace_root);
        let tool_registry = ToolRegistryBuilder::new()
            .with_file_tools()
            .with_search_tools()
            .with_git_tools()
            .with_web_tools()
            .with_project_tools()
            .with_patch_tools()
            .with_diagnostics_tool()
            .with_test_runner_tool()
            .with_shell_tools()
            .build();
        let mode = AppMode::Agent;
        let approval_mode_shared = Arc::new(Mutex::new(mode_approval_mode(mode)));
        let tasks = task_store::load_tasks(&config).unwrap_or_default();
        let _ = task_store::save_tasks(&config, &tasks);
        let status = if config.api_key.is_some() {
            "Ready".to_string()
        } else {
            "Missing API key: use /config api-key <key> or edit config.toml".to_string()
        };

        Self {
            config,
            messages: Vec::new(),
            tool_context,
            tool_registry,
            input: InputBuffer::new(),
            status,
            streaming: false,
            assistant_index: None,
            scroll: 0,
            conversation_view_height: 1,
            input_scroll: 0,
            help: HelpState::default(),
            model_picker: ModelPickerState::default(),
            session_picker: SessionPickerState::default(),
            command_palette: CommandPaletteState::default(),
            draft_browser: DraftBrowserState::default(),
            message_pager: MessagePagerState::default(),
            discovered_models: Vec::new(),
            last_prompt: None,
            mode,
            stream_task: None,
            model_load_task: None,
            draft_history: Vec::new(),
            draft_stash: Vec::new(),
            attachments: Vec::new(),
            plan_items: Vec::new(),
            tool_runtime: ToolRuntime {
                approval_mode: mode_approval_mode(mode),
                ..ToolRuntime::default()
            },
            approval_mode_shared,
            slash_menu_selected: 0,
            tasks,
            task_handles: BTreeMap::new(),
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let input_height = self.input_height(area.height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(4),
                Constraint::Length(input_height),
                Constraint::Length(1),
            ])
            .split(area);

        self.conversation_view_height = chunks[1].height.saturating_sub(2).max(1);
        self.clamp_scroll();

        self.render_header(frame, chunks[0]);
        self.render_main_panel(frame, chunks[1]);
        self.render_input(frame, chunks[2]);
        self.render_footer(frame, chunks[3]);

        if self.tool_runtime.pending_approval.is_some() {
            self.render_approval_overlay(frame, area);
        } else if self.help.open {
            self.render_help_overlay(frame, area);
        } else if self.command_palette.open {
            self.render_command_palette_overlay(frame, area);
        } else if self.draft_browser.open {
            self.render_draft_browser_overlay(frame, area);
        } else if self.session_picker.open {
            self.render_session_picker_overlay(frame, area);
        } else if self.model_picker.open {
            self.render_model_picker_overlay(frame, area);
        } else if self.message_pager.open {
            self.render_message_pager_overlay(frame, area);
        } else if self.slash_menu_visible() {
            self.render_slash_menu_overlay(frame, area);
        }
    }

    pub fn handle_terminal_event(
        &mut self,
        event: Event,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<bool> {
        match event {
            Event::Key(key) => {
                if self.tool_runtime.pending_approval.is_some() {
                    return Ok(self.handle_approval_key(key));
                }
                if self.help.open {
                    return Ok(self.handle_help_key(key));
                }
                if self.command_palette.open {
                    return self.handle_command_palette_key(key, event_tx);
                }
                if self.draft_browser.open {
                    return Ok(self.handle_draft_browser_key(key));
                }
                if self.session_picker.open {
                    return self.handle_session_picker_key(key);
                }
                if self.model_picker.open {
                    return self.handle_model_picker_key(key);
                }
                if self.message_pager.open {
                    return Ok(self.handle_message_pager_key(key));
                }

                if opens_help(key, self.input.is_empty()) {
                    self.open_help(None);
                    return Ok(false);
                }
                if opens_palette(key) {
                    self.open_command_palette();
                    return Ok(false);
                }
                if opens_session_picker(key) {
                    self.open_session_picker()?;
                    return Ok(false);
                }
                if opens_draft_history(key) {
                    self.open_draft_browser(DraftBrowserKind::History);
                    return Ok(false);
                }
                if stashes_draft(key) {
                    self.stash_current_draft();
                    return Ok(false);
                }
                if opens_last_message_pager(key) {
                    self.open_last_message_pager();
                    return Ok(false);
                }
                if toggles_mode_shortcut(key) {
                    self.toggle_mode();
                    return Ok(false);
                }

                if matches!(key.code, KeyCode::Esc) {
                    self.handle_escape_key();
                    return Ok(false);
                }

                if is_quit_key(key, self.input.is_empty()) {
                    return Ok(true);
                }

                match key.code {
                    KeyCode::Enter if inserts_newline(key) => self.input.insert_char('\n'),
                    KeyCode::Enter => {
                        if self.submit(event_tx)? {
                            return Ok(true);
                        }
                    }
                    KeyCode::Tab => {
                        if self.handle_tab_key()? {
                            return Ok(false);
                        }
                    }
                    KeyCode::Backspace if self.input.is_empty() && !self.attachments.is_empty() => {
                        if let Some(attachment) =
                            attachments::pop_last_attachment(&mut self.attachments)
                        {
                            self.status = format!("Removed attachment: {}", attachment.path);
                        }
                    }
                    KeyCode::Backspace => self.input.backspace(),
                    KeyCode::Delete => self.input.delete(),
                    KeyCode::Left => self.input.move_left(),
                    KeyCode::Right => self.input.move_right(),
                    KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll = 0
                    }
                    KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_to_bottom()
                    }
                    KeyCode::Home => self.input.move_to_line_start(),
                    KeyCode::End => self.input.move_to_line_end(),
                    KeyCode::PageUp => self.scroll_page_up(),
                    KeyCode::PageDown => self.scroll_page_down(),
                    KeyCode::Up if self.slash_menu_visible() => {
                        self.slash_menu_selected = self.slash_menu_selected.saturating_sub(1);
                    }
                    KeyCode::Down if self.slash_menu_visible() => {
                        let last = self.slash_menu_entries().len().saturating_sub(1);
                        self.slash_menu_selected = (self.slash_menu_selected + 1).min(last);
                    }
                    KeyCode::Up => self.scroll_by(-1),
                    KeyCode::Down => self.scroll_by(1),
                    KeyCode::Char('a' | 'A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.move_to_line_start();
                    }
                    KeyCode::Char('e' | 'E') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.move_to_line_end();
                    }
                    KeyCode::Char('j' | 'J') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.insert_char('\n');
                    }
                    KeyCode::Char('u' | 'U') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.clear_current_draft();
                    }
                    KeyCode::Char('\u{15}') => self.clear_current_draft(),
                    KeyCode::Char(c)
                        if !key.modifiers.contains(KeyModifiers::CONTROL)
                            && !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        self.input.insert_char(c);
                        self.clamp_slash_menu_selection();
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                self.input.insert_str(&text);
                self.clamp_slash_menu_selection();
            }
            _ => {}
        }

        Ok(false)
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Delta(delta) => {
                if let Some(index) = self.assistant_index
                    && let Some(message) = self.messages.get_mut(index)
                {
                    message.content.push_str(&delta);
                }
                self.scroll_to_bottom();
            }
            AppEvent::Finished(Ok(())) => {
                self.stream_task = None;
                self.streaming = false;
                self.assistant_index = None;
                self.status = "Ready".to_string();
                self.scroll_to_bottom();
            }
            AppEvent::Finished(Err(error)) => {
                self.stream_task = None;
                self.streaming = false;
                if let Some(index) = self.assistant_index
                    && let Some(message) = self.messages.get_mut(index)
                    && message.content.is_empty()
                {
                    message.content = format!("Request failed: {error}");
                }
                self.assistant_index = None;
                self.status = error;
                self.scroll_to_bottom();
            }
            AppEvent::ModelsLoaded(Ok(models)) => {
                self.model_load_task = None;
                self.model_picker.loading = false;
                self.discovered_models = normalize_remote_models(models);
                self.refresh_model_picker_catalog();
                let count = self.discovered_models.len();
                self.status = if count == 0 {
                    "MiMo returned an empty model catalog; using local suggestions".to_string()
                } else {
                    format!("Loaded {count} models from MiMo API")
                };
            }
            AppEvent::ModelsLoaded(Err(error)) => {
                self.model_load_task = None;
                self.model_picker.loading = false;
                self.refresh_model_picker_catalog();
                self.status = format!("Model refresh failed; using local suggestions ({error})");
            }
            AppEvent::Status(status) => {
                self.status = status;
            }
            AppEvent::ApprovalRequested(request, responder) => {
                self.tool_runtime.begin_approval(request.clone(), responder);
                self.status = format!("Approval required: {}", request.summary);
            }
            AppEvent::ToolStarted(request) => {
                self.tool_runtime.start(request.clone());
                self.status = request.summary;
            }
            AppEvent::ToolFinished(request) => {
                let status = match request.status {
                    ToolStatus::Completed => request.summary.clone(),
                    ToolStatus::Failed => format!("Tool failed: {}", request.summary),
                    ToolStatus::Denied => format!("Tool denied: {}", request.summary),
                    ToolStatus::PendingApproval | ToolStatus::Running => request.summary.clone(),
                };
                self.tool_runtime.finish(request);
                self.status = status;
            }
            AppEvent::TaskStarted { id, routed_model } => {
                if let Some(task) = self.find_task_mut(&id) {
                    task.status = task_store::TaskStatus::Running;
                    task.started_at_epoch = Some(task_store::now_epoch());
                    task.updated_at_epoch = task_store::now_epoch();
                    task.routed_model = Some(routed_model.clone());
                    task.activity_log
                        .push(format!("Started background task on {routed_model}."));
                    let _ = self.persist_tasks();
                }
                self.status = format!("Background task {id} started");
            }
            AppEvent::TaskProgress { id, line } => {
                if let Some(task) = self.find_task_mut(&id) {
                    task.updated_at_epoch = task_store::now_epoch();
                    task.activity_log.push(line);
                    trim_task_log(&mut task.activity_log);
                    let _ = self.persist_tasks();
                }
            }
            AppEvent::TaskOutputDelta { id, delta } => {
                if let Some(task) = self.find_task_mut(&id) {
                    task.updated_at_epoch = task_store::now_epoch();
                    task.assistant_output.push_str(&delta);
                    let _ = self.persist_tasks();
                }
            }
            AppEvent::TaskFinished { id, result } => {
                self.task_handles.remove(&id);
                let mut next_status = None;
                if let Some(task) = self.find_task_mut(&id) {
                    task.updated_at_epoch = task_store::now_epoch();
                    task.finished_at_epoch = Some(task_store::now_epoch());
                    match result {
                        Ok(()) => {
                            task.status = task_store::TaskStatus::Completed;
                            task.error = None;
                            task.activity_log
                                .push("Task completed successfully.".to_string());
                            next_status = Some(format!("Background task {id} completed"));
                        }
                        Err(error) => {
                            task.status = task_store::TaskStatus::Failed;
                            task.error = Some(error.clone());
                            task.activity_log.push(format!("Task failed: {error}"));
                            next_status = Some(format!("Background task {id} failed"));
                        }
                    }
                    trim_task_log(&mut task.activity_log);
                    let _ = self.persist_tasks();
                }
                if let Some(status) = next_status {
                    self.status = status;
                }
            }
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let status_style = if self.streaming {
            Style::default().fg(Color::Yellow)
        } else if self.config.api_key.is_none() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        };

        let line = Line::from(vec![
            Span::styled(
                "MiMo TUI",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" | mode: "),
            Span::styled(self.mode.to_string(), Style::default().fg(Color::Blue)),
            Span::raw(" | approvals: "),
            Span::styled(
                approval_mode_label(self.tool_runtime.approval_mode),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" | model: "),
            Span::styled(&self.config.model, Style::default().fg(Color::Magenta)),
            Span::raw(" | attachments: "),
            Span::styled(
                self.attachments.len().to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" | jobs: "),
            Span::styled(
                self.active_shell_jobs().to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" | tasks: "),
            Span::styled(
                self.tasks.len().to_string(),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" | "),
            Span::styled(&self.status, status_style),
        ]);

        frame.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
            area,
        );
    }

    fn render_main_panel(&self, frame: &mut Frame, area: Rect) {
        if self.mode == AppMode::Plan {
            let panels = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
                .split(area);
            self.render_conversation(frame, panels[0]);
            self.render_plan_panel(frame, panels[1]);
        } else {
            self.render_conversation(frame, area);
        }
    }

    fn render_conversation(&self, frame: &mut Frame, area: Rect) {
        let text = if self.messages.is_empty() {
            Text::from(vec![
                Line::styled(
                    "Welcome to MiMo TUI.",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Line::raw("Type a prompt and press Enter to stream a response from MiMo."),
                Line::raw("Use /help or F1 to discover commands and keybindings."),
                Line::raw("Use /config api-key <key> to save credentials from inside the TUI."),
            ])
        } else {
            Text::from(self.message_lines())
        };

        frame.render_widget(
            Paragraph::new(text)
                .block(Block::default().title("Conversation").borders(Borders::ALL))
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn render_plan_panel(&self, frame: &mut Frame, area: Rect) {
        let mut lines = vec![
            Line::styled(
                "Plan checklist",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
        ];
        if self.plan_items.is_empty() {
            lines.push(Line::raw("Use /plan add <text> to track planned steps."));
            lines.push(Line::raw("Use /plan done <n> to mark a step complete."));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Plan mode stays read-only until you switch to agent or yolo mode.",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            for (index, item) in self.plan_items.iter().enumerate() {
                let marker = if item.done { "[x]" } else { "[ ]" };
                let style = if item.done {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default()
                };
                lines.push(Line::styled(
                    format!("{}. {} {}", index + 1, marker, item.text),
                    style,
                ));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                format!("Tools: {}", self.tool_runtime.summary()),
                Style::default().fg(Color::DarkGray),
            ));
        }

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().title("Plan").borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            area,
        );
    }

    fn render_input(&mut self, frame: &mut Frame, area: Rect) {
        let attachment_height = if self.attachments.is_empty() { 0 } else { 3 };
        if attachment_height > 0 {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(3)])
                .split(area);
            frame.render_widget(
                Paragraph::new(attachments::attachment_preview(&self.attachments))
                    .block(
                        Block::default()
                            .title("Attached context")
                            .borders(Borders::ALL),
                    )
                    .wrap(Wrap { trim: false }),
                chunks[0],
            );
            self.render_draft_box(frame, chunks[1]);
        } else {
            self.render_draft_box(frame, area);
        }
    }

    fn render_draft_box(&mut self, frame: &mut Frame, area: Rect) {
        let title = if self.streaming {
            "Prompt (waiting for MiMo)"
        } else {
            "Prompt"
        };
        let visible_lines = area.height.saturating_sub(2).max(1) as usize;
        let (cursor_line, cursor_col) = self.input.cursor_line_col();
        let max_scroll = self
            .input
            .line_count()
            .saturating_sub(visible_lines)
            .try_into()
            .unwrap_or(u16::MAX);
        if cursor_line < self.input_scroll as usize {
            self.input_scroll = cursor_line as u16;
        } else if cursor_line >= self.input_scroll as usize + visible_lines {
            self.input_scroll = (cursor_line + 1 - visible_lines) as u16;
        }
        self.input_scroll = self.input_scroll.min(max_scroll);

        frame.render_widget(
            Paragraph::new(self.input.as_str())
                .block(Block::default().title(title).borders(Borders::ALL))
                .scroll((self.input_scroll, 0)),
            area,
        );

        if !self.help.open
            && !self.model_picker.open
            && !self.session_picker.open
            && !self.command_palette.open
            && !self.draft_browser.open
            && !self.message_pager.open
        {
            let visible_line = cursor_line.saturating_sub(self.input_scroll as usize);
            let cursor_x =
                area.x + 1 + cursor_col.min(area.width.saturating_sub(2) as usize) as u16;
            let cursor_y = area.y + 1 + visible_line.min(visible_lines.saturating_sub(1)) as u16;
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(keybindings::footer_hint()).style(Style::default().fg(Color::DarkGray)),
            area,
        );
    }

    fn render_help_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 85, 80);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(3)])
            .split(popup);

        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title("Help")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
            popup,
        );

        self.render_filter_box(frame, inner[0], &self.help.filter, "Search");

        let lines = commands::help_overlay_lines(self.help.filter.trim());
        self.help.scroll = clamp_scroll(lines.len(), inner[1].height, self.help.scroll);
        frame.render_widget(
            Paragraph::new(Text::from(
                lines.into_iter().map(Line::raw).collect::<Vec<_>>(),
            ))
            .block(
                Block::default()
                    .title("Commands and keybindings")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false })
            .scroll((self.help.scroll, 0)),
            inner[1],
        );

        set_filter_cursor(frame, inner[0], &self.help.filter);
    }

    fn render_model_picker_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 70, 60);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(popup);

        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title("MiMo models")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
            popup,
        );

        let body_height = inner[0].height.saturating_sub(2).max(1) as usize;
        self.model_picker.scroll = adjust_selection_scroll(
            self.model_picker.selected,
            self.model_picker.scroll,
            body_height,
        );
        let lines = self
            .model_picker
            .models
            .iter()
            .enumerate()
            .map(|(index, model)| {
                let prefix = if index == self.model_picker.selected {
                    "> "
                } else {
                    "  "
                };
                let style = if model == &self.config.model {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                Line::styled(format!("{prefix}{model}"), style)
            })
            .collect::<Vec<_>>();

        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(
                    Block::default()
                        .title("Select a model")
                        .borders(Borders::ALL),
                )
                .scroll((self.model_picker.scroll, 0)),
            inner[0],
        );
        frame.render_widget(
            Paragraph::new(if self.model_picker.loading {
                "Up/Down move | Enter apply | Esc cancel | Loading latest models..."
            } else {
                "Up/Down move | Enter apply | Esc cancel"
            })
            .style(Style::default().fg(Color::DarkGray)),
            inner[1],
        );
    }

    fn render_session_picker_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 80, 60);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title("Saved sessions")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
            popup,
        );

        let lines = session_picker::session_lines(
            &self.session_picker.entries,
            self.session_picker.selected,
        );
        self.session_picker.scroll =
            clamp_scroll(lines.len(), inner[0].height, self.session_picker.scroll);
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(
                    Block::default()
                        .title("Resume a session")
                        .borders(Borders::ALL),
                )
                .wrap(Wrap { trim: false })
                .scroll((self.session_picker.scroll, 0)),
            inner[0],
        );
        frame.render_widget(
            Paragraph::new("Up/Down move | Enter load | Esc cancel")
                .style(Style::default().fg(Color::DarkGray)),
            inner[1],
        );
    }

    fn render_command_palette_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 80, 70);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(3)])
            .split(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title("Command palette")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            popup,
        );

        self.render_filter_box(frame, inner[0], &self.command_palette.filter, "Filter");
        let entries =
            command_palette::filtered_entries(self.command_palette.filter.trim(), self.mode);
        self.command_palette.scroll = adjust_selection_scroll(
            self.command_palette.selected,
            self.command_palette.scroll,
            inner[1].height.saturating_sub(2).max(1) as usize,
        );
        let lines = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let prefix = if index == self.command_palette.selected {
                    "> "
                } else {
                    "  "
                };
                let style = if index == self.command_palette.selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::styled(format!("{prefix}{:<28} {}", entry.label, entry.hint), style)
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().title("Actions").borders(Borders::ALL))
                .wrap(Wrap { trim: false })
                .scroll((self.command_palette.scroll, 0)),
            inner[1],
        );
        set_filter_cursor(frame, inner[0], &self.command_palette.filter);
    }

    fn render_draft_browser_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 70, 55);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title(self.draft_browser_title())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
            popup,
        );

        self.draft_browser.scroll = adjust_selection_scroll(
            self.draft_browser.selected,
            self.draft_browser.scroll,
            inner[0].height.saturating_sub(2).max(1) as usize,
        );
        let lines = self
            .draft_browser_items()
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let prefix = if index == self.draft_browser.selected {
                    "> "
                } else {
                    "  "
                };
                let style = if index == self.draft_browser.selected {
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::styled(format!("{prefix}{item}"), style)
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().title("Drafts").borders(Borders::ALL))
                .wrap(Wrap { trim: false })
                .scroll((self.draft_browser.scroll, 0)),
            inner[0],
        );
        frame.render_widget(
            Paragraph::new("Up/Down move | Enter restore | Esc cancel")
                .style(Style::default().fg(Color::DarkGray)),
            inner[1],
        );
    }

    fn render_message_pager_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 80, 75);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(Text::from(self.message_pager.lines.clone()))
                .block(
                    Block::default()
                        .title(self.message_pager.title.as_str())
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .wrap(Wrap { trim: false })
                .scroll((self.message_pager.scroll, 0)),
            popup,
        );
    }

    fn render_approval_overlay(&self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 76, 38);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title("Tool approval")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
            popup,
        );

        let Some(pending) = self.tool_runtime.pending_approval.as_ref() else {
            return;
        };

        let kind = match pending.request.kind {
            crate::tools::ToolKind::FileRead => "File read",
            crate::tools::ToolKind::FileWrite => "File write",
            crate::tools::ToolKind::Search => "Search",
            crate::tools::ToolKind::Git => "Git",
            crate::tools::ToolKind::Network => "Network",
            crate::tools::ToolKind::Project => "Project",
            crate::tools::ToolKind::Shell => "Shell",
        };
        let mode = approval_mode_label(self.tool_runtime.approval_mode);
        let body = format!(
            "Kind: {kind}\nTool: {}\nMode: {mode}\n\n{}\n\nEnter/y approve | Esc/n deny | a approve and switch to yolo mode | r deny and switch to plan mode | p switch to agent mode",
            pending.request.name, pending.request.summary
        );
        frame.render_widget(
            Paragraph::new(body)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Pending request"),
                )
                .wrap(Wrap { trim: false }),
            centered_rect(popup, 94, 80),
        );
    }

    fn render_slash_menu_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 72, 34);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title("Commands")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Blue)),
            popup,
        );
        let entries = self.slash_menu_entries();
        let lines = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let prefix = if index == self.slash_menu_selected {
                    "> "
                } else {
                    "  "
                };
                let style = if index == self.slash_menu_selected {
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::styled(
                    format!(
                        "{prefix}{:<16} {}",
                        entry.command.usage, entry.command.description
                    ),
                    style,
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(Block::default().title("Slash menu").borders(Borders::ALL))
                .wrap(Wrap { trim: false }),
            popup,
        );
    }

    fn render_filter_box(&self, frame: &mut Frame, area: Rect, input: &InputBuffer, title: &str) {
        let filter_visible_lines = area.height.saturating_sub(2).max(1) as usize;
        let (filter_line, _) = input.cursor_line_col();
        let filter_scroll =
            filter_line.saturating_sub(filter_visible_lines.saturating_sub(1)) as u16;

        frame.render_widget(
            Paragraph::new(input.as_str())
                .block(Block::default().title(title).borders(Borders::ALL))
                .scroll((filter_scroll, 0)),
            area,
        );
    }

    fn handle_help_key(&mut self, key: KeyEvent) -> bool {
        if closes_overlay(key) {
            self.help = HelpState::default();
            return false;
        }

        match key.code {
            KeyCode::Up => self.help.scroll = self.help.scroll.saturating_sub(1),
            KeyCode::Down => self.help.scroll = self.help.scroll.saturating_add(1),
            KeyCode::PageUp => self.help.scroll = self.help.scroll.saturating_sub(10),
            KeyCode::PageDown => self.help.scroll = self.help.scroll.saturating_add(10),
            KeyCode::Home => self.help.scroll = 0,
            KeyCode::End => self.help.scroll = u16::MAX,
            _ => {
                if handle_text_input(&mut self.help.filter, key) {
                    self.help.scroll = 0;
                }
            }
        }
        false
    }

    fn handle_command_palette_key(
        &mut self,
        key: KeyEvent,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<bool> {
        if closes_overlay(key) {
            self.command_palette = CommandPaletteState::default();
            return Ok(false);
        }

        match key.code {
            KeyCode::Up => {
                self.command_palette.selected = self.command_palette.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                let last = command_palette::filtered_entries(
                    self.command_palette.filter.trim(),
                    self.mode,
                )
                .len()
                .saturating_sub(1);
                self.command_palette.selected = (self.command_palette.selected + 1).min(last);
            }
            KeyCode::Enter => {
                let entries = command_palette::filtered_entries(
                    self.command_palette.filter.trim(),
                    self.mode,
                );
                let Some(entry) = entries.get(self.command_palette.selected).cloned() else {
                    return Ok(false);
                };
                self.command_palette = CommandPaletteState::default();
                self.apply_palette_action(entry.action, event_tx)?;
            }
            _ => {
                if handle_text_input(&mut self.command_palette.filter, key) {
                    self.command_palette.selected = 0;
                    self.command_palette.scroll = 0;
                }
            }
        }
        Ok(false)
    }

    fn handle_draft_browser_key(&mut self, key: KeyEvent) -> bool {
        if closes_overlay(key) {
            self.draft_browser = DraftBrowserState::default();
            return false;
        }

        match key.code {
            KeyCode::Up => {
                self.draft_browser.selected = self.draft_browser.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                let last = self.draft_browser_items().len().saturating_sub(1);
                self.draft_browser.selected = (self.draft_browser.selected + 1).min(last);
            }
            KeyCode::Enter => self.apply_selected_draft(),
            _ => {}
        }
        false
    }

    fn handle_session_picker_key(&mut self, key: KeyEvent) -> Result<bool> {
        if closes_overlay(key) {
            self.session_picker = SessionPickerState::default();
            self.status = "Session picker closed".to_string();
            return Ok(false);
        }

        match key.code {
            KeyCode::Up => {
                self.session_picker.selected = self.session_picker.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                let last = self.session_picker.entries.len().saturating_sub(1);
                self.session_picker.selected = (self.session_picker.selected + 1).min(last);
            }
            KeyCode::Home => self.session_picker.selected = 0,
            KeyCode::End => {
                self.session_picker.selected = self.session_picker.entries.len().saturating_sub(1)
            }
            KeyCode::Enter => self.apply_selected_session()?,
            _ => {}
        }

        Ok(false)
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => self.close_model_picker(),
            KeyCode::Up => {
                self.model_picker.selected = self.model_picker.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                let last = self.model_picker.models.len().saturating_sub(1);
                self.model_picker.selected = (self.model_picker.selected + 1).min(last);
            }
            KeyCode::Home => self.model_picker.selected = 0,
            KeyCode::End => {
                self.model_picker.selected = self.model_picker.models.len().saturating_sub(1)
            }
            KeyCode::PageUp => {
                self.model_picker.selected = self.model_picker.selected.saturating_sub(5)
            }
            KeyCode::PageDown => {
                let last = self.model_picker.models.len().saturating_sub(1);
                self.model_picker.selected = (self.model_picker.selected + 5).min(last);
            }
            KeyCode::Enter => self.apply_selected_model()?,
            _ => {}
        }
        Ok(false)
    }

    fn handle_message_pager_key(&mut self, key: KeyEvent) -> bool {
        if closes_overlay(key) || opens_last_message_pager(key) {
            self.message_pager = MessagePagerState::default();
            return false;
        }

        match key.code {
            KeyCode::Up => self.message_pager.scroll = self.message_pager.scroll.saturating_sub(1),
            KeyCode::Down => {
                self.message_pager.scroll = self.message_pager.scroll.saturating_add(1)
            }
            KeyCode::PageUp => {
                self.message_pager.scroll = self.message_pager.scroll.saturating_sub(10)
            }
            KeyCode::PageDown => {
                self.message_pager.scroll = self.message_pager.scroll.saturating_add(10)
            }
            KeyCode::Home => self.message_pager.scroll = 0,
            KeyCode::End => self.message_pager.scroll = u16::MAX,
            _ => {}
        }
        false
    }

    fn handle_tab_key(&mut self) -> Result<bool> {
        if self.slash_menu_visible() {
            let entries = self.slash_menu_entries();
            if let Some(entry) = entries.get(self.slash_menu_selected) {
                self.input.set_text(entry.insertion_text());
                self.status = format!("Command selected: /{}", entry.command.name);
                self.slash_menu_selected = 0;
                return Ok(true);
            }
            if let Some(completed) = slash_menu::autocomplete_input(self.input.trim()) {
                self.input.set_text(completed.clone());
                self.status = format!("Command completed: {}", completed.trim_end());
                self.slash_menu_selected = 0;
                return Ok(true);
            }
        }

        if let Some(status) =
            attachments::try_attach_from_input(&mut self.input, &mut self.attachments)?
        {
            self.status = status;
            return Ok(true);
        }

        Ok(false)
    }

    fn submit(&mut self, event_tx: UnboundedSender<AppEvent>) -> Result<bool> {
        let prompt = self.input.trim().to_string();
        if prompt.starts_with('/') {
            return self.handle_slash_command(&prompt, event_tx);
        }

        if self.streaming {
            self.status = "MiMo is already responding".to_string();
            return Ok(false);
        }

        if prompt.is_empty() {
            self.status = "Type a prompt before sending".to_string();
            return Ok(false);
        }

        self.send_prompt(prompt, event_tx)?;
        Ok(false)
    }

    fn send_prompt(&mut self, prompt: String, event_tx: UnboundedSender<AppEvent>) -> Result<()> {
        let client = match MimoClient::new(&self.config) {
            Ok(client) => client,
            Err(error) => {
                self.status = error.to_string();
                return Ok(());
            }
        };

        let mut request_messages = self.request_messages()?;
        request_messages.push(ChatMessage::user(prompt.clone()));
        let routed_model = client.resolve_model(&request_messages);
        remember_draft(&mut self.draft_history, &prompt);
        self.last_prompt = Some(prompt.clone());
        self.input.clear();
        self.messages.push(ChatMessage::user(prompt));
        let assistant_index = self.messages.len();
        self.messages.push(ChatMessage::assistant(String::new()));
        self.assistant_index = Some(assistant_index);
        self.streaming = true;
        self.status = if self.config.model == crate::config::AUTO_MODEL {
            format!("Auto routed to {routed_model}; streaming from MiMo...")
        } else {
            "Streaming from MiMo...".to_string()
        };
        self.scroll_to_bottom();
        self.attachments.clear();
        let tool_context = self.tool_context.clone();
        let tool_registry = self.tool_registry.clone();
        let approval_mode = Arc::clone(&self.approval_mode_shared);

        let stream_task = tokio::spawn(async move {
            let result = agent::run_agent_turn(
                &client,
                request_messages,
                &tool_registry,
                &tool_context,
                |delta| {
                    event_tx
                        .send(AppEvent::Delta(delta.to_string()))
                        .context("TUI closed")?;
                    Ok(())
                },
                |status| {
                    match status {
                        AgentStatus::ToolRequested(invocation) => {
                            event_tx
                                .send(AppEvent::Status(format!(
                                    "Preparing {}",
                                    invocation.summary
                                )))
                                .context("TUI closed")?;
                        }
                        AgentStatus::ToolStarted(invocation) => {
                            let request = tool_request_from_invocation(
                                &invocation,
                                ToolStatus::Running,
                                format!("Running {}", invocation.summary),
                            );
                            event_tx
                                .send(AppEvent::ToolStarted(request))
                                .context("TUI closed")?;
                        }
                        AgentStatus::ToolSucceeded(invocation, summary) => {
                            let request = tool_request_from_invocation(
                                &invocation,
                                ToolStatus::Completed,
                                summary,
                            );
                            event_tx
                                .send(AppEvent::ToolFinished(request))
                                .context("TUI closed")?;
                        }
                        AgentStatus::ToolFailed(invocation, error) => {
                            let request = tool_request_from_invocation(
                                &invocation,
                                ToolStatus::Failed,
                                error,
                            );
                            event_tx
                                .send(AppEvent::ToolFinished(request))
                                .context("TUI closed")?;
                        }
                        AgentStatus::ToolFinished(invocation, approved) => {
                            if !approved {
                                let request = tool_request_from_invocation(
                                    &invocation,
                                    ToolStatus::Denied,
                                    format!("Denied {}", invocation.summary),
                                );
                                event_tx
                                    .send(AppEvent::ToolFinished(request))
                                    .context("TUI closed")?;
                            }
                        }
                    }
                    Ok(())
                },
                |invocation| {
                    let event_tx = event_tx.clone();
                    let approval_mode = Arc::clone(&approval_mode);
                    async move {
                        match approval_mode
                            .lock()
                            .map(|guard| *guard)
                            .unwrap_or(ApprovalMode::Prompt)
                        {
                            ApprovalMode::Auto => Ok(true),
                            ApprovalMode::ReadOnly => Ok(false),
                            ApprovalMode::Prompt => {
                                let (response_tx, response_rx) = oneshot::channel();
                                let request = tool_request_from_invocation(
                                    &invocation,
                                    ToolStatus::PendingApproval,
                                    invocation.summary.clone(),
                                );
                                event_tx
                                    .send(AppEvent::ApprovalRequested(request, response_tx))
                                    .context("TUI closed")?;
                                response_rx.await.context("approval prompt dropped")
                            }
                        }
                    }
                },
            )
            .await
            .map_err(|error| error.to_string());

            let _ = event_tx.send(AppEvent::Finished(result));
        });
        self.stream_task = Some(stream_task);

        Ok(())
    }

    fn handle_slash_command(
        &mut self,
        command_line: &str,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<bool> {
        self.input.clear();
        match commands::parse_slash_command(command_line) {
            Ok(command) => self.execute_command(command, event_tx),
            Err(CommandParseError::NotACommand) => Ok(false),
            Err(CommandParseError::UnknownCommand(name)) => {
                self.status =
                    format!("Unknown command /{name}. Type /help for available commands.");
                Ok(false)
            }
            Err(CommandParseError::Usage(usage)) => {
                self.status = format!("Usage: {usage}");
                Ok(false)
            }
        }
    }

    fn execute_command(
        &mut self,
        command: SlashCommand,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<bool> {
        match command {
            SlashCommand::Help { topic } => {
                if topic.is_some() {
                    self.push_system_message(commands::command_help(topic.as_deref()));
                }
                self.open_help(topic.as_deref());
                self.status = "Help opened".to_string();
                Ok(false)
            }
            SlashCommand::Clear => {
                self.clear_conversation();
                Ok(false)
            }
            SlashCommand::Exit => Ok(true),
            SlashCommand::Model { model } => {
                self.handle_model_command(model)?;
                Ok(false)
            }
            SlashCommand::Models => {
                self.open_model_picker(event_tx);
                Ok(false)
            }
            SlashCommand::Save { path } => {
                let saved_path = session_store::save_session(
                    &self.config,
                    &self.config.model,
                    self.mode,
                    &self.plan_items,
                    &self.attachments,
                    &self.messages,
                    path.as_deref(),
                )?;
                self.status = format!("Saved session to {}", saved_path.display());
                Ok(false)
            }
            SlashCommand::Load { path } => {
                if self.streaming {
                    self.status = "Cannot load a session while MiMo is responding".to_string();
                    return Ok(false);
                }
                let (session, loaded_path) =
                    session_store::load_session(&self.config, path.as_deref())?;
                self.apply_loaded_session(session, loaded_path.display().to_string());
                Ok(false)
            }
            SlashCommand::Sessions => {
                self.open_session_picker()?;
                Ok(false)
            }
            SlashCommand::Export { path } => {
                let export_path = session_store::export_markdown(
                    &self.config,
                    &self.config.model,
                    self.mode,
                    &self.plan_items,
                    &self.attachments,
                    &self.messages,
                    path.as_deref(),
                )?;
                self.status = format!("Exported conversation to {}", export_path.display());
                Ok(false)
            }
            SlashCommand::Config(config_command) => {
                self.handle_config_command(config_command)?;
                Ok(false)
            }
            SlashCommand::Status => {
                self.push_system_message(self.status_summary()?);
                self.status = "Status summary shown".to_string();
                Ok(false)
            }
            SlashCommand::Retry => {
                if self.streaming {
                    self.status = "Cannot retry while MiMo is responding".to_string();
                    return Ok(false);
                }
                let Some(prompt) = self.last_prompt.clone() else {
                    self.status = "No prompt available to retry".to_string();
                    return Ok(false);
                };
                self.send_prompt(prompt, event_tx)?;
                Ok(false)
            }
            SlashCommand::Context => {
                let summary = project_context::summarize_current_directory(2, 60)?;
                self.messages.push(ChatMessage::system(summary));
                self.scroll_to_bottom();
                self.status = "Project context added to the conversation".to_string();
                Ok(false)
            }
            SlashCommand::Mode { mode } => {
                if let Some(mode) = mode {
                    self.set_mode(mode.into());
                    self.status = format!("Mode switched to {}", self.mode);
                } else {
                    self.status = format!("Current mode: {}", self.mode);
                }
                Ok(false)
            }
            SlashCommand::Plan(command) => {
                self.handle_plan_command(command);
                Ok(false)
            }
            SlashCommand::Jobs(command) => {
                self.handle_jobs_command(command)?;
                Ok(false)
            }
            SlashCommand::Task(command) => {
                self.handle_task_command(command, event_tx)?;
                Ok(false)
            }
            SlashCommand::Diff => {
                self.show_workspace_diff()?;
                Ok(false)
            }
            SlashCommand::Undo => {
                self.restore_snapshot(None)?;
                Ok(false)
            }
            SlashCommand::Restore { id } => {
                self.restore_snapshot(id.as_deref())?;
                Ok(false)
            }
            SlashCommand::Note { text } => {
                let path = memory_store::append_note(&self.config, &text)?;
                self.status = format!("Saved memory note to {}", path.display());
                Ok(false)
            }
            SlashCommand::Memory(command) => {
                self.handle_memory_command(command)?;
                Ok(false)
            }
            SlashCommand::Recall { query } => {
                self.push_system_message(self.recall_matches(&query)?);
                self.status = format!("Recall results shown for {query}");
                Ok(false)
            }
            SlashCommand::Compact => {
                self.compact_conversation();
                Ok(false)
            }
        }
    }

    fn handle_model_command(&mut self, model: Option<String>) -> Result<()> {
        let Some(model) = model else {
            self.status = format!("Current model: {}", self.config.model);
            return Ok(());
        };
        let Some(model) = normalize_model_name(&model) else {
            self.status =
                "Invalid MiMo model id. Expected auto or a non-empty id starting with mimo-"
                    .to_string();
            return Ok(());
        };
        self.config.set_model(model.clone())?;
        self.status = format!("Model switched to {model}");
        Ok(())
    }

    fn handle_config_command(&mut self, command: ConfigCommand) -> Result<()> {
        match command {
            ConfigCommand::Show => {
                self.push_system_message(self.config_summary());
                self.status = "Configuration shown".to_string();
            }
            ConfigCommand::SetApiKey(api_key) => {
                self.config.set_api_key(Some(api_key))?;
                self.status = "API key saved to config.toml".to_string();
            }
            ConfigCommand::ClearApiKey => {
                self.config.set_api_key(None)?;
                self.status = "API key removed from config.toml".to_string();
            }
            ConfigCommand::SetBaseUrl(base_url) => {
                let Some(base_url) = normalize_base_url(&base_url) else {
                    self.status = "Base URL must be a valid http(s) URL".to_string();
                    return Ok(());
                };
                self.config.set_base_url(base_url.clone())?;
                self.status = format!("Base URL switched to {base_url}");
            }
            ConfigCommand::SetModel(model) => {
                let Some(model) = normalize_model_name(&model) else {
                    self.status =
                        "Invalid MiMo model id. Expected auto or a non-empty id starting with mimo-"
                            .to_string();
                    return Ok(());
                };
                self.config.set_model(model.clone())?;
                self.status = format!("Model saved as {model}");
            }
            ConfigCommand::SetTemperature(value) => match value.parse::<f32>() {
                Ok(temperature) => {
                    self.config.set_temperature(temperature)?;
                    self.status = format!("Temperature saved as {temperature}");
                }
                Err(_) => self.status = "Temperature must be a floating point number".to_string(),
            },
        }
        Ok(())
    }

    fn handle_plan_command(&mut self, command: PlanCommand) {
        match command {
            PlanCommand::Show => {
                self.push_system_message(self.plan_summary());
                self.status = "Plan checklist shown".to_string();
            }
            PlanCommand::Add(text) => {
                self.plan_items.push(PlanItem::new(text.clone()));
                self.status = format!("Added plan item: {text}");
            }
            PlanCommand::Done(index) => {
                if let Some(item) = self.plan_items.get_mut(index.saturating_sub(1)) {
                    item.done = true;
                    self.status = format!("Completed plan item {index}");
                } else {
                    self.status = format!("Unknown plan item {index}");
                }
            }
            PlanCommand::Undo(index) => {
                if let Some(item) = self.plan_items.get_mut(index.saturating_sub(1)) {
                    item.done = false;
                    self.status = format!("Reopened plan item {index}");
                } else {
                    self.status = format!("Unknown plan item {index}");
                }
            }
            PlanCommand::Remove(index) => {
                let index = index.saturating_sub(1);
                if index < self.plan_items.len() {
                    self.plan_items.remove(index);
                    self.status = format!("Removed plan item {}", index + 1);
                } else {
                    self.status = format!("Unknown plan item {}", index + 1);
                }
            }
            PlanCommand::Clear => {
                self.plan_items.clear();
                self.status = "Plan checklist cleared".to_string();
            }
        }
    }

    fn handle_memory_command(&mut self, command: MemoryCommand) -> Result<()> {
        match command {
            MemoryCommand::Show => {
                self.push_system_message(memory_store::show_memory(&self.config)?);
                self.status = "Memory shown".to_string();
            }
            MemoryCommand::Path => {
                self.push_system_message(format!(
                    "Memory path\n\n{}",
                    memory_store::memory_path(&self.config).display()
                ));
                self.status = "Memory path shown".to_string();
            }
            MemoryCommand::Clear => {
                let path = memory_store::clear_memory(&self.config)?;
                self.status = format!("Cleared memory file {}", path.display());
            }
            MemoryCommand::Help => {
                self.push_system_message(
                    "Memory commands\n\n/memory show\n/memory path\n/memory clear\n/note <text>\n/recall <query>\n/compact"
                        .to_string(),
                );
                self.status = "Memory help shown".to_string();
            }
        }
        Ok(())
    }

    fn handle_jobs_command(&mut self, command: JobsCommand) -> Result<()> {
        let (message, status) = {
            let mut manager = self
                .tool_context
                .shell_manager
                .lock()
                .map_err(|_| anyhow::anyhow!("shell manager is unavailable"))?;
            match command {
                JobsCommand::List => {
                    let jobs = manager.list_jobs()?;
                    if jobs.is_empty() {
                        (
                            "Shell jobs\n\nNo background shell jobs.".to_string(),
                            "Shell jobs listed".to_string(),
                        )
                    } else {
                        let mut output = String::from("Shell jobs\n\n");
                        for job in jobs {
                            output.push_str(&format!(
                                "- {} | {:?} | stdin={} | {}\n",
                                job.id, job.status, job.stdin_available, job.command
                            ));
                        }
                        (
                            output.trim_end().to_string(),
                            "Shell jobs listed".to_string(),
                        )
                    }
                }
                JobsCommand::Show(id) => {
                    let jobs = manager.list_jobs()?;
                    let Some(job) = jobs.into_iter().find(|job| job.id == id) else {
                        bail!("shell job {id} not found");
                    };
                    (
                        format!(
                            "Shell job details\n\n{}",
                            to_string_pretty(&job).context("failed to encode job snapshot")?
                        ),
                        format!("Shell job {id} shown"),
                    )
                }
                JobsCommand::Poll(id) => {
                    let result = manager.wait(&id, false, 100)?;
                    (
                        format!("Shell job poll\n\n{}", render_shell_result(&result)),
                        format!("Polled shell job {id}"),
                    )
                }
                JobsCommand::Wait(id) => {
                    let result = manager.wait(&id, true, 30_000)?;
                    (
                        format!("Shell job wait\n\n{}", render_shell_result(&result)),
                        format!("Waited on shell job {id}"),
                    )
                }
                JobsCommand::Stdin { id, input } => {
                    let result = manager.write_stdin(&id, &input, false)?;
                    (
                        format!("Shell job stdin\n\n{}", render_shell_result(&result)),
                        format!("Sent stdin to shell job {id}"),
                    )
                }
                JobsCommand::Cancel(id) => {
                    let result = manager.cancel(&id)?;
                    (
                        format!("Shell job cancelled\n\n{}", render_shell_result(&result)),
                        format!("Cancelled shell job {id}"),
                    )
                }
            }
        };
        self.push_system_message(message);
        self.status = status;
        Ok(())
    }

    fn handle_task_command(
        &mut self,
        command: TaskCommand,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<()> {
        match command {
            TaskCommand::Add(prompt) => self.enqueue_background_task(prompt, event_tx)?,
            TaskCommand::List => {
                if self.tasks.is_empty() {
                    self.push_system_message("Background tasks\n\nNo saved tasks.".to_string());
                } else {
                    let mut output = String::from("Background tasks\n\n");
                    for task in &self.tasks {
                        let routed = task.routed_model.as_deref().unwrap_or("-");
                        output.push_str(&format!(
                            "- {} | {:?} | mode={} | model={} | routed={} | {}\n",
                            task.id, task.status, task.mode, task.model, routed, task.prompt
                        ));
                    }
                    self.push_system_message(output.trim_end().to_string());
                }
                self.status = "Background tasks listed".to_string();
            }
            TaskCommand::Show(id) => {
                let task = self
                    .tasks
                    .iter()
                    .find(|task| task.id == id)
                    .cloned()
                    .ok_or_else(|| anyhow!("background task {id} not found"))?;
                self.push_system_message(format!(
                    "Background task details\n\n{}",
                    to_string_pretty(&task).context("failed to encode background task")?
                ));
                self.status = format!("Background task {id} shown");
            }
            TaskCommand::Cancel(id) => {
                if let Some(handle) = self.task_handles.remove(&id) {
                    handle.abort();
                }
                let Some(task) = self.find_task_mut(&id) else {
                    bail!("background task {id} not found");
                };
                task.status = task_store::TaskStatus::Cancelled;
                task.updated_at_epoch = task_store::now_epoch();
                task.finished_at_epoch = Some(task_store::now_epoch());
                task.error = Some("Cancelled by user".to_string());
                task.activity_log
                    .push("Task cancelled by user.".to_string());
                trim_task_log(&mut task.activity_log);
                self.persist_tasks()?;
                self.status = format!("Background task {id} cancelled");
            }
        }
        Ok(())
    }

    fn enqueue_background_task(
        &mut self,
        prompt: String,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<()> {
        let now = task_store::now_epoch();
        let id = task_store::next_task_id(&self.tasks);
        let task = task_store::SavedTask {
            id: id.clone(),
            prompt: prompt.clone(),
            model: self.config.model.clone(),
            routed_model: None,
            mode: self.mode,
            status: task_store::TaskStatus::Queued,
            created_at_epoch: now,
            updated_at_epoch: now,
            started_at_epoch: None,
            finished_at_epoch: None,
            assistant_output: String::new(),
            activity_log: vec!["Task queued.".to_string()],
            error: None,
        };
        self.tasks.push(task);
        task_store::sort_tasks(&mut self.tasks);
        self.persist_tasks()?;
        self.spawn_background_task(id.clone(), prompt, event_tx)?;
        self.status = format!("Queued background task {id}");
        Ok(())
    }

    fn show_workspace_diff(&mut self) -> Result<()> {
        let diff = current_workspace_diff(&self.tool_context.workspace_root)?;
        let snapshots = self.tool_context.list_workspace_snapshots()?;
        let mut output = String::from("Workspace diff\n\n");
        if diff.trim().is_empty() {
            output.push_str("No git diff.\n");
        } else {
            output.push_str(&diff);
            output.push_str("\n\n");
        }
        output.push_str("Tracked restore snapshots\n");
        if snapshots.is_empty() {
            output.push_str("- none");
        } else {
            for snapshot in snapshots {
                output.push_str(&format!(
                    "- {} | {} | files={}\n",
                    snapshot.id,
                    snapshot.summary,
                    snapshot.files.len()
                ));
            }
        }
        self.push_system_message(output.trim_end().to_string());
        self.status = "Workspace diff shown".to_string();
        Ok(())
    }

    fn restore_snapshot(&mut self, id: Option<&str>) -> Result<()> {
        let snapshot = self.tool_context.restore_workspace_snapshot(id)?;
        self.push_system_message(format!(
            "Workspace restored\n\nSnapshot: {}\nSummary : {}\nFiles   : {}",
            snapshot.id,
            snapshot.summary,
            snapshot.files.len()
        ));
        self.status = format!("Restored {}", snapshot.id);
        Ok(())
    }

    fn spawn_background_task(
        &mut self,
        id: String,
        prompt: String,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<()> {
        let mode = self.mode;
        let config = self.config.clone();
        let request_messages = self.task_request_messages(prompt.clone(), mode)?;
        let tool_context = self.tool_context.clone();
        let tool_registry = self.tool_registry.clone();

        let task_id = id.clone();
        let task_handle = tokio::spawn(async move {
            let client = match MimoClient::new(&config) {
                Ok(client) => client,
                Err(error) => {
                    let _ = event_tx.send(AppEvent::TaskFinished {
                        id: task_id,
                        result: Err(error.to_string()),
                    });
                    return;
                }
            };
            let routed_model = client.resolve_model(&request_messages);
            let _ = event_tx.send(AppEvent::TaskStarted {
                id: task_id.clone(),
                routed_model,
            });
            let result = agent::run_agent_turn(
                &client,
                request_messages,
                &tool_registry,
                &tool_context,
                |delta| {
                    event_tx
                        .send(AppEvent::TaskOutputDelta {
                            id: task_id.clone(),
                            delta: delta.to_string(),
                        })
                        .context("TUI closed")?;
                    Ok(())
                },
                |status| {
                    let line = background_task_status_line(&status);
                    event_tx
                        .send(AppEvent::TaskProgress {
                            id: task_id.clone(),
                            line,
                        })
                        .context("TUI closed")?;
                    Ok(())
                },
                |invocation| {
                    let allow_prompt_tools = matches!(mode, AppMode::Agent | AppMode::Yolo);
                    async move {
                        match invocation.approval_requirement {
                            crate::tools::ApprovalRequirement::Auto => Ok(true),
                            crate::tools::ApprovalRequirement::Prompt => Ok(allow_prompt_tools),
                        }
                    }
                },
            )
            .await
            .map_err(|error| error.to_string());
            let _ = event_tx.send(AppEvent::TaskFinished {
                id: task_id,
                result,
            });
        });
        self.task_handles.insert(id, task_handle);
        Ok(())
    }

    fn clear_conversation(&mut self) {
        if self.streaming {
            self.status = "Cannot clear while MiMo is responding".to_string();
            return;
        }

        self.messages.clear();
        self.assistant_index = None;
        self.last_prompt = None;
        self.scroll = 0;
        self.tool_runtime.clear_transient();
        self.status = "Conversation cleared".to_string();
    }

    fn clear_current_draft(&mut self) {
        let draft = self.input.trim().to_string();
        if !draft.is_empty() {
            remember_draft(&mut self.draft_history, &draft);
        }
        self.input.clear();
        self.status = "Draft cleared".to_string();
    }

    fn stash_current_draft(&mut self) {
        let draft = self.input.trim().to_string();
        if draft.is_empty() {
            self.status = "Nothing to stash".to_string();
            return;
        }
        remember_draft(&mut self.draft_stash, &draft);
        self.input.clear();
        self.status = "Draft stashed".to_string();
    }

    fn push_system_message(&mut self, content: String) {
        self.messages.push(ChatMessage::system(content));
        self.scroll_to_bottom();
    }

    fn request_messages(&self) -> Result<Vec<ChatMessage>> {
        let mut messages = self.base_request_messages(self.mode)?;
        if !self.plan_items.is_empty() {
            messages.push(ChatMessage::system(self.plan_summary()));
        }
        messages.extend(attachments::attachment_messages(&self.attachments)?);
        messages.extend(self.messages.clone());
        Ok(messages)
    }

    fn task_request_messages(&self, prompt: String, mode: AppMode) -> Result<Vec<ChatMessage>> {
        let mut messages = self.base_request_messages(mode)?;
        messages.push(ChatMessage::system(format!(
            "Background task request\n\nRun this task independently from the visible transcript. Summarize progress through tool actions when useful and finish with a direct final answer.\n\nTask: {prompt}"
        )));
        messages.push(ChatMessage::user(prompt));
        Ok(messages)
    }

    fn base_request_messages(&self, mode: AppMode) -> Result<Vec<ChatMessage>> {
        let memory_notes = memory_store::load_notes(&self.config)?;
        let mut messages = Vec::with_capacity(memory_notes.len() + 4);
        messages.push(ChatMessage::system(self.config.system_prompt.clone()));
        if mode == AppMode::Plan {
            messages.push(ChatMessage::system(
                "You are in planning mode. Analyze the project, propose concrete implementation steps, and do not claim that files were modified or commands were executed unless the user explicitly switches to agent or yolo mode.".to_string(),
            ));
        }
        if !memory_notes.is_empty() {
            let mut memory = String::from("User memory\n\n");
            for note in memory_notes {
                memory.push_str("- ");
                memory.push_str(&note);
                memory.push('\n');
            }
            messages.push(ChatMessage::system(memory.trim_end().to_string()));
        }
        Ok(messages)
    }

    fn cancel_active_stream(&mut self) {
        if let Some(stream_task) = self.stream_task.take() {
            stream_task.abort();
        }
        self.streaming = false;
        if let Some(index) = self.assistant_index
            && let Some(message) = self.messages.get_mut(index)
            && message.content.trim().is_empty()
        {
            message.content = "Request cancelled.".to_string();
        }
        self.assistant_index = None;
        self.status = "Generation cancelled".to_string();
        self.scroll_to_bottom();
    }

    fn handle_escape_key(&mut self) {
        if self.streaming {
            self.cancel_active_stream();
        }
    }

    fn handle_approval_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter | KeyCode::Char('y' | 'Y') => {
                if let Some(request) = self.tool_runtime.approve_pending(true) {
                    self.status = format!("Approved: {}", request.summary);
                }
            }
            KeyCode::Esc | KeyCode::Char('n' | 'N') => {
                if let Some(request) = self.tool_runtime.approve_pending(false) {
                    self.status = format!("Denied: {}", request.summary);
                }
            }
            KeyCode::Char('a' | 'A') => {
                self.set_mode(AppMode::Yolo);
                if let Some(request) = self.tool_runtime.approve_pending(true) {
                    self.status =
                        format!("Approved and switched to yolo mode: {}", request.summary);
                }
            }
            KeyCode::Char('r' | 'R') => {
                self.set_mode(AppMode::Plan);
                if let Some(request) = self.tool_runtime.approve_pending(false) {
                    self.status = format!("Denied and switched to plan mode: {}", request.summary);
                }
            }
            KeyCode::Char('p' | 'P') => {
                self.set_mode(AppMode::Agent);
                self.status = "Mode switched to agent".to_string();
            }
            _ => {}
        }
        false
    }

    fn set_approval_mode(&mut self, mode: ApprovalMode) {
        self.tool_runtime.approval_mode = mode;
        if let Ok(mut shared) = self.approval_mode_shared.lock() {
            *shared = mode;
        }
    }

    fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
        self.set_approval_mode(mode_approval_mode(mode));
    }

    fn open_help(&mut self, topic: Option<&str>) {
        self.help.open = true;
        self.help.scroll = 0;
        self.help.filter.set_text(topic.unwrap_or_default());
    }

    fn toggle_mode(&mut self) {
        self.set_mode(next_mode(self.mode));
        self.status = format!("Mode switched to {}", self.mode);
    }

    fn open_model_picker(&mut self, event_tx: UnboundedSender<AppEvent>) {
        self.refresh_model_picker_catalog();
        self.model_picker.scroll = 0;
        self.model_picker.open = true;
        self.refresh_models_from_api(event_tx);
    }

    fn close_model_picker(&mut self) {
        self.model_picker = ModelPickerState::default();
        self.status = "Model picker closed".to_string();
    }

    fn open_session_picker(&mut self) -> Result<()> {
        let entries = session_store::list_sessions(&self.config)?;
        if entries.is_empty() {
            self.status = "No saved sessions yet".to_string();
            return Ok(());
        }
        self.session_picker.open = true;
        self.session_picker.entries = entries;
        self.session_picker.selected = 0;
        self.session_picker.scroll = 0;
        self.status = "Session picker opened".to_string();
        Ok(())
    }

    fn open_command_palette(&mut self) {
        self.command_palette = CommandPaletteState::default();
        self.command_palette.open = true;
        self.status = "Command palette opened".to_string();
    }

    fn open_draft_browser(&mut self, kind: DraftBrowserKind) {
        let items = match kind {
            DraftBrowserKind::History => &self.draft_history,
            DraftBrowserKind::Stash => &self.draft_stash,
        };
        if items.is_empty() {
            self.status = match kind {
                DraftBrowserKind::History => "No cleared drafts yet".to_string(),
                DraftBrowserKind::Stash => "No stashed drafts yet".to_string(),
            };
            return;
        }
        self.draft_browser.open = true;
        self.draft_browser.kind = kind;
        self.draft_browser.selected = 0;
        self.draft_browser.scroll = 0;
        self.status = match kind {
            DraftBrowserKind::History => "Draft history opened".to_string(),
            DraftBrowserKind::Stash => "Draft stash opened".to_string(),
        };
    }

    fn open_last_message_pager(&mut self) {
        let Some(message) = self.messages.last() else {
            self.status = "No message to open in the pager".to_string();
            return;
        };
        let role = match message.role {
            Role::System => "System",
            Role::User => "You",
            Role::Assistant => "MiMo",
            Role::Tool => "Tool",
        };
        self.message_pager.open = true;
        self.message_pager.title = format!("Last message — {role}");
        self.message_pager.lines = markdown::render_markdown_lines(&message.content);
        self.message_pager.scroll = 0;
        self.status = "Opened last message pager".to_string();
    }

    fn apply_selected_model(&mut self) -> Result<()> {
        let Some(model) = self
            .model_picker
            .models
            .get(self.model_picker.selected)
            .cloned()
        else {
            self.close_model_picker();
            return Ok(());
        };
        self.config.set_model(model.clone())?;
        self.model_picker = ModelPickerState::default();
        self.status = format!("Model switched to {model}");
        Ok(())
    }

    fn apply_selected_session(&mut self) -> Result<()> {
        let Some(entry) = self
            .session_picker
            .entries
            .get(self.session_picker.selected)
            .cloned()
        else {
            self.session_picker = SessionPickerState::default();
            return Ok(());
        };
        let (session, _) =
            session_store::load_session(&self.config, Some(&entry.path.display().to_string()))?;
        self.apply_loaded_session(session, entry.path.display().to_string());
        Ok(())
    }

    fn apply_loaded_session(&mut self, session: session_store::SavedSession, source: String) {
        self.messages = session.messages;
        self.set_mode(session.mode);
        self.config.model = session.model;
        self.plan_items = session.plan_items;
        self.attachments = session.attachments;
        self.assistant_index = None;
        self.tool_runtime.clear_transient();
        self.scroll_to_bottom();
        self.last_prompt = self.last_user_prompt();
        self.session_picker = SessionPickerState::default();
        self.status = format!("Loaded session from {source}");
    }

    fn apply_selected_draft(&mut self) {
        let Some(draft) = self
            .draft_browser_items()
            .get(self.draft_browser.selected)
            .cloned()
        else {
            self.draft_browser = DraftBrowserState::default();
            return;
        };
        self.input.set_text(draft);
        self.draft_browser = DraftBrowserState::default();
        self.status = "Draft restored".to_string();
    }

    fn apply_palette_action(
        &mut self,
        action: command_palette::PaletteAction,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<()> {
        match action {
            command_palette::PaletteAction::InsertCommand(command) => {
                self.input.set_text(command.clone());
                self.status = format!("Inserted {command}");
            }
            command_palette::PaletteAction::OpenHelp => self.open_help(None),
            command_palette::PaletteAction::OpenModels => self.open_model_picker(event_tx),
            command_palette::PaletteAction::OpenSessions => self.open_session_picker()?,
            command_palette::PaletteAction::ShowStatus => {
                self.push_system_message(self.status_summary()?);
                self.status = "Status summary shown".to_string();
            }
            command_palette::PaletteAction::ShowLastMessage => self.open_last_message_pager(),
            command_palette::PaletteAction::BrowseDraftHistory => {
                self.open_draft_browser(DraftBrowserKind::History)
            }
            command_palette::PaletteAction::BrowseDraftStash => {
                self.open_draft_browser(DraftBrowserKind::Stash)
            }
            command_palette::PaletteAction::SwitchMode(mode) => {
                self.set_mode(mode);
                self.status = format!("Mode switched to {}", self.mode);
            }
        }
        Ok(())
    }

    fn refresh_model_picker_catalog(&mut self) {
        let selected_model = self
            .model_picker
            .models
            .get(self.model_picker.selected)
            .cloned();
        self.model_picker.models = model_catalog(&self.config.model, &self.discovered_models);
        self.model_picker.selected = selected_model
            .as_deref()
            .and_then(|model| {
                self.model_picker
                    .models
                    .iter()
                    .position(|candidate| candidate == model)
            })
            .or_else(|| {
                self.model_picker
                    .models
                    .iter()
                    .position(|model| model == &self.config.model)
            })
            .unwrap_or_default();
    }

    fn refresh_models_from_api(&mut self, event_tx: UnboundedSender<AppEvent>) {
        if let Some(task) = self.model_load_task.take() {
            task.abort();
        }

        let client = match MimoClient::new(&self.config) {
            Ok(client) => client,
            Err(_) => {
                self.model_picker.loading = false;
                self.status =
                    "Model picker opened with local suggestions (API key missing)".to_string();
                return;
            }
        };

        self.model_picker.loading = true;
        self.status = "Loading latest models from MiMo API...".to_string();
        let task = tokio::spawn(async move {
            let result = client
                .list_models()
                .await
                .map_err(|error| error.to_string());
            let _ = event_tx.send(AppEvent::ModelsLoaded(result));
        });
        self.model_load_task = Some(task);
    }

    fn input_height(&self, total_height: u16) -> u16 {
        let draft_height = self.input.line_count().clamp(1, 6) as u16 + 2;
        let attachments_height = if self.attachments.is_empty() { 0 } else { 3 };
        (draft_height + attachments_height)
            .min(total_height.saturating_sub(4))
            .max(3)
    }

    fn message_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for message in &self.messages {
            let (label, color) = match message.role {
                Role::System => ("System", Color::DarkGray),
                Role::User => ("You", Color::Green),
                Role::Assistant => ("MiMo", Color::Cyan),
                Role::Tool => ("Tool", Color::Yellow),
            };

            lines.push(Line::styled(
                format!("{label}:"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));

            if message.content.is_empty() {
                lines.push(Line::styled("  ...", Style::default().fg(Color::DarkGray)));
            } else {
                lines.extend(markdown::render_markdown_lines(&message.content));
            }
            lines.push(Line::raw(""));
        }
        lines
    }

    fn total_conversation_lines(&self) -> usize {
        if self.messages.is_empty() {
            3
        } else {
            self.message_lines().len()
        }
    }

    fn max_scroll(&self) -> u16 {
        self.total_conversation_lines()
            .saturating_sub(self.conversation_view_height as usize)
            .try_into()
            .unwrap_or(u16::MAX)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.max_scroll());
    }

    fn scroll_by(&mut self, amount: i32) {
        let max_scroll = self.max_scroll() as i32;
        self.scroll = (self.scroll as i32 + amount).clamp(0, max_scroll) as u16;
    }

    fn scroll_page_up(&mut self) {
        self.scroll_by(-(self.conversation_view_height as i32));
    }

    fn scroll_page_down(&mut self) {
        self.scroll_by(self.conversation_view_height as i32);
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    fn config_summary(&self) -> String {
        let memory_notes = memory_store::load_notes(&self.config)
            .map(|notes| notes.len())
            .unwrap_or_default();
        format!(
            "Configuration\n\nConfig file : {}\nBase URL    : {}\nModel       : {}\nTemperature : {}\nAPI key     : {}\nMode        : {}\nApprovals   : {}\nAttachments : {}\nPlan items  : {}\nMemory notes: {}\nTasks file  : {}\nTasks saved : {}",
            self.config.config_path.display(),
            self.config.base_url,
            self.config.model,
            self.config.temperature,
            self.config.masked_api_key(),
            self.mode,
            approval_mode_label(self.tool_runtime.approval_mode),
            self.attachments.len(),
            self.plan_items.len(),
            memory_notes,
            task_store::tasks_path(&self.config).display(),
            self.tasks.len(),
        )
    }

    fn status_summary(&self) -> Result<String> {
        let session_count = session_store::list_sessions(&self.config)?.len();
        let memory_notes = memory_store::load_notes(&self.config)?.len();
        let request_chars = self
            .request_messages()?
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>();
        Ok(format!(
            "Status\n\nWorkspace    : {}\nMode         : {}\nApprovals    : {}\nModel        : {}\nStreaming    : {}\nMessages     : {}\nSaved files  : {}\nAPI key      : {}\nAttachments  : {}\nDraft stash  : {}\nPlan items   : {}\nMemory notes : {}\nRequest chars: {}\nShell jobs   : {}\nTasks        : {}\nTools        : {}",
            self.tool_context.workspace_root.display(),
            self.mode,
            approval_mode_label(self.tool_runtime.approval_mode),
            self.config.model,
            if self.streaming { "yes" } else { "no" },
            self.messages.len(),
            session_count,
            self.config.masked_api_key(),
            self.attachments.len(),
            self.draft_stash.len(),
            self.plan_items.len(),
            memory_notes,
            request_chars,
            self.active_shell_jobs(),
            self.tasks.len(),
            self.tool_runtime.summary(),
        ))
    }

    fn plan_summary(&self) -> String {
        if self.plan_items.is_empty() {
            return "Plan checklist\n\nNo plan items yet.".to_string();
        }

        let mut output = String::from("Plan checklist\n\n");
        for (index, item) in self.plan_items.iter().enumerate() {
            let marker = if item.done { "x" } else { " " };
            output.push_str(&format!("{}. [{}] {}\n", index + 1, marker, item.text));
        }
        output
    }

    fn recall_matches(&self, query: &str) -> Result<String> {
        let query = query.trim();
        if query.is_empty() {
            bail!("recall query cannot be empty");
        }
        let query_lower = query.to_ascii_lowercase();
        let mut output = format!("Recall results for \"{query}\"\n\n");

        let memory_matches = memory_store::search_memory(&self.config, query, 10)?;
        let has_memory_matches = !memory_matches.is_empty();
        if has_memory_matches {
            output.push_str("Memory notes\n");
            for entry in &memory_matches {
                output.push_str("- ");
                output.push_str(entry);
                output.push('\n');
            }
            output.push('\n');
        }

        let transcript_matches = self
            .messages
            .iter()
            .filter(|message| message.content.to_ascii_lowercase().contains(&query_lower))
            .take(10)
            .collect::<Vec<_>>();
        let has_transcript_matches = !transcript_matches.is_empty();
        if has_transcript_matches {
            output.push_str("Transcript matches\n");
            for message in transcript_matches {
                let role = match message.role {
                    Role::System => "System",
                    Role::User => "You",
                    Role::Assistant => "MiMo",
                    Role::Tool => "Tool",
                };
                output.push_str("- ");
                output.push_str(role);
                output.push_str(": ");
                output.push_str(&truncate_for_summary(&message.content, 180));
                output.push('\n');
            }
        }

        if !has_memory_matches && !has_transcript_matches {
            output.push_str("No memory or transcript matches found.");
        }

        Ok(output.trim_end().to_string())
    }

    fn compact_conversation(&mut self) {
        if self.streaming {
            self.status = "Cannot compact while MiMo is responding".to_string();
            return;
        }
        const KEEP_RECENT_MESSAGES: usize = 6;
        if self.messages.len() <= KEEP_RECENT_MESSAGES {
            self.status = "Conversation is already compact".to_string();
            return;
        }

        let split_index = self.messages.len() - KEEP_RECENT_MESSAGES;
        let summary = compacted_summary(&self.messages[..split_index]);
        let mut recent = self.messages.split_off(split_index);
        self.messages = vec![ChatMessage::system(summary)];
        self.messages.append(&mut recent);
        self.assistant_index = None;
        self.scroll_to_bottom();
        self.status = "Compacted older conversation into a summary".to_string();
    }

    fn persist_tasks(&mut self) -> Result<()> {
        task_store::sort_tasks(&mut self.tasks);
        task_store::save_tasks(&self.config, &self.tasks)?;
        Ok(())
    }

    fn find_task_mut(&mut self, id: &str) -> Option<&mut task_store::SavedTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    fn active_shell_jobs(&self) -> usize {
        self.tool_context
            .shell_manager
            .lock()
            .ok()
            .and_then(|mut manager| manager.list_jobs().ok())
            .map(|jobs| {
                jobs.into_iter()
                    .filter(|job| job.status == crate::tools::ShellStatus::Running)
                    .count()
            })
            .unwrap_or(0)
    }

    fn draft_browser_items(&self) -> &Vec<String> {
        match self.draft_browser.kind {
            DraftBrowserKind::History => &self.draft_history,
            DraftBrowserKind::Stash => &self.draft_stash,
        }
    }

    fn draft_browser_title(&self) -> &'static str {
        match self.draft_browser.kind {
            DraftBrowserKind::History => "Draft history",
            DraftBrowserKind::Stash => "Draft stash",
        }
    }

    fn last_user_prompt(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find_map(|message| matches!(message.role, Role::User).then(|| message.content.clone()))
    }

    fn slash_menu_entries(&self) -> Vec<slash_menu::SlashMenuEntry> {
        slash_menu::visible_entries(self.input.trim())
    }

    fn slash_menu_visible(&self) -> bool {
        slash_menu::is_active(self.input.trim()) && !self.slash_menu_entries().is_empty()
    }

    fn clamp_slash_menu_selection(&mut self) {
        let last = self.slash_menu_entries().len().saturating_sub(1);
        self.slash_menu_selected = self.slash_menu_selected.min(last);
    }
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn opens_help(key: KeyEvent, input_is_empty: bool) -> bool {
    matches!(key.code, KeyCode::F(1)) || (input_is_empty && matches!(key.code, KeyCode::Char('?')))
}

fn opens_palette(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('k' | 'K') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn opens_session_picker(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn opens_draft_history(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('r' | 'R') if key.modifiers.contains(KeyModifiers::ALT))
}

fn stashes_draft(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('s' | 'S') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn opens_last_message_pager(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('l' | 'L') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn toggles_mode_shortcut(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::F(2))
        || (matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
            && key.modifiers.contains(KeyModifiers::CONTROL))
}

fn inserts_newline(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::SHIFT)
        || matches!(key.code, KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT))
}

fn closes_overlay(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Esc | KeyCode::F(1))
        || matches!(key.code, KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn is_quit_key(key: KeyEvent, input_is_empty: bool) -> bool {
    matches!(key.code, KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL))
        || (input_is_empty
            && matches!(key.code, KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL)))
}

fn model_catalog(current_model: &str, discovered_models: &[String]) -> Vec<String> {
    let base = if discovered_models.is_empty() {
        known_mimo_models(current_model)
    } else {
        discovered_models.to_vec()
    };

    let mut models = Vec::new();
    for model in base.into_iter().chain(known_mimo_models(current_model)) {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    }

    models
}

fn normalize_remote_models(models: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if !model.is_empty() && !normalized.iter().any(|existing| existing == model) {
            normalized.push(model.to_string());
        }
    }
    normalized
}

fn clamp_scroll(line_count: usize, height: u16, scroll: u16) -> u16 {
    line_count
        .saturating_sub(height.saturating_sub(2).max(1) as usize)
        .try_into()
        .unwrap_or(u16::MAX)
        .min(scroll)
}

fn adjust_selection_scroll(selected: usize, scroll: u16, visible_lines: usize) -> u16 {
    let mut scroll = scroll;
    if selected < scroll as usize {
        scroll = selected as u16;
    } else if selected >= scroll as usize + visible_lines {
        scroll = (selected + 1 - visible_lines) as u16;
    }
    scroll
}

fn set_filter_cursor(frame: &mut Frame, area: Rect, input: &InputBuffer) {
    let visible_lines = area.height.saturating_sub(2).max(1) as usize;
    let (cursor_line, cursor_col) = input.cursor_line_col();
    let cursor_scroll = cursor_line.saturating_sub(visible_lines.saturating_sub(1)) as u16;
    let visible_line = cursor_line.saturating_sub(cursor_scroll as usize);
    let cursor_x = area.x + 1 + cursor_col.min(area.width.saturating_sub(2) as usize) as u16;
    let cursor_y = area.y + 1 + visible_line.min(visible_lines.saturating_sub(1)) as u16;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn handle_text_input(input: &mut InputBuffer, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Backspace => {
            input.backspace();
            true
        }
        KeyCode::Delete => {
            input.delete();
            true
        }
        KeyCode::Left => {
            input.move_left();
            false
        }
        KeyCode::Right => {
            input.move_right();
            false
        }
        KeyCode::Char('a' | 'A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.move_to_line_start();
            false
        }
        KeyCode::Char('e' | 'E') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.move_to_line_end();
            false
        }
        KeyCode::Char('u' | 'U') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.clear();
            true
        }
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            input.insert_char(c);
            true
        }
        _ => false,
    }
}

fn remember_draft(bucket: &mut Vec<String>, draft: &str) {
    if draft.is_empty() {
        return;
    }
    if let Some(index) = bucket.iter().position(|entry| entry == draft) {
        bucket.remove(index);
    }
    bucket.insert(0, draft.to_string());
    bucket.truncate(20);
}

fn tool_request_from_invocation(
    invocation: &ToolInvocation,
    status: ToolStatus,
    summary: String,
) -> ToolRequest {
    ToolRequest {
        id: invocation.call_id.clone(),
        name: invocation.name.clone(),
        kind: invocation.kind,
        summary,
        status,
    }
}

fn render_shell_result(result: &crate::tools::ShellResult) -> String {
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

fn background_task_status_line(status: &AgentStatus) -> String {
    match status {
        AgentStatus::ToolRequested(invocation) => format!("Requested {}", invocation.summary),
        AgentStatus::ToolStarted(invocation) => format!("Running {}", invocation.summary),
        AgentStatus::ToolSucceeded(invocation, summary) => {
            format!("Completed {} ({summary})", invocation.summary)
        }
        AgentStatus::ToolFailed(invocation, error) => {
            format!("Failed {} ({error})", invocation.summary)
        }
        AgentStatus::ToolFinished(invocation, approved) => {
            if *approved {
                format!("Finished {}", invocation.summary)
            } else {
                format!("Denied {}", invocation.summary)
            }
        }
    }
}

fn trim_task_log(log: &mut Vec<String>) {
    const MAX_TASK_LOG_LINES: usize = 40;
    if log.len() > MAX_TASK_LOG_LINES {
        let excess = log.len() - MAX_TASK_LOG_LINES;
        log.drain(0..excess);
    }
}

fn approval_mode_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Prompt => "prompt",
        ApprovalMode::ReadOnly => "read-only",
        ApprovalMode::Auto => "auto",
    }
}

fn mode_approval_mode(mode: AppMode) -> ApprovalMode {
    match mode {
        AppMode::Plan => ApprovalMode::ReadOnly,
        AppMode::Agent => ApprovalMode::Prompt,
        AppMode::Yolo => ApprovalMode::Auto,
    }
}

fn next_mode(mode: AppMode) -> AppMode {
    match mode {
        AppMode::Agent => AppMode::Plan,
        AppMode::Plan => AppMode::Yolo,
        AppMode::Yolo => AppMode::Agent,
    }
}

fn compacted_summary(messages: &[ChatMessage]) -> String {
    let mut output = String::from("Conversation summary (compacted)\n\n");
    for message in messages {
        let role = match message.role {
            Role::System => "System",
            Role::User => "You",
            Role::Assistant => "MiMo",
            Role::Tool => "Tool",
        };
        output.push_str("- ");
        output.push_str(role);
        output.push_str(": ");
        output.push_str(&truncate_for_summary(&message.content, 220));
        output.push('\n');
    }
    output.trim_end().to_string()
}

fn truncate_for_summary(content: &str, limit: usize) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut shortened = collapsed.chars().take(limit).collect::<String>();
    if collapsed.chars().count() > limit {
        shortened.push_str("...");
    }
    if shortened.is_empty() {
        "(empty)".to_string()
    } else {
        shortened
    }
}

fn current_workspace_diff(workspace_root: &std::path::Path) -> Result<String> {
    let stat = Command::new("git")
        .current_dir(workspace_root)
        .args(["--no-pager", "diff", "--stat"])
        .output()
        .context("failed to run git diff --stat")?;
    let patch = Command::new("git")
        .current_dir(workspace_root)
        .args(["--no-pager", "diff", "--"])
        .output()
        .context("failed to run git diff")?;
    if !stat.status.success() || !patch.status.success() {
        return Ok("Git diff is unavailable in the current workspace.".to_string());
    }

    let stat_text = String::from_utf8_lossy(&stat.stdout).trim().to_string();
    let patch_text = String::from_utf8_lossy(&patch.stdout).trim().to_string();
    if stat_text.is_empty() && patch_text.is_empty() {
        return Ok(String::new());
    }

    let mut output = String::new();
    if !stat_text.is_empty() {
        output.push_str(&stat_text);
    }
    if !patch_text.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str(&patch_text);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    fn test_app() -> App {
        App::new(AppConfig {
            api_key: None,
            base_url: "https://example.test/v1".to_string(),
            model: "mimo-v2-flash".to_string(),
            temperature: 0.2,
            system_prompt: "test".to_string(),
            config_path: PathBuf::from("config.toml"),
            base_url_source: crate::config::ConfigValueSource::Default,
            model_source: crate::config::ConfigValueSource::Default,
            temperature_source: crate::config::ConfigValueSource::Default,
            system_prompt_source: crate::config::ConfigValueSource::Default,
            api_key_source: crate::config::ConfigValueSource::Default,
        })
    }

    #[test]
    fn help_overlay_opens_from_question_mark_when_input_is_empty() {
        let mut app = test_app();
        assert!(opens_help(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            true
        ));
        app.input.insert_str("x");
        assert!(!opens_help(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            false
        ));
    }

    #[test]
    fn clears_conversation_state() {
        let mut app = test_app();
        app.messages.push(ChatMessage::user("hello"));
        app.messages.push(ChatMessage::assistant("world"));
        app.assistant_index = Some(1);
        app.scroll = 3;

        app.clear_conversation();
        assert!(app.messages.is_empty());
        assert_eq!(app.assistant_index, None);
        assert_eq!(app.scroll, 0);
        assert_eq!(app.last_prompt, None);
    }

    #[test]
    fn status_summary_mentions_mode() {
        let app = test_app();
        let summary = app.status_summary().expect("status summary");
        assert!(summary.contains("Mode"));
        assert!(summary.contains("agent"));
    }

    #[test]
    fn slash_commands_clear_the_draft() {
        let mut app = test_app();
        app.input.insert_str("/status");

        let (event_tx, _event_rx) = unbounded_channel();
        app.handle_slash_command("/status", event_tx)
            .expect("slash command should execute");

        assert!(app.input.is_empty());
    }

    #[test]
    fn models_command_opens_picker() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_slash_command("/models", event_tx)
            .expect("models command should execute");

        assert!(app.model_picker.open);
        assert!(!app.model_picker.models.is_empty());
        assert!(!app.model_picker.loading);
    }

    #[test]
    fn model_picker_applies_selected_model() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.open_model_picker(event_tx);
        app.model_picker.selected = app
            .model_picker
            .models
            .iter()
            .position(|model| model == "mimo-v2.5")
            .expect("model present");

        app.apply_selected_model().expect("model should apply");

        assert_eq!(app.config.model, "mimo-v2.5");
        assert!(!app.model_picker.open);
    }

    #[test]
    fn model_picker_refreshes_when_remote_catalog_arrives() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.open_model_picker(event_tx);

        app.handle_app_event(AppEvent::ModelsLoaded(Ok(vec![
            "mimo-v2.5-pro".to_string(),
            "mimo-v3-coder".to_string(),
        ])));

        assert!(
            app.model_picker
                .models
                .iter()
                .any(|model| model == "mimo-v3-coder")
        );
        assert!(
            app.discovered_models
                .iter()
                .any(|model| model == "mimo-v3-coder")
        );
        assert_eq!(app.status, "Loaded 2 models from MiMo API");
    }

    #[test]
    fn model_picker_keeps_local_fallback_when_refresh_fails() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.open_model_picker(event_tx);
        let fallback = app.model_picker.models.clone();

        app.handle_app_event(AppEvent::ModelsLoaded(Err("boom".to_string())));

        assert_eq!(app.model_picker.models, fallback);
        assert!(app.status.contains("using local suggestions"));
    }

    #[test]
    fn escape_is_not_a_quit_key() {
        assert!(!is_quit_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            true
        ));
        assert!(is_quit_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            false
        ));
    }

    #[test]
    fn last_message_pager_requires_ctrl_l() {
        assert!(!opens_last_message_pager(KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::NONE
        )));
        assert!(opens_last_message_pager(KeyEvent::new(
            KeyCode::Char('l'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn escape_cancels_active_stream() {
        let mut app = test_app();
        app.streaming = true;
        app.messages.push(ChatMessage::assistant(String::new()));
        app.assistant_index = Some(0);

        app.handle_escape_key();

        assert!(!app.streaming);
        assert_eq!(app.assistant_index, None);
        assert_eq!(app.status, "Generation cancelled");
        assert_eq!(app.messages[0].content, "Request cancelled.");
    }

    #[test]
    fn plan_commands_update_checklist() {
        let mut app = test_app();
        app.handle_plan_command(PlanCommand::Add("Ship command palette".to_string()));
        app.handle_plan_command(PlanCommand::Done(1));
        assert_eq!(app.plan_items.len(), 1);
        assert!(app.plan_items[0].done);
    }

    #[test]
    fn ctrl_tab_toggles_mode() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)),
            event_tx.clone(),
        )
        .expect("ctrl+tab should toggle mode");
        assert_eq!(app.mode, AppMode::Plan);

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)),
            event_tx.clone(),
        )
        .expect("ctrl+tab should cycle to yolo");
        assert_eq!(app.mode, AppMode::Yolo);

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::CONTROL)),
            event_tx,
        )
        .expect("ctrl+tab should cycle back to agent");
        assert_eq!(app.mode, AppMode::Agent);
    }

    #[test]
    fn f2_toggles_mode() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(Event::Key(KeyEvent::from(KeyCode::F(2))), event_tx.clone())
            .expect("f2 should toggle mode");
        assert_eq!(app.mode, AppMode::Plan);

        app.handle_terminal_event(Event::Key(KeyEvent::from(KeyCode::F(2))), event_tx.clone())
            .expect("f2 should cycle to yolo");
        assert_eq!(app.mode, AppMode::Yolo);

        app.handle_terminal_event(Event::Key(KeyEvent::from(KeyCode::F(2))), event_tx)
            .expect("f2 should cycle back to agent");
        assert_eq!(app.mode, AppMode::Agent);
    }
}
