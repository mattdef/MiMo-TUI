use super::{commands, mcp_store::McpServerConfig, skill_store::InstalledSkill, state::AppMode};

#[derive(Debug, Clone)]
pub enum PaletteAction {
    InsertCommand(String),
    ExecuteCommand(String),
    OpenHelp,
    OpenModels,
    OpenSessions,
    ShowStatus,
    ShowLastMessage,
    ShowDiagnostics,
    BrowseTasks,
    BrowseSnapshots,
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

pub fn filtered_entries(
    filter: &str,
    mode: AppMode,
    installed_skills: &[InstalledSkill],
    active_skills: &[String],
    mcp_servers: &[McpServerConfig],
) -> Vec<PaletteEntry> {
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
            label: "Show diagnostics".to_string(),
            hint: "Open the cached diagnostics report".to_string(),
            action: PaletteAction::ShowDiagnostics,
        },
        PaletteEntry {
            label: "Browse background tasks".to_string(),
            hint: "Inspect durable MiMo tasks in a dedicated overlay".to_string(),
            action: PaletteAction::BrowseTasks,
        },
        PaletteEntry {
            label: "Browse workspace snapshots".to_string(),
            hint: "Inspect tracked restore snapshots in a dedicated overlay".to_string(),
            action: PaletteAction::BrowseSnapshots,
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
            label: "Switch to plan mode".to_string(),
            hint: if mode == AppMode::Plan {
                "Current mode".to_string()
            } else {
                "Read-only planning mode".to_string()
            },
            action: PaletteAction::SwitchMode(AppMode::Plan),
        },
        PaletteEntry {
            label: "Switch to agent mode".to_string(),
            hint: if mode == AppMode::Agent {
                "Current mode".to_string()
            } else {
                "Prompt for mutating tool approvals".to_string()
            },
            action: PaletteAction::SwitchMode(AppMode::Agent),
        },
        PaletteEntry {
            label: "Switch to yolo mode".to_string(),
            hint: if mode == AppMode::Yolo {
                "Current mode".to_string()
            } else {
                "Auto-approve tool execution".to_string()
            },
            action: PaletteAction::SwitchMode(AppMode::Yolo),
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

    if !installed_skills.is_empty() {
        entries.push(PaletteEntry {
            label: "List installed skills".to_string(),
            hint: "Show installed and active local skills".to_string(),
            action: PaletteAction::ExecuteCommand("/skills".to_string()),
        });
    }
    entries.extend(installed_skills.iter().map(|skill| PaletteEntry {
        label: format!(
            "{} skill {}",
            if active_skills.iter().any(|active| active == &skill.name) {
                "Deactivate"
            } else {
                "Activate"
            },
            skill.name
        ),
        hint: skill.description.clone(),
        action: PaletteAction::ExecuteCommand(format!("/skill {}", skill.name)),
    }));

    entries.push(PaletteEntry {
        label: "List MCP servers".to_string(),
        hint: "Inspect configured local MCP definitions".to_string(),
        action: PaletteAction::ExecuteCommand("/mcp list".to_string()),
    });
    entries.extend(mcp_servers.iter().map(|server| PaletteEntry {
        label: format!(
            "{} MCP {}",
            if server.enabled {
                "Inspect"
            } else {
                "Inspect disabled"
            },
            server.name
        ),
        hint: format!("{} {}", server.transport, server.summary()),
        action: PaletteAction::ExecuteCommand(format!("/mcp show {}", server.name)),
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
