use super::{commands, state::AppMode};

#[derive(Debug, Clone)]
pub enum PaletteAction {
    InsertCommand(String),
    OpenHelp,
    OpenModels,
    OpenSessions,
    ShowStatus,
    ShowLastMessage,
    BrowseDraftHistory,
    BrowseDraftStash,
    SwitchMode(AppMode),
}

#[derive(Debug, Clone)]
pub struct PaletteEntry {
    pub label: String,
    pub hint: String,
    pub action: PaletteAction,
}

pub fn filtered_entries(filter: &str, mode: AppMode) -> Vec<PaletteEntry> {
    let mut entries = vec![
        PaletteEntry {
            label: "Open help".to_string(),
            hint: "Search commands and keybindings".to_string(),
            action: PaletteAction::OpenHelp,
        },
        PaletteEntry {
            label: "Open model picker".to_string(),
            hint: "Browse MiMo models".to_string(),
            action: PaletteAction::OpenModels,
        },
        PaletteEntry {
            label: "Open session picker".to_string(),
            hint: "Resume a saved conversation".to_string(),
            action: PaletteAction::OpenSessions,
        },
        PaletteEntry {
            label: "Show status".to_string(),
            hint: "Inspect runtime state".to_string(),
            action: PaletteAction::ShowStatus,
        },
        PaletteEntry {
            label: "Show last message".to_string(),
            hint: "Open the latest message in a pager".to_string(),
            action: PaletteAction::ShowLastMessage,
        },
        PaletteEntry {
            label: "Browse cleared drafts".to_string(),
            hint: "Recover a prompt cleared with Ctrl+U".to_string(),
            action: PaletteAction::BrowseDraftHistory,
        },
        PaletteEntry {
            label: "Browse stashed drafts".to_string(),
            hint: "Recover a prompt stashed with Ctrl+S".to_string(),
            action: PaletteAction::BrowseDraftStash,
        },
        PaletteEntry {
            label: if mode == AppMode::Plan {
                "Switch to chat mode".to_string()
            } else {
                "Switch to plan mode".to_string()
            },
            hint: "Toggle the active MiMo mode".to_string(),
            action: PaletteAction::SwitchMode(if mode == AppMode::Plan {
                AppMode::Chat
            } else {
                AppMode::Plan
            }),
        },
    ];

    entries.extend(commands::COMMANDS.iter().map(|command| PaletteEntry {
        label: format!("/{}", command.name),
        hint: command.description.to_string(),
        action: PaletteAction::InsertCommand(
            if command.usage.contains('<') || command.usage.contains('[') {
                format!("/{} ", command.name)
            } else {
                format!("/{}", command.name)
            },
        ),
    }));

    let filter = filter.trim().to_ascii_lowercase();
    entries
        .into_iter()
        .filter(|entry| {
            filter.is_empty()
                || entry.label.to_ascii_lowercase().contains(&filter)
                || entry.hint.to_ascii_lowercase().contains(&filter)
        })
        .collect()
}
