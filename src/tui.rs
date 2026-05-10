mod commands;
mod input;
mod project_context;
mod session_store;

use std::{env, fmt, io, time::Duration};

use anyhow::{Context, Result};
use commands::{CommandParseError, ConfigCommand, ModeName, SlashCommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use input::InputBuffer;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use crate::{
    client::{ChatMessage, MimoClient, Role},
    config::{AppConfig, known_mimo_models, normalize_base_url, normalize_model_name},
};

pub async fn run(config: AppConfig) -> Result<()> {
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    let (event_tx, mut event_rx) = unbounded_channel();
    let mut app = App::new(config);

    loop {
        terminal.draw(|frame| app.render(frame))?;

        while let Ok(event) = event_rx.try_recv() {
            app.handle_app_event(event);
        }

        if event::poll(Duration::from_millis(50))? {
            let event = event::read()?;
            if app.handle_terminal_event(event, event_tx.clone())? {
                break;
            }
        }
    }

    Ok(())
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

#[derive(Debug)]
enum AppEvent {
    Delta(String),
    Finished(Result<(), String>),
    ModelsLoaded(Result<Vec<String>, String>),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AppMode {
    Chat,
    Plan,
}

impl From<ModeName> for AppMode {
    fn from(value: ModeName) -> Self {
        match value {
            ModeName::Chat => Self::Chat,
            ModeName::Plan => Self::Plan,
        }
    }
}

impl fmt::Display for AppMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chat => formatter.write_str("chat"),
            Self::Plan => formatter.write_str("plan"),
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

#[derive(Debug)]
struct App {
    config: AppConfig,
    messages: Vec<ChatMessage>,
    input: InputBuffer,
    status: String,
    streaming: bool,
    assistant_index: Option<usize>,
    scroll: u16,
    conversation_view_height: u16,
    input_scroll: u16,
    help: HelpState,
    model_picker: ModelPickerState,
    discovered_models: Vec<String>,
    last_prompt: Option<String>,
    mode: AppMode,
    stream_task: Option<JoinHandle<()>>,
    model_load_task: Option<JoinHandle<()>>,
}

impl App {
    fn new(config: AppConfig) -> Self {
        let status = if config.api_key.is_some() {
            "Ready".to_string()
        } else {
            "Missing API key: use /config api-key <key> or edit config.toml".to_string()
        };

        Self {
            config,
            messages: Vec::new(),
            input: InputBuffer::new(),
            status,
            streaming: false,
            assistant_index: None,
            scroll: 0,
            conversation_view_height: 1,
            input_scroll: 0,
            help: HelpState::default(),
            model_picker: ModelPickerState::default(),
            discovered_models: Vec::new(),
            last_prompt: None,
            mode: AppMode::Chat,
            stream_task: None,
            model_load_task: None,
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let input_height = self.input_height(area.height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(input_height),
                Constraint::Length(1),
            ])
            .split(area);

        self.conversation_view_height = chunks[1].height.saturating_sub(2).max(1);
        self.clamp_scroll();

        self.render_header(frame, chunks[0]);
        self.render_conversation(frame, chunks[1]);
        self.render_input(frame, chunks[2]);
        self.render_footer(frame, chunks[3]);

        if self.help.open {
            self.render_help_overlay(frame, area);
        } else if self.model_picker.open {
            self.render_model_picker_overlay(frame, area);
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
            Span::raw(" | model: "),
            Span::styled(&self.config.model, Style::default().fg(Color::Magenta)),
            Span::raw(" | "),
            Span::styled(&self.status, status_style),
        ]);

        frame.render_widget(
            Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
            area,
        );
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

    fn render_input(&mut self, frame: &mut Frame, area: Rect) {
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

        if !self.help.open && !self.model_picker.open {
            let visible_line = cursor_line.saturating_sub(self.input_scroll as usize);
            let cursor_x =
                area.x + 1 + cursor_col.min(area.width.saturating_sub(2) as usize) as u16;
            let cursor_y = area.y + 1 + visible_line.min(visible_lines.saturating_sub(1)) as u16;
            frame.set_cursor_position((cursor_x, cursor_y));
        }
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let help = "Enter send | F1/? help | Esc cancel/close | /save session | /retry | Ctrl+U clear draft";
        frame.render_widget(
            Paragraph::new(help).style(Style::default().fg(Color::DarkGray)),
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

        let filter_visible_lines = inner[0].height.saturating_sub(2).max(1) as usize;
        let (filter_line, filter_col) = self.help.filter.cursor_line_col();
        let filter_scroll =
            filter_line.saturating_sub(filter_visible_lines.saturating_sub(1)) as u16;

        frame.render_widget(
            Paragraph::new(self.help.filter.as_str())
                .block(Block::default().title("Search").borders(Borders::ALL))
                .scroll((filter_scroll, 0)),
            inner[0],
        );

        let lines = commands::help_overlay_lines(self.help.filter.trim());
        let max_scroll = lines
            .len()
            .saturating_sub(inner[1].height.saturating_sub(2) as usize)
            .try_into()
            .unwrap_or(u16::MAX);
        self.help.scroll = self.help.scroll.min(max_scroll);
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

        let visible_line = filter_line.saturating_sub(filter_scroll as usize);
        let cursor_x =
            inner[0].x + 1 + filter_col.min(inner[0].width.saturating_sub(2) as usize) as u16;
        let cursor_y =
            inner[0].y + 1 + visible_line.min(filter_visible_lines.saturating_sub(1)) as u16;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    fn render_model_picker_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let popup = centered_rect(area, 60, 55);
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
        let max_scroll = self
            .model_picker
            .models
            .len()
            .saturating_sub(body_height)
            .try_into()
            .unwrap_or(u16::MAX);
        if self.model_picker.selected < self.model_picker.scroll as usize {
            self.model_picker.scroll = self.model_picker.selected as u16;
        } else if self.model_picker.selected >= self.model_picker.scroll as usize + body_height {
            self.model_picker.scroll = (self.model_picker.selected + 1 - body_height) as u16;
        }
        self.model_picker.scroll = self.model_picker.scroll.min(max_scroll);

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

    fn message_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for message in &self.messages {
            let (label, color) = match message.role {
                Role::System => ("System", Color::DarkGray),
                Role::User => ("You", Color::Green),
                Role::Assistant => ("MiMo", Color::Cyan),
            };

            lines.push(Line::styled(
                format!("{label}:"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));

            if message.content.is_empty() {
                lines.push(Line::styled("  ...", Style::default().fg(Color::DarkGray)));
            } else {
                for line in message.content.lines() {
                    lines.push(Line::raw(format!("  {line}")));
                }
            }
            lines.push(Line::raw(""));
        }
        lines
    }

    fn handle_terminal_event(
        &mut self,
        event: Event,
        event_tx: UnboundedSender<AppEvent>,
    ) -> Result<bool> {
        match event {
            Event::Key(key) => {
                if self.help.open {
                    return Ok(self.handle_help_key(key));
                }
                if self.model_picker.open {
                    return self.handle_model_picker_key(key);
                }

                if opens_help(key, self.input.is_empty()) {
                    self.open_help(None);
                    return Ok(false);
                }

                if matches!(key.code, KeyCode::Esc) {
                    self.handle_escape_key();
                    return Ok(false);
                }

                if is_quit_key(key) {
                    return Ok(true);
                }

                match key.code {
                    KeyCode::Enter if inserts_newline(key) => {
                        self.input.insert_char('\n');
                    }
                    KeyCode::Enter => {
                        if self.submit(event_tx)? {
                            return Ok(true);
                        }
                    }
                    KeyCode::Backspace => self.input.backspace(),
                    KeyCode::Delete => self.input.delete(),
                    KeyCode::Left => self.input.move_left(),
                    KeyCode::Right => self.input.move_right(),
                    KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll = 0;
                    }
                    KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.scroll_to_bottom();
                    }
                    KeyCode::Home => self.input.move_to_line_start(),
                    KeyCode::End => self.input.move_to_line_end(),
                    KeyCode::PageUp => self.scroll_page_up(),
                    KeyCode::PageDown => self.scroll_page_down(),
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
                        self.input.clear();
                    }
                    KeyCode::Char('\u{15}') => self.input.clear(),
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.input.insert_char(c);
                    }
                    _ => {}
                }
            }
            Event::Paste(text) => {
                if self.help.open {
                    self.help.filter.insert_str(&text);
                } else {
                    self.input.insert_str(&text);
                }
            }
            _ => {}
        }

        Ok(false)
    }

    fn handle_help_key(&mut self, key: KeyEvent) -> bool {
        if matches!(key.code, KeyCode::Esc | KeyCode::F(1))
            || matches!(key.code, KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.help.open = false;
            self.help.filter.clear();
            self.help.scroll = 0;
            return false;
        }

        match key.code {
            KeyCode::Up => self.help.scroll = self.help.scroll.saturating_sub(1),
            KeyCode::Down => self.help.scroll = self.help.scroll.saturating_add(1),
            KeyCode::PageUp => self.help.scroll = self.help.scroll.saturating_sub(10),
            KeyCode::PageDown => self.help.scroll = self.help.scroll.saturating_add(10),
            KeyCode::Home => self.help.scroll = 0,
            KeyCode::End => self.help.scroll = u16::MAX,
            KeyCode::Backspace => {
                self.help.filter.backspace();
                self.help.scroll = 0;
            }
            KeyCode::Delete => {
                self.help.filter.delete();
                self.help.scroll = 0;
            }
            KeyCode::Left => self.help.filter.move_left(),
            KeyCode::Right => self.help.filter.move_right(),
            KeyCode::Char('a' | 'A') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help.filter.move_to_line_start();
            }
            KeyCode::Char('e' | 'E') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help.filter.move_to_line_end();
            }
            KeyCode::Char('u' | 'U') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help.filter.clear();
                self.help.scroll = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.help.filter.insert_char(c);
                self.help.scroll = 0;
            }
            _ => {}
        }
        false
    }

    fn handle_model_picker_key(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => self.close_model_picker(),
            KeyCode::Up => {
                self.model_picker.selected = self.model_picker.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let last_index = self.model_picker.models.len().saturating_sub(1);
                self.model_picker.selected = (self.model_picker.selected + 1).min(last_index);
            }
            KeyCode::Home => self.model_picker.selected = 0,
            KeyCode::End => {
                self.model_picker.selected = self.model_picker.models.len().saturating_sub(1);
            }
            KeyCode::PageUp => {
                self.model_picker.selected = self.model_picker.selected.saturating_sub(5);
            }
            KeyCode::PageDown => {
                let last_index = self.model_picker.models.len().saturating_sub(1);
                self.model_picker.selected = (self.model_picker.selected + 5).min(last_index);
            }
            KeyCode::Enter => self.apply_selected_model()?,
            _ => {}
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

        self.last_prompt = Some(prompt.clone());
        self.input.clear();
        self.messages.push(ChatMessage::user(prompt));
        let request_messages = self.request_messages();
        let assistant_index = self.messages.len();
        self.messages.push(ChatMessage::assistant(String::new()));
        self.assistant_index = Some(assistant_index);
        self.streaming = true;
        self.status = "Streaming from MiMo...".to_string();
        self.scroll_to_bottom();

        let stream_task = tokio::spawn(async move {
            let result = client
                .stream_chat(&request_messages, |delta| {
                    event_tx
                        .send(AppEvent::Delta(delta.to_string()))
                        .context("TUI closed")?;
                    Ok(())
                })
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
                self.input.clear();
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
                self.messages = session.messages;
                self.mode = session.mode;
                self.config.model = session.model;
                self.assistant_index = None;
                self.scroll_to_bottom();
                self.last_prompt = self.last_user_prompt();
                self.status = format!("Loaded session from {}", loaded_path.display());
                Ok(false)
            }
            SlashCommand::Sessions => {
                let sessions = session_store::list_sessions(&self.config)?;
                if sessions.is_empty() {
                    self.status = "No saved sessions yet".to_string();
                } else {
                    let mut output = String::from("Saved sessions\n\n");
                    for entry in sessions.into_iter().take(20) {
                        output.push_str(&format!(
                            "- {} (modified: {})\n",
                            entry.path.display(),
                            entry.modified_epoch
                        ));
                    }
                    self.push_system_message(output);
                    self.status = "Saved sessions listed".to_string();
                }
                Ok(false)
            }
            SlashCommand::Export { path } => {
                let export_path = session_store::export_markdown(
                    &self.config,
                    &self.config.model,
                    self.mode,
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
                    self.mode = mode.into();
                    self.status = format!("Mode switched to {}", self.mode);
                } else {
                    self.status = format!("Current mode: {}", self.mode);
                }
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
                "Invalid MiMo model id. Expected a non-empty id starting with mimo-".to_string();
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
                        "Invalid MiMo model id. Expected a non-empty id starting with mimo-"
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

    fn clear_conversation(&mut self) {
        if self.streaming {
            self.status = "Cannot clear while MiMo is responding".to_string();
            return;
        }

        self.messages.clear();
        self.assistant_index = None;
        self.last_prompt = None;
        self.scroll = 0;
        self.status = "Conversation cleared".to_string();
    }

    fn push_system_message(&mut self, content: String) {
        self.messages.push(ChatMessage::system(content));
        self.scroll_to_bottom();
    }

    fn request_messages(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::with_capacity(self.messages.len() + 2);
        messages.push(ChatMessage::system(self.config.system_prompt.clone()));
        if self.mode == AppMode::Plan {
            messages.push(ChatMessage::system(
                "You are in planning mode. Analyze the project, propose concrete implementation steps, and do not claim that files were modified or commands were executed unless the user explicitly switches back to chat mode.".to_string(),
            ));
        }
        messages.extend(self.messages.clone());
        messages
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Delta(delta) => {
                if let Some(index) = self.assistant_index {
                    if let Some(message) = self.messages.get_mut(index) {
                        message.content.push_str(&delta);
                    }
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
                if let Some(index) = self.assistant_index {
                    if let Some(message) = self.messages.get_mut(index) {
                        if message.content.is_empty() {
                            message.content = format!("Request failed: {error}");
                        }
                    }
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
        }
    }

    fn handle_escape_key(&mut self) {
        if self.streaming {
            self.cancel_active_stream();
        }
    }

    fn cancel_active_stream(&mut self) {
        if let Some(stream_task) = self.stream_task.take() {
            stream_task.abort();
        }
        self.streaming = false;
        if let Some(index) = self.assistant_index {
            if let Some(message) = self.messages.get_mut(index) {
                if message.content.trim().is_empty() {
                    message.content = "Request cancelled.".to_string();
                }
            }
        }
        self.assistant_index = None;
        self.status = "Generation cancelled".to_string();
        self.scroll_to_bottom();
    }

    fn open_help(&mut self, topic: Option<&str>) {
        self.help.open = true;
        self.help.scroll = 0;
        self.help.filter.set_text(topic.unwrap_or_default());
    }

    fn open_model_picker(&mut self, event_tx: UnboundedSender<AppEvent>) {
        self.refresh_model_picker_catalog();
        self.model_picker.scroll = 0;
        self.model_picker.open = true;
        self.refresh_models_from_api(event_tx);
    }

    fn close_model_picker(&mut self) {
        self.model_picker.open = false;
        self.model_picker.selected = 0;
        self.model_picker.scroll = 0;
        self.model_picker.loading = false;
        self.status = "Model picker closed".to_string();
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
        self.model_picker.open = false;
        self.model_picker.selected = 0;
        self.model_picker.scroll = 0;
        self.model_picker.loading = false;
        self.status = format!("Model switched to {model}");
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
        let desired = self.input.line_count().clamp(1, 6) as u16 + 2;
        desired.min(total_height.saturating_sub(4)).max(3)
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
        format!(
            "Configuration\n\nConfig file : {}\nBase URL    : {}\nModel       : {}\nTemperature : {}\nAPI key     : {}\nMode        : {}",
            self.config.config_path.display(),
            self.config.base_url,
            self.config.model,
            self.config.temperature,
            self.config.masked_api_key(),
            self.mode
        )
    }

    fn status_summary(&self) -> Result<String> {
        let cwd = env::current_dir()?;
        let session_count = session_store::list_sessions(&self.config)?.len();
        Ok(format!(
            "Status\n\nWorkspace    : {}\nMode         : {}\nModel        : {}\nStreaming    : {}\nMessages     : {}\nSaved files  : {}\nAPI key      : {}",
            cwd.display(),
            self.mode,
            self.config.model,
            if self.streaming { "yes" } else { "no" },
            self.messages.len(),
            session_count,
            self.config.masked_api_key(),
        ))
    }

    fn last_user_prompt(&self) -> Option<String> {
        self.messages
            .iter()
            .rev()
            .find_map(|message| matches!(message.role, Role::User).then(|| message.content.clone()))
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

fn inserts_newline(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::SHIFT)
        || matches!(key.code, KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT))
}

fn is_quit_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL))
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn test_app() -> App {
        App::new(AppConfig {
            api_key: None,
            base_url: "https://example.test/v1".to_string(),
            model: "mimo-v2-flash".to_string(),
            temperature: 0.2,
            system_prompt: "test".to_string(),
            config_path: PathBuf::from("config.toml"),
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
        assert!(summary.contains("chat"));
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
        assert!(!is_quit_key(KeyEvent::new(
            KeyCode::Esc,
            KeyModifiers::NONE
        )));
        assert!(is_quit_key(KeyEvent::new(
            KeyCode::Char('c'),
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
}
