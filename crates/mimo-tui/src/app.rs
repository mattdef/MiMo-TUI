use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Command,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use mimo_agent::{self as agent, AgentStatus};
use mimo_client::MimoClient;
use mimo_config::{
    AUTO_MODEL, AppConfig, known_mimo_models, normalize_base_url, normalize_model_name,
};
use mimo_protocol::{ChatMessage, Role};
use mimo_state::{
    AppMode, FileAttachment, PlanItem,
    branch::{ConversationTree, MessageId},
    diagnostics_store, mcp_store, memory_store, session_store, skill_store, task_store,
};
use mimo_tools::{
    ApprovalRequirement, ShellResult, ShellStatus, ToolContext, ToolInvocation, ToolKind,
    ToolRegistry, ToolRegistryBuilder, default_workspace_root,
};
use mimo_tui_core::{
    commands::{
        self, CommandParseError, ConfigCommand, JobsCommand, LspCommand, McpCommand, MemoryCommand,
        ModeName, PlanCommand, ReviewCommand, SkillCommand, SlashCommand, TaskCommand,
    },
    input::InputBuffer,
    markdown,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde_json::to_string_pretty;
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio::task::JoinHandle;

use super::{
    attachments, command_palette, session_picker, slash_menu,
    tooling::{ApprovalMode, ToolRequest, ToolRuntime, ToolStatus},
};

mod command_handlers;

pub enum AppEvent {
    Delta(String),
    Finished(Result<(), String>),
    ModelsLoaded(Result<Vec<String>, String>),
    Status(String),
    DiagnosticsFinished(Result<diagnostics_store::DiagnosticsSnapshot, String>),
    SkillInstalled(Result<skill_store::InstalledSkill, String>),
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

const XIAOMI_ORANGE: Color = Color::Rgb(255, 106, 0);
const SLASH_MENU_PAGE_STEP: i32 = 5;

fn app_mode_for(mode: ModeName) -> AppMode {
    match mode {
        ModeName::Agent => AppMode::Agent,
        ModeName::Plan => AppMode::Plan,
        ModeName::Yolo => AppMode::Yolo,
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

#[derive(Debug, Default)]
struct BranchSelectionState {
    open: bool,
    selected: usize,
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

// Cache the last attachment search and preview so unchanged queries do not rescan the filesystem.
#[derive(Debug, Default)]
struct AttachmentPickerCache {
    search_base_dir: Option<PathBuf>,
    search_raw: Option<String>,
    results: Option<attachments::AttachmentSearchResults>,
    preview_suggestion: Option<attachments::AttachmentSuggestion>,
    preview: Option<attachments::AttachmentPreview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorBrowserKind {
    Tasks,
    Snapshots,
}

#[derive(Debug)]
struct InspectorBrowserState {
    open: bool,
    kind: InspectorBrowserKind,
    selected: usize,
    scroll: u16,
}

impl Default for InspectorBrowserState {
    fn default() -> Self {
        Self {
            open: false,
            kind: InspectorBrowserKind::Tasks,
            selected: 0,
            scroll: 0,
        }
    }
}

pub struct App {
    config: AppConfig,
    messages: Vec<ChatMessage>,
    conversation_tree: ConversationTree,
    tool_context: ToolContext,
    tool_registry: ToolRegistry,
    input: InputBuffer,
    status: String,
    streaming: bool,
    assistant_index: Option<usize>,
    assistant_message_id: Option<MessageId>,
    scroll: u16,
    conversation_view_height: u16,
    input_scroll: u16,
    help: HelpState,
    model_picker: ModelPickerState,
    session_picker: SessionPickerState,
    command_palette: CommandPaletteState,
    branch_selection: BranchSelectionState,
    draft_browser: DraftBrowserState,
    message_pager: MessagePagerState,
    inspector_browser: InspectorBrowserState,
    discovered_models: Vec<String>,
    installed_skills_cache: Vec<skill_store::InstalledSkill>,
    last_prompt: Option<String>,
    mode: AppMode,
    stream_task: Option<JoinHandle<()>>,
    stream_context: Option<ToolContext>,
    model_load_task: Option<JoinHandle<()>>,
    skill_install_task: Option<JoinHandle<()>>,
    draft_history: Vec<String>,
    draft_stash: Vec<String>,
    attachments: Vec<FileAttachment>,
    attachment_picker: attachments::AttachmentPickerState,
    attachment_picker_cache: AttachmentPickerCache,
    active_skills: Vec<String>,
    diagnostics: Option<diagnostics_store::DiagnosticsSnapshot>,
    diagnostics_auto_run: bool,
    plan_items: Vec<PlanItem>,
    tool_runtime: ToolRuntime,
    approval_mode_shared: Arc<Mutex<ApprovalMode>>,
    slash_menu_selected: usize,
    slash_menu_scroll: u16,
    event_tx: Option<UnboundedSender<AppEvent>>,
    mcp_servers_cache: Vec<mcp_store::McpServerConfig>,
    tasks: Vec<task_store::SavedTask>,
    task_contexts: BTreeMap<String, ToolContext>,
    task_handles: BTreeMap<String, JoinHandle<()>>,
    diagnostics_task: Option<JoinHandle<()>>,
}

impl App {
    pub fn new(config: AppConfig) -> Self {
        let tool_context = ToolContext::new(default_workspace_root());
        let tool_registry = ToolRegistryBuilder::new().build_all();
        let mode = AppMode::Agent;
        let approval_mode_shared = Arc::new(Mutex::new(mode_approval_mode(mode)));
        let tasks = task_store::load_tasks(&config).unwrap_or_default();
        let diagnostics = diagnostics_store::load_snapshot(&config).unwrap_or_default();
        let installed_skills_cache =
            skill_store::list_installed_skills(&config).unwrap_or_default();
        let mcp_servers_cache = mcp_store::load_servers(&config).unwrap_or_default();
        let _ = task_store::save_tasks(&config, &tasks);
        let status = if config.api_key.is_some() {
            "Ready".to_string()
        } else {
            "Missing API key: use /config api-key <key> or edit config.toml".to_string()
        };

        Self {
            config,
            messages: Vec::new(),
            conversation_tree: ConversationTree::new(),
            tool_context,
            tool_registry,
            input: InputBuffer::new(),
            status,
            streaming: false,
            assistant_index: None,
            assistant_message_id: None,
            scroll: 0,
            conversation_view_height: 1,
            input_scroll: 0,
            help: HelpState::default(),
            model_picker: ModelPickerState::default(),
            session_picker: SessionPickerState::default(),
            command_palette: CommandPaletteState::default(),
            branch_selection: BranchSelectionState::default(),
            draft_browser: DraftBrowserState::default(),
            message_pager: MessagePagerState::default(),
            inspector_browser: InspectorBrowserState::default(),
            discovered_models: Vec::new(),
            installed_skills_cache,
            last_prompt: None,
            mode,
            stream_task: None,
            stream_context: None,
            model_load_task: None,
            skill_install_task: None,
            draft_history: Vec::new(),
            draft_stash: Vec::new(),
            attachments: Vec::new(),
            attachment_picker: attachments::AttachmentPickerState::default(),
            attachment_picker_cache: AttachmentPickerCache::default(),
            active_skills: Vec::new(),
            diagnostics,
            diagnostics_auto_run: false,
            plan_items: Vec::new(),
            tool_runtime: ToolRuntime {
                approval_mode: mode_approval_mode(mode),
                ..ToolRuntime::default()
            },
            approval_mode_shared,
            slash_menu_selected: 0,
            slash_menu_scroll: 0,
            event_tx: None,
            mcp_servers_cache,
            tasks,
            task_contexts: BTreeMap::new(),
            task_handles: BTreeMap::new(),
            diagnostics_task: None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if self.is_landing_screen() {
            self.conversation_view_height = 1;
            self.scroll = 0;
            self.render_landing_screen(frame, area);
        } else {
            self.render_workspace(frame, area);
        }

        if self.tool_runtime.pending_approval.is_some() {
            self.render_approval_overlay(frame, area);
        } else if self.help.open {
            self.render_help_overlay(frame, area);
        } else if self.command_palette.open {
            self.render_command_palette_overlay(frame, area);
        } else if self.draft_browser.open {
            self.render_draft_browser_overlay(frame, area);
        } else if self.inspector_browser.open {
            self.render_inspector_browser_overlay(frame, area);
        } else if self.session_picker.open {
            self.render_session_picker_overlay(frame, area);
        } else if self.model_picker.open {
            self.render_model_picker_overlay(frame, area);
        } else if self.message_pager.open {
            self.render_message_pager_overlay(frame, area);
        } else if self.attachment_picker.is_visible() {
            self.render_attachment_picker_overlay(frame, area);
        } else if self.slash_menu_visible() {
            self.render_slash_menu_overlay(frame, area);
        }
    }

    fn render_workspace(&mut self, frame: &mut Frame, area: Rect) {
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
    }

    fn render_landing_screen(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(6), Constraint::Length(1)])
            .split(area);
        let popup = landing_popup_rect(chunks[0], 78, 64);
        let status_height = u16::from(self.status != "Ready");
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(self.landing_hero_height()),
                Constraint::Length(2),
                Constraint::Length(self.landing_prompt_height()),
                Constraint::Length(1),
                Constraint::Length(status_height),
                Constraint::Min(0),
            ])
            .split(popup);
        frame.render_widget(
            Paragraph::new(self.landing_hero()).alignment(Alignment::Center),
            sections[0],
        );

        self.render_landing_prompt(frame, sections[2]);
        frame.render_widget(
            Paragraph::new(self.landing_tip())
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray)),
            sections[3],
        );
        if status_height > 0 {
            frame.render_widget(
                Paragraph::new(self.status.as_str())
                    .alignment(Alignment::Center)
                    .style(self.status_style()),
                sections[4],
            );
        }
        self.render_footer(frame, chunks[1]);
    }

    fn is_landing_screen(&self) -> bool {
        self.messages.is_empty()
    }

    fn landing_tip(&self) -> &'static str {
        if self.config.api_key.is_some() {
            "Tip: use /help for commands or @path + Tab to attach workspace context."
        } else {
            "Tip: use /config api-key <key> to save credentials from inside the TUI."
        }
    }

    fn landing_hero_height(&self) -> u16 {
        7
    }

    fn landing_hero(&self) -> Text<'static> {
        Text::from(vec![
            Line::styled("Xiaomi", Style::default().fg(XIAOMI_ORANGE)),
            Line::raw(""),
            Line::styled(
                "█   █  ██  █   █   ███        █████  █ █  ██",
                Style::default().fg(Color::White),
            ),
            Line::styled(
                "██ ██   █  ██ ██  █   █         █    █ █   █",
                Style::default().fg(Color::White),
            ),
            Line::styled(
                "█ █ █   █  █ █ █  █   █   ███   █    █ █   █",
                Style::default().fg(Color::White),
            ),
            Line::styled(
                "█   █   █  █   █  █   █         █    █ █   █",
                Style::default().fg(Color::White),
            ),
            Line::styled(
                "█   █  ███ █   █   ███          █    ███  ███",
                Style::default().fg(Color::White),
            ),
        ])
    }

    fn status_style(&self) -> Style {
        if self.streaming {
            Style::default().fg(Color::Yellow)
        } else if self.config.api_key.is_none() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        }
    }

    pub fn handle_terminal_event(
        &mut self,
        event: Event,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<bool> {
        self.event_tx = Some(event_tx.clone());
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
                if self.inspector_browser.open {
                    return self.handle_inspector_browser_key(key);
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
                if self.branch_selection.open {
                    return self.handle_branch_selection_key(key);
                }

                if self.attachment_picker.is_visible() && self.handle_attachment_picker_key(key)? {
                    return Ok(false);
                }

                if self.handle_slash_menu_key(key) {
                    return Ok(false);
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
                if opens_branch_selection(key) {
                    self.open_branch_selection();
                    return Ok(false);
                }

                if matches!(key.code, KeyCode::Esc) {
                    self.handle_escape_key();
                    return Ok(false);
                }

                if is_quit_key(key, self.input.is_empty()) {
                    return Ok(true);
                }

                let mut refresh_attachment_picker = false;
                match key.code {
                    KeyCode::Enter if inserts_newline(key) => {
                        self.input.insert_char('\n');
                        refresh_attachment_picker = true;
                    }
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
                    KeyCode::Backspace => {
                        self.input.backspace();
                        self.clamp_slash_menu_selection();
                        refresh_attachment_picker = true;
                    }
                    KeyCode::Delete => {
                        self.input.delete();
                        self.clamp_slash_menu_selection();
                        refresh_attachment_picker = true;
                    }
                    KeyCode::Left => {
                        self.input.move_left();
                        refresh_attachment_picker = true;
                    }
                    KeyCode::Right => {
                        self.input.move_right();
                        refresh_attachment_picker = true;
                    }
                    KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll = 0
                    }
                    KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_to_bottom()
                    }
                    KeyCode::Home => {
                        self.input.move_to_line_start();
                        refresh_attachment_picker = true;
                    }
                    KeyCode::End => {
                        self.input.move_to_line_end();
                        refresh_attachment_picker = true;
                    }
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
                        refresh_attachment_picker = true;
                    }
                    KeyCode::Char('e' | 'E') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.move_to_line_end();
                        refresh_attachment_picker = true;
                    }
                    KeyCode::Char('j' | 'J') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.insert_char('\n');
                        refresh_attachment_picker = true;
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
                        refresh_attachment_picker = true;
                    }
                    _ => {}
                }

                if refresh_attachment_picker {
                    self.sync_attachment_picker();
                }
            }
            Event::Paste(text) => {
                self.input.insert_str(&text);
                self.clamp_slash_menu_selection();
                self.sync_attachment_picker();
            }
            _ => {}
        }

        Ok(false)
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Delta(delta) => {
                self.append_streaming_assistant_delta(&delta);
                self.scroll_to_bottom();
            }
            AppEvent::Finished(Ok(())) => {
                self.stream_task = None;
                self.stream_context = None;
                self.streaming = false;
                self.assistant_index = None;
                self.assistant_message_id = None;
                if self.status != "Generation cancelled" {
                    self.status = "Ready".to_string();
                }
                self.scroll_to_bottom();
            }
            AppEvent::Finished(Err(error)) => {
                self.stream_task = None;
                self.stream_context = None;
                self.streaming = false;
                let should_write_error = self
                    .assistant_index
                    .and_then(|index| self.messages.get(index))
                    .is_some_and(|message| message.content.is_empty());
                if should_write_error {
                    self.set_streaming_assistant_content(format!("Request failed: {error}"));
                }
                self.assistant_index = None;
                self.assistant_message_id = None;
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
            AppEvent::DiagnosticsFinished(result) => {
                self.diagnostics_task = None;
                match result {
                    Ok(snapshot) => {
                        self.status = snapshot.summary.clone();
                        self.diagnostics = Some(snapshot);
                    }
                    Err(error) => {
                        self.status = format!("Diagnostics failed: {error}");
                    }
                }
            }
            AppEvent::SkillInstalled(result) => {
                self.skill_install_task = None;
                match result {
                    Ok(skill) => {
                        self.refresh_skill_cache();
                        self.status =
                            format!("Installed skill {} at {}", skill.name, skill.path.display());
                    }
                    Err(error) => {
                        self.status = format!("Skill install failed: {error}");
                    }
                }
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
                let should_refresh =
                    self.diagnostics_auto_run && should_refresh_diagnostics(&request);
                let refresh_name = request.name.clone();
                self.tool_runtime.finish(request);
                self.status = status;
                if should_refresh
                    && let Err(error) =
                        self.start_diagnostics_refresh(format!("after {refresh_name}"))
                {
                    self.status = format!(
                        "{} (auto diagnostics failed to start: {error})",
                        self.status
                    );
                }
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
                self.task_contexts.remove(&id);
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
            Span::styled(&self.status, self.status_style()),
        ]);

        frame.render_widget(
            Paragraph::new(line).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style()),
            ),
            area,
        );
    }

    fn render_main_panel(&self, frame: &mut Frame, area: Rect) {
        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(68), Constraint::Percentage(32)])
            .split(area);
        self.render_conversation(frame, panels[0]);
        self.render_plan_panel(frame, panels[1]);
    }

    fn render_conversation(&self, frame: &mut Frame, area: Rect) {
        let title = if self.branch_selection.open {
            "Select branch point — Enter create | Esc cancel".to_string()
        } else {
            format!(
                "Conversation — {}",
                self.conversation_tree.current_branch_id()
            )
        };
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
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
                )
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
            let hint = if self.mode == AppMode::Plan {
                "Plan mode stays read-only until you switch to agent or yolo mode."
            } else {
                "This checklist stays visible while you work in other modes."
            };
            lines.push(Line::styled(hint, Style::default().fg(Color::DarkGray)));
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
                .block(
                    Block::default()
                        .title("Plan")
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
                )
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
                            .borders(Borders::ALL)
                            .border_style(panel_border_style()),
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
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(panel_border_style());
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let visible_lines = inner.height.max(1) as usize;
        let (cursor_line, cursor_col) = self.sync_input_scroll(visible_lines);
        frame.render_widget(
            Paragraph::new(self.input.as_str()).scroll((self.input_scroll, 0)),
            inner,
        );

        if !self.help.open
            && !self.model_picker.open
            && !self.session_picker.open
            && !self.command_palette.open
            && !self.branch_selection.open
            && !self.draft_browser.open
            && !self.message_pager.open
            && !self.inspector_browser.open
            && self.tool_runtime.pending_approval.is_none()
            && !self.attachment_picker.is_visible()
            && !self.slash_menu_visible()
        {
            self.set_input_cursor(frame, inner, visible_lines, cursor_line, cursor_col);
        }
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(
            Paragraph::new(self.footer_summary())
                .alignment(Alignment::Right)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
    }

    fn footer_summary(&self) -> String {
        if self.branch_selection.open {
            return "Select a message · ↑/↓ move · Enter create · Esc cancel".to_string();
        }

        let summary = match self.footer_context_chars() {
            Ok(used_chars) => {
                let max_chars = estimated_max_context_chars(&self.config.model);
                let used_percent = ((used_chars as f64 / max_chars as f64) * 100.0).min(100.0);
                format!(
                    "{} ({used_percent:.0}%) · F1/? help",
                    format_compact_count(used_chars)
                )
            }
            Err(_) => "context unavailable · F1/? help".to_string(),
        };

        let summary = if self.mode == AppMode::Yolo {
            format!("{summary} · YOLO auto-approves mutating tools")
        } else {
            summary
        };

        format!(
            "{summary} · branch {}/{} · Ctrl+B branch",
            self.conversation_tree.current_branch_id(),
            self.conversation_tree.branch_count()
        )
    }

    fn footer_context_chars(&self) -> Result<usize> {
        let request_chars = self
            .request_messages()?
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>();
        Ok(request_chars + self.input.as_str().chars().count())
    }

    fn render_landing_prompt(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(panel_border_style());
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let attachments_height = u16::from(!self.attachments.is_empty());
        let sections = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(attachments_height),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);
        let draft_area = if attachments_height > 0 {
            frame.render_widget(
                Paragraph::new(attachments::attachment_preview(&self.attachments))
                    .style(Style::default().fg(Color::Yellow)),
                sections[0],
            );
            sections[1]
        } else {
            sections[1]
        };
        let visible_lines = draft_area.height.max(1) as usize;
        let (cursor_line, cursor_col) = self.sync_input_scroll(visible_lines);
        let prompt_text = if self.input.is_empty() {
            Text::from(vec![Line::styled(
                "Ask anything...  \"Fix a TODO in the codebase\"",
                Style::default().fg(Color::DarkGray),
            )])
        } else {
            Text::from(self.input.as_str())
        };
        frame.render_widget(
            Paragraph::new(prompt_text)
                .scroll((self.input_scroll, 0))
                .wrap(Wrap { trim: false }),
            draft_area,
        );

        let meta = Line::from(vec![
            Span::styled(self.mode.to_string(), Style::default().fg(Color::Blue)),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(&self.config.model, Style::default().fg(Color::Magenta)),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled("Ctrl+K commands", Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(meta), sections[2]);

        if !self.help.open
            && !self.model_picker.open
            && !self.session_picker.open
            && !self.command_palette.open
            && !self.draft_browser.open
            && !self.message_pager.open
            && !self.inspector_browser.open
            && self.tool_runtime.pending_approval.is_none()
            && !self.attachment_picker.is_visible()
            && !self.slash_menu_visible()
        {
            self.set_input_cursor(frame, draft_area, visible_lines, cursor_line, cursor_col);
        }
    }

    fn landing_prompt_height(&self) -> u16 {
        let draft_height = self.input.line_count().clamp(1, 4) as u16;
        let attachments_height = u16::from(!self.attachments.is_empty());
        (draft_height + attachments_height + 3).max(5)
    }

    fn sync_input_scroll(&mut self, visible_lines: usize) -> (usize, usize) {
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
        (cursor_line, cursor_col)
    }

    fn set_input_cursor(
        &self,
        frame: &mut Frame,
        area: Rect,
        visible_lines: usize,
        cursor_line: usize,
        cursor_col: usize,
    ) {
        let visible_line = cursor_line.saturating_sub(self.input_scroll as usize);
        let cursor_x = area.x + cursor_col.min(area.width.saturating_sub(1) as usize) as u16;
        let cursor_y = area.y + visible_line.min(visible_lines.saturating_sub(1)) as u16;
        frame.set_cursor_position((cursor_x, cursor_y));
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
                .border_style(panel_border_style()),
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
                    .borders(Borders::ALL)
                    .border_style(panel_border_style()),
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
                .border_style(panel_border_style()),
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
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
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
                .border_style(panel_border_style()),
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
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
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
                .border_style(panel_border_style()),
            popup,
        );

        self.render_filter_box(frame, inner[0], &self.command_palette.filter, "Filter");
        let entries = self.command_palette_entries();
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
                .block(
                    Block::default()
                        .title("Actions")
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
                )
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
                .border_style(panel_border_style()),
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
                .block(
                    Block::default()
                        .title("Drafts")
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
                )
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

    fn render_inspector_browser_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 74, 58);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)])
            .split(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Block::default()
                .title(self.inspector_browser_title())
                .borders(Borders::ALL)
                .border_style(panel_border_style()),
            popup,
        );

        self.inspector_browser.scroll = adjust_selection_scroll(
            self.inspector_browser.selected,
            self.inspector_browser.scroll,
            inner[0].height.saturating_sub(2).max(1) as usize,
        );
        let lines = self
            .inspector_browser_items()
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let prefix = if index == self.inspector_browser.selected {
                    "> "
                } else {
                    "  "
                };
                let style = if index == self.inspector_browser.selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::styled(format!("{prefix}{item}"), style)
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(Text::from(lines))
                .block(
                    Block::default()
                        .title("Entries")
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
                )
                .wrap(Wrap { trim: false })
                .scroll((self.inspector_browser.scroll, 0)),
            inner[0],
        );
        let footer = match self.inspector_browser.kind {
            InspectorBrowserKind::Tasks => "Up/Down move | Enter inspect | Esc cancel",
            InspectorBrowserKind::Snapshots => {
                "Up/Down move | Enter inspect | r restore selected | Esc cancel"
            }
        };
        frame.render_widget(
            Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
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
                        .border_style(panel_border_style()),
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
                .border_style(panel_border_style()),
            popup,
        );

        let Some(pending) = self.tool_runtime.pending_approval.as_ref() else {
            return;
        };

        let kind = match pending.request.kind {
            ToolKind::FileRead => "File read",
            ToolKind::FileWrite => "File write",
            ToolKind::Search => "Search",
            ToolKind::Git => "Git",
            ToolKind::Network => "Network",
            ToolKind::Project => "Project",
            ToolKind::Shell => "Shell",
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
                        .title("Pending request")
                        .border_style(panel_border_style()),
                )
                .wrap(Wrap { trim: false }),
            centered_rect(popup, 94, 80),
        );
    }

    fn render_slash_menu_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 72, 34);
        let block = Block::default()
            .title("Slash menu")
            .borders(Borders::ALL)
            .border_style(panel_border_style());
        let inner = block.inner(popup);
        frame.render_widget(Clear, popup);
        frame.render_widget(block, popup);
        let entries = self.slash_menu_entries();
        let last = entries.len().saturating_sub(1);
        self.slash_menu_selected = self.slash_menu_selected.min(last);
        self.slash_menu_scroll = adjust_selection_scroll(
            self.slash_menu_selected,
            self.slash_menu_scroll,
            inner.height.max(1) as usize,
        );
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
        // Keep each command on one row so the selection scroll stays aligned.
        frame.render_widget(
            Paragraph::new(Text::from(lines)).scroll((self.slash_menu_scroll, 0)),
            inner,
        );
    }

    fn render_attachment_picker_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 92, 80);
        frame.render_widget(Clear, popup);

        let split_vertical = popup.width < 84 || popup.height < 14;
        let panels = if split_vertical {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
                .split(popup)
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(popup)
        };

        let suggestion_title = format!(
            "Matches ({}) · ↑/↓ move · Tab/Enter attach · Esc cancel",
            self.attachment_picker.suggestions.len()
        );
        let suggestion_block = Block::default()
            .title(suggestion_title)
            .borders(Borders::ALL)
            .border_style(panel_border_style());
        let suggestion_inner = suggestion_block.inner(panels[0]);
        frame.render_widget(suggestion_block, panels[0]);

        let suggestion_body_height = suggestion_inner.height.max(1) as usize;
        self.attachment_picker.scroll = adjust_selection_scroll(
            self.attachment_picker.selected,
            self.attachment_picker.scroll,
            suggestion_body_height,
        );
        let mut suggestion_lines = self
            .attachment_picker
            .suggestions
            .iter()
            .enumerate()
            .map(|(index, suggestion)| {
                let prefix = if index == self.attachment_picker.selected {
                    "> "
                } else {
                    "  "
                };
                let style = if index == self.attachment_picker.selected {
                    Style::default()
                        .fg(Color::Blue)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::styled(
                    format!(
                        "{prefix}[{}] {}",
                        suggestion.kind.label(),
                        suggestion.display_path
                    ),
                    style,
                )
            })
            .collect::<Vec<_>>();
        if let Some(status) = &self.attachment_picker.status {
            if !suggestion_lines.is_empty() {
                suggestion_lines.push(Line::raw(""));
            }
            suggestion_lines.push(Line::styled(
                status.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        if suggestion_lines.is_empty() {
            suggestion_lines.push(Line::styled(
                "No attachment matches",
                Style::default().fg(Color::DarkGray),
            ));
        }
        frame.render_widget(
            Paragraph::new(Text::from(suggestion_lines))
                .wrap(Wrap { trim: false })
                .scroll((self.attachment_picker.scroll, 0)),
            suggestion_inner,
        );

        let preview_block = Block::default()
            .title("Preview")
            .borders(Borders::ALL)
            .border_style(panel_border_style());
        let preview_inner = preview_block.inner(panels[1]);
        frame.render_widget(preview_block, panels[1]);

        let mut preview_lines = Vec::new();
        if let Some(preview) = &self.attachment_picker.preview {
            preview_lines.extend(
                preview
                    .metadata
                    .iter()
                    .map(|line| Line::styled(line.clone(), Style::default().fg(Color::Cyan))),
            );
            if !preview.metadata.is_empty() && (!preview.lines.is_empty() || preview.note.is_some())
            {
                preview_lines.push(Line::raw(""));
            }
            preview_lines.extend(preview.lines.iter().map(|line| Line::raw(line.clone())));
            if let Some(note) = &preview.note {
                if !preview.lines.is_empty() {
                    preview_lines.push(Line::raw(""));
                }
                preview_lines.push(Line::styled(
                    note.clone(),
                    Style::default().fg(Color::Yellow),
                ));
            }
        } else if let Some(status) = &self.attachment_picker.status {
            preview_lines.push(Line::styled(
                status.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            preview_lines.push(Line::styled(
                "Select a suggestion to preview it",
                Style::default().fg(Color::DarkGray),
            ));
        }

        frame.render_widget(
            Paragraph::new(Text::from(preview_lines)).wrap(Wrap { trim: false }),
            preview_inner,
        );
    }

    fn render_filter_box(&self, frame: &mut Frame, area: Rect, input: &InputBuffer, title: &str) {
        let filter_visible_lines = area.height.saturating_sub(2).max(1) as usize;
        let (filter_line, _) = input.cursor_line_col();
        let filter_scroll =
            filter_line.saturating_sub(filter_visible_lines.saturating_sub(1)) as u16;

        frame.render_widget(
            Paragraph::new(input.as_str())
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(panel_border_style()),
                )
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
                    self.installed_skills(),
                    &self.active_skills,
                    self.mcp_servers(),
                )
                .len()
                .saturating_sub(1);
                self.command_palette.selected = (self.command_palette.selected + 1).min(last);
            }
            KeyCode::Enter => {
                let entries = self.command_palette_entries();
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

    fn handle_inspector_browser_key(&mut self, key: KeyEvent) -> Result<bool> {
        if closes_overlay(key) {
            self.inspector_browser = InspectorBrowserState::default();
            self.status = "Inspector closed".to_string();
            return Ok(false);
        }

        match key.code {
            KeyCode::Up => {
                self.inspector_browser.selected = self.inspector_browser.selected.saturating_sub(1)
            }
            KeyCode::Down => {
                let last = self.inspector_browser_items().len().saturating_sub(1);
                self.inspector_browser.selected = (self.inspector_browser.selected + 1).min(last);
            }
            KeyCode::Enter => self.inspect_selected_browser_entry()?,
            KeyCode::Char('r' | 'R')
                if self.inspector_browser.kind == InspectorBrowserKind::Snapshots =>
            {
                self.restore_selected_snapshot_from_browser()?;
            }
            _ => {}
        }
        Ok(false)
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
        if self.model_picker.models.is_empty() {
            return Ok(false);
        }
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

    fn clear_attachment_picker(&mut self) {
        self.attachment_picker = attachments::AttachmentPickerState::default();
    }

    fn dismiss_attachment_picker(&mut self) {
        if let Some(query) = self.attachment_picker.query.clone() {
            self.attachment_picker.dismissed_query = Some(query);
            self.attachment_picker.open = false;
        }
    }

    fn refresh_attachment_picker_preview(&mut self) {
        let selected = self.attachment_picker.selected_suggestion().cloned();
        self.attachment_picker.preview =
            selected.map(|suggestion| self.attachment_preview_for(&suggestion));
    }

    fn attachment_suggestions_for(
        &mut self,
        query: &attachments::AttachmentQuery,
    ) -> attachments::AttachmentSearchResults {
        let workspace_root = self.tool_context.workspace_root.clone();
        let cache_hit = self.attachment_picker_cache.search_base_dir.as_ref()
            == Some(&workspace_root)
            && self.attachment_picker_cache.search_raw.as_deref() == Some(query.raw.as_str());

        if cache_hit && let Some(results) = &self.attachment_picker_cache.results {
            return results.clone();
        }

        let results = attachments::discover_attachment_suggestions(query, workspace_root.as_path());
        self.attachment_picker_cache.search_base_dir = Some(workspace_root);
        self.attachment_picker_cache.search_raw = Some(query.raw.clone());
        self.attachment_picker_cache.results = Some(results.clone());
        results
    }

    fn attachment_preview_for(
        &mut self,
        suggestion: &attachments::AttachmentSuggestion,
    ) -> attachments::AttachmentPreview {
        if self.attachment_picker_cache.preview_suggestion.as_ref() == Some(suggestion)
            && let Some(preview) = &self.attachment_picker_cache.preview
        {
            return preview.clone();
        }

        let preview = attachments::build_attachment_preview(suggestion);
        self.attachment_picker_cache.preview_suggestion = Some(suggestion.clone());
        self.attachment_picker_cache.preview = Some(preview.clone());
        preview
    }

    fn sync_attachment_picker(&mut self) {
        let Some(query) = attachments::current_attachment_query(&self.input) else {
            self.clear_attachment_picker();
            return;
        };

        if self.attachment_picker.dismissed_query.as_ref() == Some(&query) {
            self.attachment_picker.query = Some(query);
            self.attachment_picker.open = false;
            return;
        }

        let query_changed = self.attachment_picker.query.as_ref() != Some(&query);
        if !query_changed && self.attachment_picker.is_visible() {
            return;
        }

        let results = self.attachment_suggestions_for(&query);
        self.attachment_picker.query = Some(query);
        self.attachment_picker.dismissed_query = None;
        self.attachment_picker.open = true;
        self.attachment_picker.suggestions = results.suggestions;
        self.attachment_picker.status = results.status;
        if query_changed {
            self.attachment_picker.selected = 0;
            self.attachment_picker.scroll = 0;
        }
        self.attachment_picker.clamp_selected();
        self.refresh_attachment_picker_preview();
    }

    fn move_attachment_picker_selection(&mut self, delta: i32) {
        if self.attachment_picker.suggestions.is_empty() || delta == 0 {
            return;
        }

        let step = delta.signum();
        let last_index = self.attachment_picker.suggestions.len().saturating_sub(1) as i32;
        let next_index =
            (self.attachment_picker.selected as i32 + step).clamp(0, last_index) as usize;
        if next_index == self.attachment_picker.selected {
            return;
        }
        self.attachment_picker.selected = next_index;
        self.refresh_attachment_picker_preview();
    }

    fn confirm_attachment_picker_selection(&mut self) {
        let Some(query) = self.attachment_picker.query.clone() else {
            return;
        };

        // Prefer the exact path the user typed when it already exists.
        let workspace_root = self.tool_context.workspace_root.clone();
        match attachments::try_attach_from_query_if_exists(
            &mut self.input,
            &mut self.attachments,
            workspace_root.as_path(),
            &query,
        ) {
            Ok(Some(status)) => {
                self.status = status;
                self.clear_attachment_picker();
                return;
            }
            Ok(None) => {}
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        }

        if let Some(suggestion) = self.attachment_picker.selected_suggestion().cloned() {
            match attachments::try_attach_suggestion(
                &mut self.input,
                &mut self.attachments,
                &query,
                &suggestion,
            ) {
                Ok(Some(status)) => {
                    self.status = status;
                    self.clear_attachment_picker();
                }
                Ok(None) => {}
                Err(error) => {
                    self.status = error.to_string();
                }
            }
            return;
        }

        match attachments::try_attach_from_input(&mut self.input, &mut self.attachments) {
            Ok(Some(status)) => {
                self.status = status;
                self.clear_attachment_picker();
            }
            Ok(None) => {}
            Err(error) => {
                self.status = error.to_string();
            }
        }
    }

    fn handle_attachment_picker_key(&mut self, key: KeyEvent) -> Result<bool> {
        if closes_overlay(key) {
            self.dismiss_attachment_picker();
            return Ok(true);
        }

        match key.code {
            KeyCode::Up => {
                self.move_attachment_picker_selection(-1);
                Ok(true)
            }
            KeyCode::Down => {
                self.move_attachment_picker_selection(1);
                Ok(true)
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.confirm_attachment_picker_selection();
                Ok(true)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.confirm_attachment_picker_selection();
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn move_slash_menu_selection(&mut self, delta: i32) {
        let entries_len = self.slash_menu_entries().len();
        if entries_len == 0 || delta == 0 {
            return;
        }

        let last_index = entries_len.saturating_sub(1) as i32;
        let current_index = self.slash_menu_selected.min(entries_len.saturating_sub(1)) as i32;
        let next_index = (current_index + delta).clamp(0, last_index) as usize;
        self.slash_menu_selected = next_index;
    }

    fn reset_slash_menu_navigation(&mut self) {
        self.slash_menu_selected = 0;
        self.slash_menu_scroll = 0;
    }

    fn handle_slash_menu_key(&mut self, key: KeyEvent) -> bool {
        if matches!(
            key.code,
            KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown
        ) && !key.modifiers.is_empty()
        {
            return false;
        }

        match key.code {
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown => {}
            _ => return false,
        }

        if !self.slash_menu_visible() {
            return false;
        }

        match key.code {
            KeyCode::Up => self.move_slash_menu_selection(-1),
            KeyCode::Down => self.move_slash_menu_selection(1),
            KeyCode::Home => self.slash_menu_selected = 0,
            KeyCode::End => {
                self.slash_menu_selected = self.slash_menu_entries().len().saturating_sub(1)
            }
            KeyCode::PageUp => self.move_slash_menu_selection(-SLASH_MENU_PAGE_STEP),
            KeyCode::PageDown => self.move_slash_menu_selection(SLASH_MENU_PAGE_STEP),
            _ => unreachable!(),
        }

        true
    }

    fn handle_tab_key(&mut self) -> Result<bool> {
        if self.slash_menu_visible() {
            let entries = self.slash_menu_entries();
            let last = entries.len().saturating_sub(1);
            self.slash_menu_selected = self.slash_menu_selected.min(last);
            if let Some(entry) = entries.get(self.slash_menu_selected) {
                self.input.set_text(entry.insertion_text());
                self.clear_attachment_picker();
                self.status = format!("Command selected: /{}", entry.command.name);
                self.reset_slash_menu_navigation();
                return Ok(true);
            }
            if let Some(completed) = slash_menu::autocomplete_input(self.input.trim()) {
                self.input.set_text(completed.clone());
                self.clear_attachment_picker();
                self.status = format!("Command completed: {}", completed.trim_end());
                self.reset_slash_menu_navigation();
                return Ok(true);
            }
        }

        match attachments::try_attach_from_input(&mut self.input, &mut self.attachments) {
            Ok(Some(status)) => {
                self.status = status;
                self.clear_attachment_picker();
                return Ok(true);
            }
            Ok(None) => {}
            Err(error) => {
                self.status = error.to_string();
                return Ok(true);
            }
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
        self.push_conversation_message(ChatMessage::user(prompt));
        let assistant_index = self.messages.len();
        let assistant_message_id =
            self.push_conversation_message(ChatMessage::assistant(String::new()));
        self.assistant_index = Some(assistant_index);
        self.assistant_message_id = Some(assistant_message_id);
        self.streaming = true;
        self.status = if self.config.model == AUTO_MODEL {
            format!("Auto routed to {routed_model}; streaming from MiMo...")
        } else {
            "Streaming from MiMo...".to_string()
        };
        self.scroll_to_bottom();
        self.attachments.clear();
        self.clear_attachment_picker();
        let tool_context = self.tool_context.child_operation();
        let tool_registry = self.tool_registry.clone();
        let approval_mode = Arc::clone(&self.approval_mode_shared);
        self.stream_context = Some(tool_context.clone());

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
        self.event_tx = Some(event_tx.clone());
        self.input.clear();
        self.clear_attachment_picker();
        self.reset_slash_menu_navigation();
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
                    &self.active_skills,
                    self.diagnostics_auto_run,
                    &self.plan_items,
                    &self.attachments,
                    &self.messages,
                    Some(&self.conversation_tree),
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
                    &self.active_skills,
                    self.diagnostics_auto_run,
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
                let summary = mimo_tools::summarize_workspace(2, 60)?;
                self.push_system_message(summary);
                self.scroll_to_bottom();
                self.status = "Project context added to the conversation".to_string();
                Ok(false)
            }
            SlashCommand::Branch { message_index } => {
                self.handle_branch_command(message_index)?;
                Ok(false)
            }
            SlashCommand::Branches => {
                self.show_branches();
                Ok(false)
            }
            SlashCommand::Switch { branch_id } => {
                self.switch_branch(&branch_id)?;
                Ok(false)
            }
            SlashCommand::Mode { mode } => {
                if let Some(mode) = mode {
                    self.set_mode(app_mode_for(mode));
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
            SlashCommand::Review(command) => {
                self.handle_review_command(command)?;
                Ok(false)
            }
            SlashCommand::Lsp(command) => {
                self.handle_lsp_command(command)?;
                Ok(false)
            }
            SlashCommand::Skills => {
                self.push_system_message(self.skills_summary()?);
                self.status = "Installed skills shown".to_string();
                Ok(false)
            }
            SlashCommand::Skill(command) => {
                self.handle_skill_command(command)?;
                Ok(false)
            }
            SlashCommand::Mcp(command) => {
                self.handle_mcp_command(command)?;
                Ok(false)
            }
        }
    }

    fn handle_branch_command(&mut self, message_index: Option<usize>) -> Result<()> {
        if self.streaming {
            self.status = "Cannot branch while MiMo is responding".to_string();
            return Ok(());
        }

        if self.messages.is_empty() {
            self.status = "No conversation messages available to branch".to_string();
            return Ok(());
        }

        let selected_index = match message_index {
            Some(0) => {
                self.status = "Message numbers start at 1".to_string();
                return Ok(());
            }
            Some(index) => index,
            None => {
                let Some(index) = self.default_branch_source_index() else {
                    self.status =
                        "No branchable messages are available in this conversation".to_string();
                    return Ok(());
                };
                index
            }
        };

        let Some(branch_id) = self.create_branch_from_visible_index(selected_index)? else {
            return Ok(());
        };

        self.status = format!(
            "Created branch {branch_id} from message {selected_index}; type a new prompt to continue."
        );
        Ok(())
    }

    fn default_branch_source_index(&self) -> Option<usize> {
        self.messages
            .iter()
            .enumerate()
            .rev()
            .find(|(_, message)| matches!(message.role, Role::User))
            .map(|(index, _)| index + 1)
            .or_else(|| {
                self.messages
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, message)| !matches!(message.role, Role::System))
                    .map(|(index, _)| index + 1)
            })
    }

    fn open_branch_selection(&mut self) {
        if self.streaming {
            self.status = "Cannot branch while MiMo is responding".to_string();
            return;
        }

        let Some(selected) = self.default_branch_source_index() else {
            self.status = "No branchable messages are available in this conversation".to_string();
            return;
        };

        self.clear_attachment_picker();
        self.branch_selection.open = true;
        self.branch_selection.selected = selected.saturating_sub(1);
        self.ensure_branch_selection_visible();
    }

    fn handle_branch_selection_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => self.cancel_branch_selection(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_branch_selection()
            }
            KeyCode::Enter => self.confirm_branch_selection()?,
            KeyCode::Up => self.move_branch_selection(-1),
            KeyCode::Down => self.move_branch_selection(1),
            KeyCode::Home => {
                if let Some(index) = self.first_branchable_index() {
                    self.branch_selection.selected = index;
                    self.ensure_branch_selection_visible();
                }
            }
            KeyCode::End => {
                if let Some(index) = self.last_branchable_index() {
                    self.branch_selection.selected = index;
                    self.ensure_branch_selection_visible();
                }
            }
            _ => {}
        }

        Ok(false)
    }

    fn move_branch_selection(&mut self, delta: i32) {
        if self.messages.is_empty() || delta == 0 {
            return;
        }

        let step = delta.signum();
        let last_index = self.messages.len().saturating_sub(1) as i32;
        let mut next_index = self.branch_selection.selected as i32;

        loop {
            let candidate = (next_index + step).clamp(0, last_index);
            if candidate == next_index {
                break;
            }

            next_index = candidate;
            if self.is_branchable_message(next_index as usize) {
                self.branch_selection.selected = next_index as usize;
                self.ensure_branch_selection_visible();
                break;
            }
        }
    }

    fn confirm_branch_selection(&mut self) -> Result<()> {
        let selected_index = self.branch_selection.selected + 1;
        let Some(branch_id) = self.create_branch_from_visible_index(selected_index)? else {
            return Ok(());
        };

        self.branch_selection = BranchSelectionState::default();
        self.status = format!(
            "Created branch {branch_id} from message {selected_index}; type a new prompt to continue."
        );
        Ok(())
    }

    fn cancel_branch_selection(&mut self) {
        self.branch_selection = BranchSelectionState::default();
    }

    fn create_branch_from_visible_index(
        &mut self,
        selected_index: usize,
    ) -> Result<Option<String>> {
        let Some(message_id) = self
            .visible_message_id(selected_index)
            .map(ToOwned::to_owned)
        else {
            self.status = format!("Message {selected_index} is out of range");
            return Ok(None);
        };

        let Some(message) = self.messages.get(selected_index - 1) else {
            self.status = format!("Message {selected_index} is out of range");
            return Ok(None);
        };
        if matches!(message.role, Role::System) {
            self.status = format!("Cannot branch from system message {selected_index}");
            return Ok(None);
        }

        let branch_id = self
            .conversation_tree
            .create_branch_from_message(&message_id)?;
        self.sync_messages_from_tree();
        self.assistant_index = None;
        self.assistant_message_id = None;
        self.scroll_to_bottom();
        Ok(Some(branch_id))
    }

    fn is_branchable_message(&self, index: usize) -> bool {
        self.messages
            .get(index)
            .is_some_and(|message| !matches!(message.role, Role::System))
    }

    fn first_branchable_index(&self) -> Option<usize> {
        self.messages
            .iter()
            .position(|message| !matches!(message.role, Role::System))
    }

    fn last_branchable_index(&self) -> Option<usize> {
        self.messages
            .iter()
            .rposition(|message| !matches!(message.role, Role::System))
    }

    fn ensure_branch_selection_visible(&mut self) {
        if !self.branch_selection.open {
            return;
        }

        let Some(label_offset) = self
            .message_label_line_offsets()
            .get(self.branch_selection.selected)
            .copied()
        else {
            return;
        };

        self.scroll = adjust_selection_scroll(
            label_offset,
            self.scroll,
            self.conversation_view_height.max(1) as usize,
        );
    }

    fn message_label_line_offsets(&self) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(self.messages.len());
        let mut line_offset = 0;

        for message in &self.messages {
            offsets.push(line_offset);
            line_offset += 1;
            line_offset += if message.content.is_empty() {
                1
            } else {
                markdown::render_markdown_lines(&message.content).len()
            };
            line_offset += 1;
        }

        offsets
    }

    fn show_branches(&mut self) {
        let mut output = String::from("Conversation branches\n\n");
        for summary in self.conversation_tree.branch_summaries() {
            let marker = if summary.is_current { "*" } else { "-" };
            let source = summary
                .created_from_message_number
                .map(|index| format!("message {index}"))
                .unwrap_or_else(|| "root".to_string());
            output.push_str(&format!(
                "{marker} {} | {} messages | from {} | {}\n",
                summary.id, summary.message_count, source, summary.last_message_preview
            ));
        }

        self.push_system_message(output.trim_end().to_string());
        self.status = "Conversation branches shown".to_string();
    }

    fn switch_branch(&mut self, branch_id: &str) -> Result<()> {
        if self.streaming {
            self.status = "Cannot switch branches while MiMo is responding".to_string();
            return Ok(());
        }

        if let Err(error) = self.conversation_tree.switch_branch(branch_id) {
            self.status = error.to_string();
            return Ok(());
        }

        self.sync_messages_from_tree();
        self.assistant_index = None;
        self.assistant_message_id = None;
        self.tool_runtime.clear_transient();
        self.branch_selection = BranchSelectionState::default();
        self.scroll_to_bottom();
        self.status = format!("Switched to branch {branch_id}");
        Ok(())
    }

    fn handle_skill_command(&mut self, command: SkillCommand) -> Result<()> {
        match command {
            SkillCommand::Toggle(name) => {
                let skill = skill_store::load_skill(&self.config, &name)?;
                if let Some(index) = self
                    .active_skills
                    .iter()
                    .position(|active| active == &skill.name)
                {
                    self.active_skills.remove(index);
                    self.status = format!("Deactivated skill {}", skill.name);
                } else {
                    self.active_skills.push(skill.name.clone());
                    self.active_skills.sort();
                    self.active_skills.dedup();
                    self.status = format!("Activated skill {}", skill.name);
                }
            }
            SkillCommand::Install(spec) => {
                self.start_skill_install(spec)?;
            }
            SkillCommand::Show(name) => {
                let skill = skill_store::load_skill(&self.config, &name)?;
                self.push_system_message(format!(
                    "Skill {}\n\nPath: {}\nDescription: {}\n\n{}",
                    skill.name,
                    skill.path.display(),
                    skill.description,
                    skill.content
                ));
                self.status = format!("Skill {} shown", skill.name);
            }
            SkillCommand::Uninstall(name) => {
                let normalized = skill_store::normalize_skill_name(&name);
                let path = skill_store::uninstall_skill(&self.config, &normalized)?;
                self.active_skills.retain(|skill| skill != &normalized);
                self.refresh_skill_cache();
                self.status = format!("Removed skill {} from {}", normalized, path.display());
            }
        }
        Ok(())
    }

    fn handle_mcp_command(&mut self, command: McpCommand) -> Result<()> {
        match command {
            McpCommand::List => {
                self.push_system_message(self.mcp_summary());
                self.status = "MCP servers shown".to_string();
            }
            McpCommand::Show(name) => {
                let server = mcp_store::find_server(&self.config, &name)?;
                self.push_system_message(format!(
                    "MCP server {}\n\nEnabled   : {}\nTransport : {}\nEndpoint  : {}",
                    server.name,
                    if server.enabled { "yes" } else { "no" },
                    server.transport,
                    server.summary()
                ));
                self.status = format!("MCP server {} shown", server.name);
            }
            McpCommand::AddStdio {
                name,
                command,
                args,
            } => {
                let server = mcp_store::add_stdio_server(&self.config, &name, &command, args)?;
                self.refresh_mcp_cache();
                self.status = format!("Saved MCP server {} ({})", server.name, server.summary());
            }
            McpCommand::AddHttp { name, url } => {
                let server = mcp_store::add_http_server(&self.config, &name, &url)?;
                self.refresh_mcp_cache();
                self.status = format!("Saved MCP server {} ({})", server.name, server.summary());
            }
            McpCommand::Enable(name) => {
                let server = mcp_store::set_enabled(&self.config, &name, true)?;
                self.refresh_mcp_cache();
                self.status = format!("Enabled MCP server {}", server.name);
            }
            McpCommand::Disable(name) => {
                let server = mcp_store::set_enabled(&self.config, &name, false)?;
                self.refresh_mcp_cache();
                self.status = format!("Disabled MCP server {}", server.name);
            }
            McpCommand::Remove(name) => {
                let server = mcp_store::remove_server(&self.config, &name)?;
                self.refresh_mcp_cache();
                self.status = format!("Removed MCP server {}", server.name);
            }
        }
        Ok(())
    }

    fn refresh_skill_cache(&mut self) {
        self.installed_skills_cache =
            skill_store::list_installed_skills(&self.config).unwrap_or_default();
    }

    fn installed_skills(&self) -> &[skill_store::InstalledSkill] {
        &self.installed_skills_cache
    }

    fn skills_summary(&self) -> Result<String> {
        let mut output = format!(
            "Skills\n\nDirectory: {}\n",
            skill_store::skills_path(&self.config).display()
        );
        if self.installed_skills().is_empty() {
            output.push_str("\nNo installed skills.\n");
        } else {
            output.push('\n');
            for skill in self.installed_skills() {
                let active = if self.active_skills.iter().any(|name| name == &skill.name) {
                    "active"
                } else {
                    "idle"
                };
                output.push_str(&format!(
                    "- {} | {} | {}\n",
                    skill.name, active, skill.description
                ));
            }
        }
        output.push_str(
            "\nCommands:\n/skill install <path-or-url>\n/skill <name>\n/skill show <name>\n/skill uninstall <name>",
        );
        Ok(output.trim_end().to_string())
    }

    fn refresh_mcp_cache(&mut self) {
        self.mcp_servers_cache = mcp_store::load_servers(&self.config).unwrap_or_default();
    }

    fn mcp_servers(&self) -> &[mcp_store::McpServerConfig] {
        &self.mcp_servers_cache
    }

    fn mcp_summary(&self) -> String {
        let mut output = format!(
            "MCP servers\n\nFile: {}\n",
            mcp_store::mcp_servers_path(&self.config).display()
        );
        if self.mcp_servers().is_empty() {
            output.push_str("\nNo configured MCP servers.\n");
        } else {
            output.push('\n');
            for server in self.mcp_servers() {
                output.push_str(&format!(
                    "- {} | {} | {} | {}\n",
                    server.name,
                    if server.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    server.transport,
                    server.summary()
                ));
            }
        }
        output.push_str(
            "\nCommands:\n/mcp list\n/mcp show <name>\n/mcp add stdio <name> <command> [args...]\n/mcp add http <name> <url>\n/mcp enable <name>\n/mcp disable <name>\n/mcp remove <name>",
        );
        output.trim_end().to_string()
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
                if let Some(context) = self.task_contexts.remove(&id) {
                    context.cancel();
                }
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
        let tool_context = self.tool_context.child_operation();
        let tool_registry = self.tool_registry.clone();
        self.task_contexts.insert(id.clone(), tool_context.clone());

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
                            ApprovalRequirement::Auto => Ok(true),
                            ApprovalRequirement::Prompt => Ok(allow_prompt_tools),
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

        self.clear_attachment_picker();
        self.messages.clear();
        self.conversation_tree = ConversationTree::new();
        self.assistant_index = None;
        self.assistant_message_id = None;
        self.branch_selection = BranchSelectionState::default();
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
        self.clear_attachment_picker();
        self.reset_slash_menu_navigation();
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
        self.clear_attachment_picker();
        self.reset_slash_menu_navigation();
        self.status = "Draft stashed".to_string();
    }

    fn sync_messages_from_tree(&mut self) {
        self.messages = self.conversation_tree.current_messages();
        self.last_prompt = self.last_user_prompt();
    }

    fn push_conversation_message(&mut self, message: ChatMessage) -> MessageId {
        let message_id = self
            .conversation_tree
            .add_message_to_current(message.clone());
        self.messages.push(message);
        self.last_prompt = self.last_user_prompt();
        message_id
    }

    fn append_streaming_assistant_delta(&mut self, delta: &str) {
        if let Some(index) = self.assistant_index
            && let Some(message) = self.messages.get_mut(index)
        {
            message.content.push_str(delta);
        }

        if let Some(message_id) = self.assistant_message_id.clone()
            && let Some(message) = self.conversation_tree.message_mut(&message_id)
        {
            message.content.push_str(delta);
        }
    }

    fn set_streaming_assistant_content(&mut self, content: String) {
        if let Some(index) = self.assistant_index
            && let Some(message) = self.messages.get_mut(index)
        {
            message.content = content.clone();
        }

        if let Some(message_id) = self.assistant_message_id.clone()
            && let Some(message) = self.conversation_tree.message_mut(&message_id)
        {
            message.content = content;
        }
    }

    fn visible_message_id(&self, one_based_index: usize) -> Option<&str> {
        one_based_index
            .checked_sub(1)
            .and_then(|index| self.conversation_tree.message_id_at_current_index(index))
    }

    fn reset_conversation_from_messages(&mut self, messages: Vec<ChatMessage>) {
        self.conversation_tree = ConversationTree::from_flat_messages(messages);
        self.sync_messages_from_tree();
        self.assistant_index = None;
        self.assistant_message_id = None;
    }

    fn push_system_message(&mut self, content: String) {
        self.push_conversation_message(ChatMessage::system(content));
        self.scroll_to_bottom();
    }

    fn request_messages(&self) -> Result<Vec<ChatMessage>> {
        let mut messages = self.base_request_messages(self.mode)?;
        if !self.plan_items.is_empty() {
            messages.push(ChatMessage::system(self.plan_summary()));
        }
        messages.extend(attachments::attachment_messages(&self.attachments)?);
        messages.extend(self.conversation_tree.current_messages());
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
        let mut messages = Vec::with_capacity(memory_notes.len() + self.active_skills.len() + 4);
        messages.push(ChatMessage::system(self.config.system_prompt.clone()));
        if mode == AppMode::Plan {
            messages.push(ChatMessage::system(
                "You are in planning mode. Analyze the project, propose concrete implementation steps, and do not claim that files were modified or commands were executed unless the user explicitly switches to agent or yolo mode.".to_string(),
            ));
        }
        for skill_name in &self.active_skills {
            if let Ok(skill) = skill_store::load_skill(&self.config, skill_name) {
                messages.push(ChatMessage::system(format!(
                    "Active skill: {}\n\n{}",
                    skill.name, skill.content
                )));
            }
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
            if let Some(tool_context) = &self.stream_context {
                tool_context.cancel();
            }
            stream_task.abort();
        }
        self.stream_context = None;
        self.streaming = false;
        let should_mark_cancelled = self
            .assistant_index
            .and_then(|index| self.messages.get(index))
            .is_some_and(|message| message.content.trim().is_empty());
        if should_mark_cancelled {
            self.set_streaming_assistant_content("Request cancelled.".to_string());
        }
        self.assistant_index = None;
        self.assistant_message_id = None;
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
        self.clear_attachment_picker();
        self.help.open = true;
        self.help.scroll = 0;
        self.help.filter.set_text(topic.unwrap_or_default());
    }

    fn toggle_mode(&mut self) {
        self.set_mode(next_mode(self.mode));
        self.status = format!("Mode switched to {}", self.mode);
    }

    fn open_model_picker(&mut self, event_tx: UnboundedSender<AppEvent>) {
        self.clear_attachment_picker();
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
        self.clear_attachment_picker();
        self.session_picker.open = true;
        self.session_picker.entries = entries;
        self.session_picker.selected = 0;
        self.session_picker.scroll = 0;
        self.status = "Session picker opened".to_string();
        Ok(())
    }

    fn open_command_palette(&mut self) {
        self.clear_attachment_picker();
        self.command_palette = CommandPaletteState::default();
        self.command_palette.open = true;
        self.status = "Command palette opened".to_string();
    }

    fn command_palette_entries(&self) -> Vec<command_palette::PaletteEntry> {
        command_palette::filtered_entries(
            self.command_palette.filter.trim(),
            self.mode,
            self.installed_skills(),
            &self.active_skills,
            self.mcp_servers(),
        )
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
        self.clear_attachment_picker();
        self.draft_browser.open = true;
        self.draft_browser.kind = kind;
        self.draft_browser.selected = 0;
        self.draft_browser.scroll = 0;
        self.status = match kind {
            DraftBrowserKind::History => "Draft history opened".to_string(),
            DraftBrowserKind::Stash => "Draft stash opened".to_string(),
        };
    }

    fn open_inspector_browser(&mut self, kind: InspectorBrowserKind) -> Result<()> {
        if self.inspector_browser_items_for(kind).is_empty() {
            self.status = match kind {
                InspectorBrowserKind::Tasks => "No background tasks yet".to_string(),
                InspectorBrowserKind::Snapshots => "No workspace snapshots yet".to_string(),
            };
            return Ok(());
        }
        self.clear_attachment_picker();
        self.inspector_browser.open = true;
        self.inspector_browser.kind = kind;
        self.inspector_browser.selected = 0;
        self.inspector_browser.scroll = 0;
        self.status = match kind {
            InspectorBrowserKind::Tasks => "Task browser opened".to_string(),
            InspectorBrowserKind::Snapshots => "Snapshot browser opened".to_string(),
        };
        Ok(())
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
        self.open_text_pager(format!("Last message — {role}"), message.content.clone());
        self.status = "Opened last message pager".to_string();
    }

    fn open_text_pager(&mut self, title: impl Into<String>, content: impl Into<String>) {
        self.clear_attachment_picker();
        self.message_pager.open = true;
        self.message_pager.title = title.into();
        self.message_pager.lines = markdown::render_markdown_lines(&content.into());
        self.message_pager.scroll = 0;
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
        if let Some(stream_task) = self.stream_task.take() {
            if let Some(tool_context) = &self.stream_context {
                tool_context.cancel();
            }
            stream_task.abort();
        }
        self.stream_context = None;
        self.streaming = false;

        let session_store::SavedSession {
            mode,
            model,
            active_skills,
            lsp_auto_run,
            plan_items,
            attachments,
            messages,
            conversation_tree,
            ..
        } = session;
        let warning_suffix = if let Some(mut conversation_tree) = conversation_tree {
            match conversation_tree.validate_or_repair() {
                Ok(()) => {
                    if conversation_tree.current_messages().is_empty() && !messages.is_empty() {
                        self.reset_conversation_from_messages(messages.clone());
                        " (branch data ignored: empty active branch)".to_string()
                    } else {
                        self.conversation_tree = conversation_tree;
                        self.sync_messages_from_tree();
                        String::new()
                    }
                }
                Err(error) => {
                    self.reset_conversation_from_messages(messages.clone());
                    format!(" (branch data ignored: {error})")
                }
            }
        } else {
            self.reset_conversation_from_messages(messages.clone());
            String::new()
        };

        self.set_mode(mode);
        self.config.model = model;
        self.active_skills = active_skills;
        self.diagnostics_auto_run = lsp_auto_run;
        self.plan_items = plan_items;
        self.attachments = attachments;
        self.clear_attachment_picker();
        self.assistant_index = None;
        self.assistant_message_id = None;
        self.tool_runtime.clear_transient();
        self.branch_selection = BranchSelectionState::default();
        self.scroll_to_bottom();
        self.session_picker = SessionPickerState::default();
        self.status = format!("Loaded session from {source}{warning_suffix}");
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
        self.clear_attachment_picker();
        self.sync_attachment_picker();
        self.draft_browser = DraftBrowserState::default();
        self.status = "Draft restored".to_string();
    }

    fn inspect_selected_browser_entry(&mut self) -> Result<()> {
        match self.inspector_browser.kind {
            InspectorBrowserKind::Tasks => {
                let Some(task) = self.tasks.get(self.inspector_browser.selected).cloned() else {
                    self.inspector_browser = InspectorBrowserState::default();
                    return Ok(());
                };
                self.open_text_pager(
                    format!("Task {}", task.id),
                    format!(
                        "Background task details\n\n{}",
                        to_string_pretty(&task).context("failed to encode background task")?
                    ),
                );
                self.status = format!("Opened task {}", task.id);
            }
            InspectorBrowserKind::Snapshots => {
                let snapshots = self.tool_context.list_workspace_snapshots()?;
                let Some(snapshot) = snapshots.get(self.inspector_browser.selected).cloned() else {
                    self.inspector_browser = InspectorBrowserState::default();
                    return Ok(());
                };
                let mut details = format!(
                    "Workspace snapshot {}\n\nSummary: {}\nCreated: {}\n\nFiles\n",
                    snapshot.id, snapshot.summary, snapshot.created_at_epoch
                );
                for file in snapshot.files {
                    details.push_str(&format!("- {}\n", file.path));
                }
                self.open_text_pager(format!("Snapshot {}", snapshot.id), details.trim_end());
                self.status = format!("Opened snapshot {}", snapshot.id);
            }
        }
        Ok(())
    }

    fn restore_selected_snapshot_from_browser(&mut self) -> Result<()> {
        let snapshots = self.tool_context.list_workspace_snapshots()?;
        let Some(snapshot) = snapshots.get(self.inspector_browser.selected).cloned() else {
            self.inspector_browser = InspectorBrowserState::default();
            return Ok(());
        };
        self.restore_snapshot(Some(&snapshot.id))?;
        self.inspector_browser = InspectorBrowserState::default();
        Ok(())
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
            command_palette::PaletteAction::ExecuteCommand(command) => {
                self.handle_slash_command(&command, event_tx)?;
            }
            command_palette::PaletteAction::OpenHelp => self.open_help(None),
            command_palette::PaletteAction::OpenModels => self.open_model_picker(event_tx),
            command_palette::PaletteAction::OpenSessions => self.open_session_picker()?,
            command_palette::PaletteAction::ShowStatus => {
                self.push_system_message(self.status_summary()?);
                self.status = "Status summary shown".to_string();
            }
            command_palette::PaletteAction::ShowLastMessage => self.open_last_message_pager(),
            command_palette::PaletteAction::ShowDiagnostics => {
                self.open_text_pager("Diagnostics", self.diagnostics_detail_message());
                self.status = "Diagnostics opened".to_string();
            }
            command_palette::PaletteAction::BrowseTasks => {
                self.open_inspector_browser(InspectorBrowserKind::Tasks)?
            }
            command_palette::PaletteAction::BrowseSnapshots => {
                self.open_inspector_browser(InspectorBrowserKind::Snapshots)?
            }
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
        for (index, message) in self.messages.iter().enumerate() {
            let marker = self
                .conversation_tree
                .message_id_at_current_index(index)
                .filter(|message_id| self.conversation_tree.is_branch_point(message_id))
                .map(|_| " ◇")
                .unwrap_or("");
            let prefix = if self.branch_selection.open && self.branch_selection.selected == index {
                "> "
            } else {
                ""
            };
            let (label, color) = match message.role {
                Role::System => ("System", Color::DarkGray),
                Role::User => ("You", Color::Green),
                Role::Assistant => ("MiMo", Color::Cyan),
                Role::Tool => ("Tool", Color::Yellow),
            };
            let label_style =
                if self.branch_selection.open && self.branch_selection.selected == index {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color).add_modifier(Modifier::BOLD)
                };

            lines.push(Line::styled(
                format!("{prefix}#{:>1} {label}:{marker}", index + 1),
                label_style,
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
        let installed_skills = self.installed_skills().len();
        let mcp_servers = self.mcp_servers();
        let enabled_mcp = mcp_servers.iter().filter(|server| server.enabled).count();
        let diagnostics_status = self
            .diagnostics
            .as_ref()
            .map(|snapshot| snapshot.status.to_string())
            .unwrap_or_else(|| "none".to_string());
        format!(
            "Configuration\n\nConfig file      : {}\nBase URL         : {}\nModel            : {}\nTemperature      : {}\nAPI key          : {}\nMode             : {}\nApprovals        : {}\nAttachments      : {}\nPlan items       : {}\nMemory notes     : {}\nSkills dir       : {}\nSkills active    : {}\nSkills installed : {}\nMCP file         : {}\nMCP enabled      : {}\nMCP servers      : {}\nDiagnostics file : {}\nDiagnostics auto : {}\nDiagnostics state: {}\nTasks file       : {}\nTasks saved      : {}",
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
            skill_store::skills_path(&self.config).display(),
            self.active_skills.len(),
            installed_skills,
            mcp_store::mcp_servers_path(&self.config).display(),
            enabled_mcp,
            mcp_servers.len(),
            diagnostics_store::diagnostics_path(&self.config).display(),
            if self.diagnostics_auto_run {
                "on"
            } else {
                "off"
            },
            diagnostics_status,
            task_store::tasks_path(&self.config).display(),
            self.tasks.len(),
        )
    }

    fn status_summary(&self) -> Result<String> {
        let session_count = session_store::list_sessions(&self.config)?.len();
        let memory_notes = memory_store::load_notes(&self.config)?.len();
        let installed_skills = self.installed_skills().len();
        let mcp_servers = self.mcp_servers();
        let enabled_mcp = mcp_servers.iter().filter(|server| server.enabled).count();
        let diagnostics_status = self
            .diagnostics
            .as_ref()
            .map(|snapshot| snapshot.status.to_string())
            .unwrap_or_else(|| "none".to_string());
        let request_chars = self
            .request_messages()?
            .iter()
            .map(|message| message.content.chars().count())
            .sum::<usize>();
        Ok(format!(
            "Status\n\nWorkspace         : {}\nMode              : {}\nApprovals         : {}\nModel             : {}\nStreaming         : {}\nMessages          : {}\nActive branch     : {}\nBranches          : {}\nSaved files       : {}\nAPI key           : {}\nAttachments       : {}\nDraft stash       : {}\nPlan items        : {}\nMemory notes      : {}\nSkills active     : {}\nSkills installed  : {}\nMCP enabled       : {}\nMCP servers       : {}\nDiagnostics auto  : {}\nDiagnostics state : {}\nRequest chars     : {}\nShell jobs        : {}\nTasks             : {}\nTools             : {}",
            self.tool_context.workspace_root.display(),
            self.mode,
            approval_mode_label(self.tool_runtime.approval_mode),
            self.config.model,
            if self.streaming { "yes" } else { "no" },
            self.messages.len(),
            self.conversation_tree.current_branch_id(),
            self.conversation_tree.branch_count(),
            session_count,
            self.config.masked_api_key(),
            self.attachments.len(),
            self.draft_stash.len(),
            self.plan_items.len(),
            memory_notes,
            self.active_skills.len(),
            installed_skills,
            enabled_mcp,
            mcp_servers.len(),
            if self.diagnostics_auto_run {
                "on"
            } else {
                "off"
            },
            diagnostics_status,
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
        let mut new_messages = vec![ChatMessage::system(summary)];
        new_messages.extend(self.messages[split_index..].iter().cloned());
        self.conversation_tree
            .replace_current_branch_messages(new_messages);
        self.sync_messages_from_tree();
        self.assistant_index = None;
        self.assistant_message_id = None;
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
                    .filter(|job| job.status == ShellStatus::Running)
                    .count()
            })
            .unwrap_or(0)
    }

    fn start_skill_install(&mut self, spec: String) -> Result<()> {
        if self.skill_install_task.is_some() {
            self.status = "A skill install is already running".to_string();
            return Ok(());
        }
        let Some(event_tx) = self.event_tx.clone() else {
            bail!("event channel is unavailable");
        };
        let config = self.config.clone();
        self.status = format!("Installing skill from {spec}...");
        self.skill_install_task = Some(tokio::spawn(async move {
            let result =
                tokio::task::spawn_blocking(move || skill_store::install_skill(&config, &spec))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
            let _ = event_tx.send(AppEvent::SkillInstalled(result));
        }));
        Ok(())
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

    fn inspector_browser_items(&self) -> Vec<String> {
        self.inspector_browser_items_for(self.inspector_browser.kind)
    }

    fn inspector_browser_items_for(&self, kind: InspectorBrowserKind) -> Vec<String> {
        match kind {
            InspectorBrowserKind::Tasks => self
                .tasks
                .iter()
                .map(|task| {
                    format!(
                        "{} | {:?} | mode={} | {}",
                        task.id, task.status, task.mode, task.prompt
                    )
                })
                .collect(),
            InspectorBrowserKind::Snapshots => self
                .tool_context
                .list_workspace_snapshots()
                .unwrap_or_default()
                .into_iter()
                .map(|snapshot| {
                    format!(
                        "{} | {} file(s) | {}",
                        snapshot.id,
                        snapshot.files.len(),
                        snapshot.summary
                    )
                })
                .collect(),
        }
    }

    fn inspector_browser_title(&self) -> &'static str {
        match self.inspector_browser.kind {
            InspectorBrowserKind::Tasks => "Background tasks",
            InspectorBrowserKind::Snapshots => "Workspace snapshots",
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
        !self.branch_selection.open
            && slash_menu::is_active(self.input.trim())
            && !self.slash_menu_entries().is_empty()
    }

    fn clamp_slash_menu_selection(&mut self) {
        let last = self.slash_menu_entries().len().saturating_sub(1);
        self.slash_menu_selected = self.slash_menu_selected.min(last);
        self.slash_menu_scroll = 0;
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

fn landing_popup_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let mut rect = centered_rect(area, width_percent, height_percent);
    let max_y = area.y + area.height.saturating_sub(rect.height);
    let spare_height = area.height.saturating_sub(rect.height);
    let downward_offset = spare_height.saturating_add(5) / 6;
    rect.y = rect.y.saturating_add(downward_offset).min(max_y);
    rect
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

fn opens_branch_selection(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('b' | 'B') if key.modifiers.contains(KeyModifiers::CONTROL))
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
            && matches!(
                key.code,
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL)
            ))
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

fn estimated_max_context_chars(model: &str) -> usize {
    match model {
        "mimo-v2-flash" | "mimo-v2.5" | "mimo-v2.5-pro" | AUTO_MODEL => 1_000_000,
        _ => 1_000_000,
    }
}

fn format_compact_count(value: usize) -> String {
    match value {
        1_000_000.. => format_compact_decimal(value as f64 / 1_000_000.0, "M"),
        1_000.. => format_compact_decimal(value as f64 / 1_000.0, "K"),
        _ => value.to_string(),
    }
}

fn format_compact_decimal(value: f64, suffix: &str) -> String {
    let formatted = format!("{value:.1}");
    let formatted = formatted.strip_suffix(".0").unwrap_or(&formatted);
    format!("{formatted}{suffix}")
}

fn panel_border_style() -> Style {
    Style::default().fg(XIAOMI_ORANGE)
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

fn render_shell_result(result: &ShellResult) -> String {
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

fn capture_workspace_diagnostics(
    workspace_root: &std::path::Path,
) -> Result<diagnostics_store::DiagnosticsSnapshot> {
    if !workspace_root.join("Cargo.toml").exists() {
        bail!(
            "no default diagnostics command is available in {}",
            workspace_root.display()
        );
    }
    let output = Command::new("cargo")
        .current_dir(workspace_root)
        .args(["check", "--message-format", "short"])
        .output()
        .context("failed to run cargo check")?;
    let mut combined = String::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        combined.push_str(stdout.trim());
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push_str("\n\n");
        }
        combined.push_str(stderr.trim());
    }
    let output_text = truncate_for_summary_output(&combined, 24_000);
    let error_count = output_text
        .lines()
        .filter(|line| line.contains("error"))
        .count();
    let warning_count = output_text
        .lines()
        .filter(|line| line.contains("warning"))
        .count();
    let status = if output.status.success() {
        diagnostics_store::DiagnosticsStatus::Passed
    } else {
        diagnostics_store::DiagnosticsStatus::Failed
    };
    let summary = match status {
        diagnostics_store::DiagnosticsStatus::Passed => {
            if warning_count == 0 {
                "Diagnostics passed".to_string()
            } else {
                format!("Diagnostics passed with {warning_count} warning(s)")
            }
        }
        diagnostics_store::DiagnosticsStatus::Failed => {
            format!("Diagnostics failed with {error_count} error(s) and {warning_count} warning(s)")
        }
    };
    Ok(diagnostics_store::DiagnosticsSnapshot {
        updated_at_epoch: diagnostics_store::now_epoch(),
        command: "cargo check --message-format short".to_string(),
        status,
        summary,
        output: if output_text.trim().is_empty() {
            "No diagnostics output.".to_string()
        } else {
            output_text
        },
        error_count,
        warning_count,
    })
}

fn truncate_for_summary_output(content: &str, limit: usize) -> String {
    let mut shortened = content.chars().take(limit).collect::<String>();
    if content.chars().count() > limit {
        shortened.push_str("\n\n... output truncated ...");
    }
    shortened
}

fn first_non_empty_lines(content: &str, max_lines: usize) -> String {
    let lines = content
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "No diagnostics output.".to_string()
    } else {
        lines.join("\n")
    }
}

fn run_workspace_command(
    workspace_root: &std::path::Path,
    program: &str,
    args: &[&str],
) -> Result<String> {
    let output = Command::new(program)
        .current_dir(workspace_root)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {program} {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "{} {} exited with status {}",
            program,
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn relative_path_display(workspace_root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn should_refresh_diagnostics(request: &ToolRequest) -> bool {
    matches!(
        request.name.as_str(),
        "write_file" | "edit_file" | "apply_patch"
    )
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use dirs::home_dir;
    use ratatui::{Terminal, backend::TestBackend};
    use tempfile::tempdir;
    use tokio::sync::mpsc::unbounded_channel;

    use mimo_tui_core::commands::COMMANDS;

    use super::*;

    fn test_app() -> App {
        App::new(AppConfig {
            api_key: None,
            base_url: "https://example.test/v1".to_string(),
            model: "mimo-v2-flash".to_string(),
            temperature: 0.2,
            system_prompt: "test".to_string(),
            config_path: PathBuf::from("config.toml"),
            base_url_source: mimo_config::ConfigValueSource::Default,
            model_source: mimo_config::ConfigValueSource::Default,
            temperature_source: mimo_config::ConfigValueSource::Default,
            system_prompt_source: mimo_config::ConfigValueSource::Default,
            api_key_source: mimo_config::ConfigValueSource::Default,
        })
    }

    fn render_screen(app: &mut App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).expect("test terminal should initialize");
        terminal
            .draw(|frame| app.render(frame))
            .expect("app should render");

        let buffer = terminal.backend().buffer();
        let area = buffer.area();
        let mut screen = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                let symbol = buffer
                    .cell((x, y))
                    .expect("cell should exist inside the rendered area")
                    .symbol();
                screen.push_str(symbol);
            }
            screen.push('\n');
        }
        screen
    }

    fn attachment_query(text: &str) -> attachments::AttachmentQuery {
        attachments::current_attachment_query(&mimo_tui_core::input::InputBuffer::from(text))
            .expect("query should exist")
    }

    fn prepare_attachment_picker(
        app: &mut App,
        query: attachments::AttachmentQuery,
        results: attachments::AttachmentSearchResults,
    ) {
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(query),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: results.suggestions,
            preview: None,
            status: results.status,
        };
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
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);
        app.assistant_index = Some(1);
        app.assistant_message_id = Some("msg-2".to_string());
        app.scroll = 3;

        app.clear_conversation();
        assert!(app.messages.is_empty());
        assert!(app.conversation_tree.is_empty());
        assert_eq!(app.assistant_index, None);
        assert_eq!(app.assistant_message_id, None);
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
    fn request_messages_reads_from_the_conversation_tree() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![ChatMessage::user("hello")]);
        app.messages.push(ChatMessage::assistant("stale cache"));

        let request_messages = app.request_messages().expect("request messages");

        assert!(
            request_messages
                .iter()
                .any(|message| message.content == "hello")
        );
        assert!(
            !request_messages
                .iter()
                .any(|message| message.content == "stale cache")
        );
    }

    #[test]
    fn stream_deltas_update_the_tree_backed_assistant_message() {
        let mut app = test_app();
        app.push_conversation_message(ChatMessage::user("hello"));
        let assistant_message_id =
            app.push_conversation_message(ChatMessage::assistant(String::new()));
        app.assistant_index = Some(1);
        app.assistant_message_id = Some(assistant_message_id.clone());

        app.handle_app_event(AppEvent::Delta("hi".to_string()));

        assert_eq!(app.messages[1].content, "hi");
        assert_eq!(
            app.conversation_tree
                .message_mut(&assistant_message_id)
                .expect("assistant message should exist")
                .content
                .as_str(),
            "hi"
        );
    }

    #[test]
    fn branch_command_creates_branch_from_explicit_index() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);

        app.handle_branch_command(Some(2))
            .expect("branch command should succeed");

        assert_eq!(app.conversation_tree.branch_count(), 2);
        assert_eq!(app.conversation_tree.current_branch_id(), "branch-2");
        assert_eq!(app.messages.len(), 2);
        assert!(
            app.status
                .contains("Created branch branch-2 from message 2")
        );
    }

    #[test]
    fn branch_command_defaults_to_last_user_message() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::system("summary"),
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);

        app.handle_branch_command(None)
            .expect("branch command should succeed");

        assert_eq!(app.messages.len(), 4);
        assert_eq!(
            app.messages.last().map(|message| message.content.as_str()),
            Some("follow up")
        );
        assert!(app.status.contains("message 4"));
    }

    #[test]
    fn branch_command_rejects_invalid_index() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![ChatMessage::user("hello")]);

        app.handle_branch_command(Some(9))
            .expect("invalid branch command should not error");

        assert_eq!(app.conversation_tree.branch_count(), 1);
        assert_eq!(app.status, "Message 9 is out of range");
    }

    #[test]
    fn branch_command_rejects_system_messages() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::system("summary"),
            ChatMessage::user("hello"),
        ]);

        app.handle_branch_command(Some(1))
            .expect("invalid branch command should not error");

        assert_eq!(app.conversation_tree.branch_count(), 1);
        assert_eq!(app.status, "Cannot branch from system message 1");
    }

    #[test]
    fn branches_command_lists_current_branch_and_ids() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);
        app.handle_branch_command(Some(2))
            .expect("branch command should succeed");

        app.show_branches();

        let output = &app
            .messages
            .last()
            .expect("system message should exist")
            .content;
        assert!(output.contains("branch-1"));
        assert!(output.contains("branch-2"));
        assert!(output.contains("* branch-2"));
    }

    #[test]
    fn switch_branch_changes_visible_messages() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);
        app.handle_branch_command(Some(2))
            .expect("branch command should succeed");
        app.push_conversation_message(ChatMessage::user("branched question"));
        app.push_conversation_message(ChatMessage::assistant("branched answer"));

        app.switch_branch("branch-1")
            .expect("switch should succeed");
        assert_eq!(app.messages.len(), 4);
        assert_eq!(
            app.messages.last().map(|message| message.content.as_str()),
            Some("answer")
        );

        app.switch_branch("branch-2")
            .expect("switch should succeed");
        assert_eq!(app.messages.len(), 4);
        assert_eq!(
            app.messages.last().map(|message| message.content.as_str()),
            Some("branched answer")
        );
    }

    #[test]
    fn switching_branches_updates_last_prompt_for_retry() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("root prompt"),
            ChatMessage::assistant("root answer"),
            ChatMessage::user("original prompt"),
            ChatMessage::assistant("original answer"),
        ]);
        app.handle_branch_command(Some(1))
            .expect("branch command should succeed");
        app.push_conversation_message(ChatMessage::user("branch prompt"));
        app.push_conversation_message(ChatMessage::assistant("branch answer"));

        app.switch_branch("branch-1")
            .expect("switch should succeed");
        assert_eq!(app.last_prompt.as_deref(), Some("original prompt"));

        app.switch_branch("branch-2")
            .expect("switch should succeed");
        assert_eq!(app.last_prompt.as_deref(), Some("branch prompt"));
    }

    #[test]
    fn compact_only_changes_the_active_branch() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("one"),
            ChatMessage::assistant("two"),
            ChatMessage::user("three"),
            ChatMessage::assistant("four"),
            ChatMessage::user("five"),
            ChatMessage::assistant("six"),
            ChatMessage::user("seven"),
            ChatMessage::assistant("eight"),
        ]);
        app.handle_branch_command(Some(6))
            .expect("branch command should succeed");
        app.push_conversation_message(ChatMessage::user("branched prompt"));
        app.push_conversation_message(ChatMessage::assistant("branched answer"));

        app.compact_conversation();
        assert!(matches!(
            app.messages.first().map(|message| &message.role),
            Some(&Role::System)
        ));

        app.switch_branch("branch-1")
            .expect("switch should succeed");
        assert_eq!(app.messages.len(), 8);
        assert_eq!(app.messages[0].content, "one");
    }

    #[test]
    fn slash_commands_clear_the_draft() {
        let mut app = test_app();
        app.input.insert_str("/status");
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        let (event_tx, _event_rx) = unbounded_channel();
        app.handle_slash_command("/status", event_tx)
            .expect("slash command should execute");

        assert!(app.input.is_empty());
        assert!(!app.attachment_picker.is_visible());
    }

    #[test]
    fn models_command_opens_picker() {
        let mut app = test_app();
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_slash_command("/models", event_tx)
            .expect("models command should execute");

        assert!(app.model_picker.open);
        assert!(!app.model_picker.models.is_empty());
        assert!(!app.model_picker.loading);
        assert!(!app.attachment_picker.is_visible());
    }

    #[test]
    fn model_picker_applies_selected_model() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };
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
        assert!(!app.attachment_picker.is_visible());
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
    fn opens_branch_selection_recognizes_ctrl_b_only() {
        assert!(!opens_branch_selection(KeyEvent::new(
            KeyCode::Char('b'),
            KeyModifiers::NONE
        )));
        assert!(opens_branch_selection(KeyEvent::new(
            KeyCode::Char('b'),
            KeyModifiers::CONTROL
        )));
    }

    #[test]
    fn ctrl_b_opens_branch_selection_when_messages_exist() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            event_tx,
        )
        .expect("ctrl+b should open branch selection");

        assert!(app.branch_selection.open);
        assert_eq!(app.branch_selection.selected, 0);
        assert!(!app.attachment_picker.is_visible());
    }

    #[test]
    fn ctrl_u_clears_the_draft_and_attachment_picker() {
        let mut app = test_app();
        app.input.insert_str("hello");
        app.slash_menu_selected = 4;
        app.slash_menu_scroll = 8;
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        let (event_tx, _event_rx) = unbounded_channel();
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL)),
            event_tx,
        )
        .expect("ctrl+u should clear the draft");

        assert!(app.input.is_empty());
        assert!(!app.attachment_picker.is_visible());
        assert_eq!(app.slash_menu_selected, 0);
        assert_eq!(app.slash_menu_scroll, 0);
        assert_eq!(app.status, "Draft cleared");
    }

    #[test]
    fn ctrl_s_stashes_the_draft_and_attachment_picker() {
        let mut app = test_app();
        app.input.insert_str("hello");
        app.slash_menu_selected = 4;
        app.slash_menu_scroll = 8;
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        let (event_tx, _event_rx) = unbounded_channel();
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            event_tx,
        )
        .expect("ctrl+s should stash the draft");

        assert!(app.input.is_empty());
        assert!(!app.attachment_picker.is_visible());
        assert_eq!(app.slash_menu_selected, 0);
        assert_eq!(app.slash_menu_scroll, 0);
        assert_eq!(app.draft_stash, vec!["hello".to_string()]);
        assert_eq!(app.status, "Draft stashed");
    }

    #[test]
    fn branch_selection_escape_closes_without_creating_branch() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);
        app.open_branch_selection();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("escape should be handled");

        assert!(!app.branch_selection.open);
        assert_eq!(app.conversation_tree.branch_count(), 1);
    }

    #[test]
    fn branch_selection_enter_creates_branch() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);
        app.open_branch_selection();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("enter should create a branch");

        assert!(!app.branch_selection.open);
        assert_eq!(app.conversation_tree.branch_count(), 2);
        assert_eq!(app.conversation_tree.current_branch_id(), "branch-2");
        assert_eq!(app.messages.len(), 3);
    }

    #[test]
    fn escape_cancels_active_stream() {
        let mut app = test_app();
        app.streaming = true;
        let assistant_message_id =
            app.push_conversation_message(ChatMessage::assistant(String::new()));
        app.assistant_index = Some(0);
        app.assistant_message_id = Some(assistant_message_id.clone());

        app.handle_escape_key();

        assert!(!app.streaming);
        assert_eq!(app.assistant_index, None);
        assert_eq!(app.assistant_message_id, None);
        assert_eq!(app.status, "Generation cancelled");
        assert_eq!(app.messages[0].content, "Request cancelled.");
        assert_eq!(
            app.conversation_tree
                .message_mut(&assistant_message_id)
                .expect("assistant message should exist")
                .content
                .as_str(),
            "Request cancelled."
        );
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
    fn landing_screen_replaces_workspace_panels_when_empty() {
        let mut app = test_app();
        let screen = render_screen(&mut app);
        assert!(screen.contains("Xiaomi"));
        assert!(screen.contains("Ask anything...  \"Fix a TODO in the codebase\""));
        assert!(screen.contains("(0%) · F1/? help"));
        assert!(!screen.contains("Enter send |"));
        assert!(!screen.contains("Ask anything about this workspace."));
        assert!(!screen.contains("Conversation"));
        assert!(!screen.contains("Plan"));
    }

    #[test]
    fn workspace_panels_return_once_messages_exist() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);

        let screen = render_screen(&mut app);
        assert!(screen.contains("Conversation"));
        assert!(screen.contains("Plan"));
    }

    #[test]
    fn render_shows_branch_title_and_marker() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);
        app.handle_branch_command(Some(2))
            .expect("branch command should succeed");

        let screen = render_screen(&mut app);
        assert!(screen.contains("Conversation — branch-2"));
        assert!(screen.contains("◇"));
    }

    #[test]
    fn yolo_mode_shows_persistent_safety_warning() {
        let mut app = test_app();
        app.mode = AppMode::Yolo;

        let screen = render_screen(&mut app);
        assert!(screen.contains("YOLO auto-approves mutating tools"));
    }

    #[test]
    fn plan_panel_remains_visible_outside_plan_mode() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);

        let screen = render_screen(&mut app);
        assert!(screen.contains("Conversation"));
        assert!(screen.contains("Plan"));

        app.mode = AppMode::Yolo;
        let screen = render_screen(&mut app);
        assert!(screen.contains("Conversation"));
        assert!(screen.contains("Plan"));
    }

    #[test]
    fn clearing_conversation_returns_to_landing_screen() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        app.clear_conversation();

        assert!(!app.attachment_picker.is_visible());
        let screen = render_screen(&mut app);
        assert!(screen.contains("Ask anything...  \"Fix a TODO in the codebase\""));
        assert!(screen.contains("Conversation cleared"));
    }

    #[test]
    fn opening_other_overlays_clears_attachment_picker() {
        let mut app = test_app();
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        app.open_help(None);
        assert!(app.help.open);
        assert!(!app.attachment_picker.is_visible());

        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };

        app.open_command_palette();
        assert!(app.command_palette.open);
        assert!(!app.attachment_picker.is_visible());
    }

    #[test]
    fn apply_loaded_session_restores_saved_branch_tree() {
        let mut app = test_app();
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: Vec::new(),
            preview: None,
            status: Some("open".to_string()),
        };
        let mut tree = ConversationTree::from_flat_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("hi"),
            ChatMessage::user("follow up"),
            ChatMessage::assistant("answer"),
        ]);
        tree.create_branch_from_message("msg-2")
            .expect("branch should be created");
        tree.add_message_to_current(ChatMessage::user("branched prompt"));
        tree.add_message_to_current(ChatMessage::assistant("branched answer"));

        app.apply_loaded_session(
            session_store::SavedSession {
                saved_at_epoch: 1,
                title: "hello".to_string(),
                model: "mimo-v2-flash".to_string(),
                mode: AppMode::Agent,
                active_skills: Vec::new(),
                lsp_auto_run: false,
                plan_items: Vec::new(),
                attachments: Vec::new(),
                messages: tree.current_messages(),
                conversation_tree: Some(tree),
            },
            "test.json".to_string(),
        );

        assert_eq!(app.conversation_tree.branch_count(), 2);
        assert_eq!(app.conversation_tree.current_branch_id(), "branch-2");
        assert!(!app.attachment_picker.is_visible());
        assert_eq!(
            app.messages.last().map(|message| message.content.as_str()),
            Some("branched answer")
        );
    }

    #[test]
    fn apply_loaded_session_migrates_legacy_flat_messages() {
        let mut app = test_app();
        app.apply_loaded_session(
            session_store::SavedSession {
                saved_at_epoch: 1,
                title: "legacy".to_string(),
                model: "mimo-v2-flash".to_string(),
                mode: AppMode::Agent,
                active_skills: Vec::new(),
                lsp_auto_run: false,
                plan_items: Vec::new(),
                attachments: Vec::new(),
                messages: vec![
                    ChatMessage::user("legacy"),
                    ChatMessage::assistant("session"),
                ],
                conversation_tree: None,
            },
            "legacy.json".to_string(),
        );

        assert_eq!(app.conversation_tree.branch_count(), 1);
        assert_eq!(app.messages.len(), 2);
        assert_eq!(app.conversation_tree.current_branch_id(), "branch-1");
    }

    #[test]
    fn lsp_commands_toggle_auto_diagnostics() {
        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_slash_command("/lsp on", event_tx.clone())
            .expect("lsp on should execute");
        assert!(app.diagnostics_auto_run);

        app.handle_slash_command("/lsp off", event_tx)
            .expect("lsp off should execute");
        assert!(!app.diagnostics_auto_run);
    }

    #[test]
    fn task_browser_opens_when_tasks_exist() {
        let mut app = test_app();
        app.tasks.push(task_store::SavedTask {
            id: "task-1".to_string(),
            prompt: "Review the diff".to_string(),
            model: "mimo-v2-flash".to_string(),
            routed_model: None,
            mode: AppMode::Agent,
            status: task_store::TaskStatus::Queued,
            created_at_epoch: 1,
            updated_at_epoch: 1,
            started_at_epoch: None,
            finished_at_epoch: None,
            assistant_output: String::new(),
            activity_log: Vec::new(),
            error: None,
        });

        app.open_inspector_browser(InspectorBrowserKind::Tasks)
            .expect("task browser should open");

        assert!(app.inspector_browser.open);
        assert_eq!(app.inspector_browser.kind, InspectorBrowserKind::Tasks);
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

    #[test]
    fn typing_at_opens_attachment_picker_and_filters_results() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::create_dir(tempdir.path().join("folder")).expect("folder should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("file should be written");
        fs::write(tempdir.path().join("banana.txt"), "banana").expect("file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("typing @ should update the picker");

        assert!(app.attachment_picker.is_visible());
        assert!(!app.attachment_picker.suggestions.is_empty());

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE)),
            event_tx,
        )
        .expect("typing should refine the picker");

        assert_eq!(app.input.as_str(), "@f");
        assert_eq!(app.attachment_picker.suggestions.len(), 1);
        assert_eq!(app.attachment_picker.suggestions[0].display_path, "folder");
    }

    #[test]
    fn attachment_picker_navigation_updates_preview() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("file should be written");
        fs::write(tempdir.path().join("apricot.txt"), "apricot").expect("file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("typing @ should open the picker");
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("typing a should filter results");

        let first_preview = app
            .attachment_picker
            .preview
            .as_ref()
            .expect("preview should exist")
            .metadata[0]
            .clone();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("down should move the selection");

        let next_preview = app
            .attachment_picker
            .preview
            .as_ref()
            .expect("preview should still exist")
            .metadata[0]
            .clone();
        assert_eq!(app.attachment_picker.selected, 1);
        assert_ne!(next_preview, first_preview);
        assert!(next_preview.contains("apricot.txt"));
    }

    #[test]
    fn attachment_picker_escape_can_reopen_after_query_changes() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("typing @ should open the picker");
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("escape should dismiss the picker");

        assert!(!app.attachment_picker.is_visible());
        assert!(app.attachment_picker.dismissed_query.is_some());

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            event_tx,
        )
        .expect("changing the query should reopen the picker");

        assert!(app.attachment_picker.is_visible());
        assert!(app.attachment_picker.dismissed_query.is_none());
        assert!(!app.attachment_picker.suggestions.is_empty());
    }

    #[test]
    fn attachment_picker_enter_confirms_without_submitting() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("typing @ should open the picker");

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("enter should confirm the selected attachment");

        assert_eq!(app.attachments.len(), 1);
        assert!(app.messages.is_empty());
        assert!(app.last_prompt.is_none());
        assert!(app.input.as_str().is_empty());
        assert!(!app.attachment_picker.is_visible());
        assert!(app.status.contains("Attached workspace context"));
    }

    #[test]
    fn attachment_picker_duplicate_confirmation_is_reported() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        app.attachments.push(FileAttachment::new("apple.txt"));
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('@'), KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("typing @ should open the picker");
        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            event_tx,
        )
        .expect("typing a should filter results");

        let (confirm_tx, _confirm_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            confirm_tx,
        )
        .expect("enter should confirm the selected attachment");

        assert_eq!(app.attachments.len(), 1);
        assert!(app.input.as_str().is_empty());
        assert!(app.messages.is_empty());
        assert!(app.last_prompt.is_none());
        assert!(app.status.contains("Attachment already added"));
        assert!(!app.attachment_picker.is_visible());
    }

    #[test]
    fn attachment_picker_prefers_exact_directory_attachment_for_trailing_slash() {
        let tempdir = tempdir().expect("tempdir should be created");
        let docs_dir = tempdir.path().join("docs");
        fs::create_dir(&docs_dir).expect("docs dir should be created");
        fs::write(docs_dir.join("child.txt"), "child").expect("child file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        app.input.set_text("@docs/");

        let query = attachment_query("@docs/");
        let results = attachments::discover_attachment_suggestions(&query, tempdir.path());
        assert_eq!(
            results
                .suggestions
                .first()
                .map(|suggestion| suggestion.display_path.as_str()),
            Some("docs/child.txt")
        );
        prepare_attachment_picker(&mut app, query, results);

        app.confirm_attachment_picker_selection();

        assert_eq!(app.attachments, vec![FileAttachment::new("docs")]);
        assert_eq!(app.input.as_str(), "");
        assert!(!app.attachment_picker.is_visible());
        assert_eq!(app.status, "Attached workspace context: docs");
    }

    #[test]
    fn attachment_picker_prefers_exact_home_directory_attachment_for_trailing_slash() {
        let home_path = home_dir().expect("home directory should exist");
        let _home_entry =
            tempfile::tempdir_in(&home_path).expect("temporary home entry should be created");
        let tempdir = tempdir().expect("tempdir should be created");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        app.input.set_text("@~/");

        let query = attachment_query("@~/");
        let results = attachments::discover_attachment_suggestions(&query, tempdir.path());
        assert!(
            !results.suggestions.is_empty(),
            "home directory should contain at least one entry"
        );
        prepare_attachment_picker(&mut app, query, results);

        app.confirm_attachment_picker_selection();

        let expected = home_path.display().to_string();
        let attachment_path = &app
            .attachments
            .first()
            .expect("attachment should be added")
            .path;
        assert_eq!(
            attachment_path.trim_end_matches(['/', '\\']),
            expected.trim_end_matches(['/', '\\'])
        );
        assert_eq!(app.input.as_str(), "");
        assert!(!app.attachment_picker.is_visible());
        assert_eq!(
            app.status,
            format!("Attached workspace context: {attachment_path}")
        );
    }

    #[test]
    fn attachment_picker_reuses_cached_suggestions_for_unchanged_query() {
        let tempdir = tempdir().expect("tempdir should be created");
        fs::write(tempdir.path().join("apple.txt"), "apple").expect("apple file should be written");

        let mut app = test_app();
        app.tool_context.workspace_root = tempdir.path().to_path_buf();
        app.input.set_text("@a");

        app.sync_attachment_picker();
        let initial_suggestions = app
            .attachment_picker
            .suggestions
            .iter()
            .map(|suggestion| suggestion.display_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(initial_suggestions, vec!["apple.txt".to_string()]);

        fs::write(tempdir.path().join("apricot.txt"), "apricot")
            .expect("apricot file should be written");
        app.attachment_picker.open = false;
        app.sync_attachment_picker();

        let cached_suggestions = app
            .attachment_picker
            .suggestions
            .iter()
            .map(|suggestion| suggestion.display_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(cached_suggestions, initial_suggestions);
        assert!(!cached_suggestions.iter().any(|path| path == "apricot.txt"));
    }

    #[test]
    fn attachment_picker_reuses_cached_preview_for_same_selection() {
        let tempdir = tempdir().expect("tempdir should be created");
        let file_path = tempdir.path().join("notes.txt");
        fs::write(&file_path, "old").expect("file should be written");

        let mut app = test_app();
        prepare_attachment_picker(
            &mut app,
            attachment_query("@notes"),
            attachments::AttachmentSearchResults {
                suggestions: vec![attachments::AttachmentSuggestion {
                    display_path: "notes.txt".to_string(),
                    resolved_path: file_path.clone(),
                    kind: attachments::AttachmentEntryKind::File,
                    score: 0,
                }],
                status: None,
            },
        );

        app.refresh_attachment_picker_preview();
        let first_preview = app
            .attachment_picker
            .preview
            .clone()
            .expect("preview should be generated");
        assert_eq!(first_preview.lines.clone(), vec!["old".to_string()]);

        fs::write(&file_path, "new").expect("file should be rewritten");
        app.refresh_attachment_picker_preview();

        assert_eq!(app.attachment_picker.preview, Some(first_preview));
    }

    #[test]
    fn tab_falls_back_to_exact_attachment_when_picker_inactive() {
        let tempdir = tempdir().expect("tempdir should be created");
        let file_path = tempdir.path().join("exact.txt");
        fs::write(&file_path, "exact").expect("file should be written");

        let mut app = test_app();
        let (event_tx, _event_rx) = unbounded_channel();
        app.input.set_text(format!("@{}", file_path.display()));

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("tab should still confirm an exact attachment");

        assert_eq!(app.attachments.len(), 1);
        assert!(app.input.as_str().is_empty());
        assert!(app.status.contains("Attached workspace context"));
    }

    #[test]
    fn attachment_picker_overlay_renders_suggestions_and_preview() {
        let mut app = test_app();
        app.attachment_picker = attachments::AttachmentPickerState {
            query: Some(attachments::AttachmentQuery {
                token_start: 0,
                token_end: 1,
                raw: String::new(),
            }),
            dismissed_query: None,
            open: true,
            selected: 0,
            scroll: 0,
            suggestions: vec![attachments::AttachmentSuggestion {
                display_path: "notes.txt".to_string(),
                resolved_path: PathBuf::from("notes.txt"),
                kind: attachments::AttachmentEntryKind::File,
                score: 0,
            }],
            preview: Some(attachments::AttachmentPreview {
                metadata: vec!["Path: notes.txt".to_string(), "Type: text".to_string()],
                lines: vec!["hello".to_string()],
                note: None,
            }),
            status: None,
        };

        let screen = render_screen(&mut app);
        assert!(screen.contains("Matches (1)"));
        assert!(screen.contains("Preview"));
        assert!(screen.contains("notes.txt"));
        assert!(screen.contains("Tab/Enter attach"));
    }

    #[test]
    fn slash_menu_overlay_scrolls_to_show_late_entries() {
        let mut app = test_app();
        app.input.set_text("/");
        let last_entry = slash_menu::visible_entries("/")
            .last()
            .expect("slash menu should have a last entry")
            .clone();
        app.slash_menu_selected = COMMANDS.len().saturating_sub(1);

        let screen = render_screen(&mut app);

        assert!(app.slash_menu_scroll > 0);
        assert!(screen.contains(&format!("/{}", last_entry.command.name)));
    }

    #[test]
    fn slash_menu_navigation_home_end_and_page_steps_do_not_scroll_conversation() {
        let mut app = test_app();
        app.input.set_text("/");
        app.scroll = 7;
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("page down should move the slash menu selection");
        assert_eq!(app.scroll, 7);
        assert!(app.slash_menu_selected > 0);

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("page up should move the slash menu selection");
        assert_eq!(app.scroll, 7);
        assert_eq!(app.slash_menu_selected, 0);

        for _ in 0..(COMMANDS.len() + 2) {
            app.handle_terminal_event(
                Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
                event_tx.clone(),
            )
            .expect("down should keep moving within the slash menu");
        }
        assert_eq!(app.scroll, 7);
        assert_eq!(app.slash_menu_selected, COMMANDS.len().saturating_sub(1));

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("up should move within the slash menu");
        assert_eq!(app.scroll, 7);
        assert_eq!(app.slash_menu_selected, COMMANDS.len().saturating_sub(2));

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE)),
            event_tx.clone(),
        )
        .expect("end should jump to the last slash command");
        assert_eq!(app.slash_menu_selected, COMMANDS.len().saturating_sub(1));

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("home should jump to the first slash command");
        assert_eq!(app.slash_menu_selected, 0);
    }

    #[test]
    fn slash_menu_tab_inserts_selected_entry_and_resets_navigation() {
        let mut app = test_app();
        app.input.set_text("/");
        let expected_input = slash_menu::visible_entries("/")
            .last()
            .expect("slash menu should have a last entry")
            .clone()
            .insertion_text();
        app.slash_menu_selected = COMMANDS.len().saturating_sub(1);
        app.slash_menu_scroll = 12;
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("tab should select the slash menu entry");

        assert_eq!(app.input.as_str(), expected_input);
        assert_eq!(app.slash_menu_selected, 0);
        assert_eq!(app.slash_menu_scroll, 0);
    }

    #[test]
    fn slash_menu_selection_clamps_and_resets_scroll_when_filter_changes() {
        let mut app = test_app();
        app.input.set_text("/mc");
        app.slash_menu_selected = COMMANDS.len();
        app.slash_menu_scroll = 12;
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            event_tx,
        )
        .expect("backspace should update the slash menu selection");

        let entries = app.slash_menu_entries();
        assert!(!entries.is_empty());
        assert_eq!(app.slash_menu_selected, entries.len().saturating_sub(1));
        assert_eq!(app.slash_menu_scroll, 0);
    }

    #[test]
    fn slash_menu_ctrl_home_and_end_scroll_the_conversation() {
        let mut app = test_app();
        app.reset_conversation_from_messages(vec![
            ChatMessage::user("hello"),
            ChatMessage::assistant("world"),
        ]);
        app.conversation_view_height = 3;
        app.input.set_text("/");
        assert!(app.slash_menu_entries().len() > 1);
        app.slash_menu_selected = 1;
        app.slash_menu_scroll = 9;
        app.scroll = 7;
        let expected_bottom = app.max_scroll();
        assert!(expected_bottom > 0);
        let (event_tx, _event_rx) = unbounded_channel();

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::Home, KeyModifiers::CONTROL)),
            event_tx.clone(),
        )
        .expect("ctrl+home should scroll the conversation to the top");

        assert_eq!(app.scroll, 0);
        assert_eq!(app.slash_menu_selected, 1);

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::CONTROL)),
            event_tx.clone(),
        )
        .expect("ctrl+pagedown should scroll the conversation");

        assert!(app.scroll > 0);
        assert_eq!(app.slash_menu_selected, 1);

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::CONTROL)),
            event_tx.clone(),
        )
        .expect("ctrl+pageup should scroll the conversation");

        assert_eq!(app.scroll, 0);
        assert_eq!(app.slash_menu_selected, 1);

        app.handle_terminal_event(
            Event::Key(KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL)),
            event_tx,
        )
        .expect("ctrl+end should scroll the conversation to the bottom");

        assert_eq!(app.scroll, expected_bottom);
        assert_eq!(app.slash_menu_selected, 1);
    }
}
