mod app;
mod attachments;
mod command_palette;
mod commands;
mod input;
mod keybindings;
mod markdown;
mod project_context;
mod session_picker;
pub mod session_store;
mod slash_menu;
mod state;
mod tooling;

use std::{io, time::Duration};

use anyhow::Result;
use crossterm::{
    event::{self},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc::unbounded_channel;

use crate::config::AppConfig;

use self::app::App;

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
