use std::fmt::Write;

use super::keybindings::KEYBINDINGS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeName {
    Agent,
    Plan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlashCommand {
    Help { topic: Option<String> },
    Clear,
    Exit,
    Model { model: Option<String> },
    Models,
    Save { path: Option<String> },
    Load { path: Option<String> },
    Sessions,
    Export { path: Option<String> },
    Config(ConfigCommand),
    Status,
    Retry,
    Context,
    Branch { message_index: Option<usize> },
    Branches,
    Switch { branch_id: String },
    Mode { mode: Option<ModeName> },
    Plan(PlanCommand),
    Jobs(JobsCommand),
    Task(TaskCommand),
    Diff,
    Undo,
    Restore { id: Option<String> },
    Note { text: String },
    Memory(MemoryCommand),
    Recall { query: String },
    Compact,
    Review(ReviewCommand),
    Lsp(LspCommand),
    Skills,
    Skill(SkillCommand),
    Mcp(McpCommand),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigCommand {
    Show,
    SetApiKey(String),
    ClearApiKey,
    SetBaseUrl(String),
    SetModel(String),
    SetTemperature(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanCommand {
    Show,
    Add(String),
    Done(usize),
    Undo(usize),
    Remove(usize),
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryCommand {
    Show,
    Path,
    Clear,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobsCommand {
    List,
    Show(String),
    Poll(String),
    Wait(String),
    Stdin { id: String, input: String },
    Cancel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCommand {
    Add(String),
    List,
    Show(String),
    Cancel(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewCommand {
    Workspace,
    Staged,
    Path(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspCommand {
    Status,
    Run,
    Show,
    Clear,
    On,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillCommand {
    Toggle(String),
    Install(String),
    Show(String),
    Uninstall(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpCommand {
    List,
    Show(String),
    AddStdio {
        name: String,
        command: String,
        args: Vec<String>,
    },
    AddHttp {
        name: String,
        url: String,
    },
    Enable(String),
    Disable(String),
    Remove(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandParseError {
    NotACommand,
    UnknownCommand(String),
    Usage(&'static str),
}

pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        aliases: &["?"],
        usage: "/help [command]",
        description: "Show command help or open the help overlay.",
    },
    CommandInfo {
        name: "clear",
        aliases: &[],
        usage: "/clear",
        description: "Clear the current conversation history.",
    },
    CommandInfo {
        name: "exit",
        aliases: &["quit", "q"],
        usage: "/exit",
        description: "Quit MiMo TUI.",
    },
    CommandInfo {
        name: "model",
        aliases: &[],
        usage: "/model [id|auto]",
        description: "Show or switch the current mode's MiMo model.",
    },
    CommandInfo {
        name: "models",
        aliases: &[],
        usage: "/models",
        description: "Open the MiMo model picker for the current mode and refresh it from the API when possible.",
    },
    CommandInfo {
        name: "save",
        aliases: &[],
        usage: "/save [path]",
        description: "Save the current conversation to a local JSON session file.",
    },
    CommandInfo {
        name: "load",
        aliases: &[],
        usage: "/load [path]",
        description: "Load a local JSON session file, or the most recent saved session.",
    },
    CommandInfo {
        name: "sessions",
        aliases: &["resume"],
        usage: "/sessions",
        description: "Open the local saved-session picker.",
    },
    CommandInfo {
        name: "export",
        aliases: &[],
        usage: "/export [path]",
        description: "Export the current conversation as Markdown.",
    },
    CommandInfo {
        name: "config",
        aliases: &[],
        usage: "/config [show|api-key <key>|clear-api-key|base-url <url>|model <id>|temperature <value>]",
        description: "Show or update local MiMo-TUI configuration, including the current mode's model.",
    },
    CommandInfo {
        name: "status",
        aliases: &[],
        usage: "/status",
        description: "Show current runtime state and configuration summary.",
    },
    CommandInfo {
        name: "retry",
        aliases: &[],
        usage: "/retry",
        description: "Resend the last non-command prompt.",
    },
    CommandInfo {
        name: "context",
        aliases: &[],
        usage: "/context",
        description: "Summarize the current project directory in read-only form.",
    },
    CommandInfo {
        name: "branch",
        aliases: &["b"],
        usage: "/branch [message-number]",
        description: "Create a new conversation branch from a visible message.",
    },
    CommandInfo {
        name: "branches",
        aliases: &[],
        usage: "/branches",
        description: "List conversation branches.",
    },
    CommandInfo {
        name: "switch",
        aliases: &[],
        usage: "/switch <branch-id>",
        description: "Switch to a conversation branch.",
    },
    CommandInfo {
        name: "mode",
        aliases: &[],
        usage: "/mode [agent|plan]",
        description: "Show or switch the current MiMo interaction mode.",
    },
    CommandInfo {
        name: "plan",
        aliases: &[],
        usage: "/plan [show|add <text>|done <n>|undo <n>|remove <n>|clear]",
        description: "Inspect or update the local plan checklist.",
    },
    CommandInfo {
        name: "jobs",
        aliases: &["job"],
        usage: "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
        description: "Inspect or control background shell jobs started by tools.",
    },
    CommandInfo {
        name: "task",
        aliases: &["tasks"],
        usage: "/task [add <prompt>|list|show <id>|cancel <id>]",
        description: "Queue or inspect durable background MiMo tasks.",
    },
    CommandInfo {
        name: "diff",
        aliases: &[],
        usage: "/diff",
        description: "Show the current workspace diff and tracked restore snapshots.",
    },
    CommandInfo {
        name: "undo",
        aliases: &[],
        usage: "/undo",
        description: "Restore the latest tracked workspace snapshot.",
    },
    CommandInfo {
        name: "restore",
        aliases: &[],
        usage: "/restore [snapshot-id]",
        description: "Restore a tracked workspace snapshot by id, or the latest one.",
    },
    CommandInfo {
        name: "note",
        aliases: &[],
        usage: "/note <text>",
        description: "Add a persistent memory note that is injected into later requests.",
    },
    CommandInfo {
        name: "memory",
        aliases: &[],
        usage: "/memory [show|path|clear|help]",
        description: "Inspect or clear the persistent MiMo memory file.",
    },
    CommandInfo {
        name: "recall",
        aliases: &[],
        usage: "/recall <query>",
        description: "Search the current transcript and memory notes for a query.",
    },
    CommandInfo {
        name: "compact",
        aliases: &[],
        usage: "/compact",
        description: "Compact older transcript messages into a local summary.",
    },
    CommandInfo {
        name: "review",
        aliases: &[],
        usage: "/review [workspace|staged|path <path>]",
        description: "Show a lightweight review context from git diff plus cached diagnostics.",
    },
    CommandInfo {
        name: "lsp",
        aliases: &[],
        usage: "/lsp [status|run|show|clear|on|off]",
        description: "Manage cached diagnostics and automatic refresh after file-tool edits.",
    },
    CommandInfo {
        name: "skills",
        aliases: &[],
        usage: "/skills",
        description: "List installed local skills and show which ones are active.",
    },
    CommandInfo {
        name: "skill",
        aliases: &[],
        usage: "/skill [name|install <path-or-url>|show <name>|uninstall <name>]",
        description: "Toggle, install, inspect, or uninstall a local skill.",
    },
    CommandInfo {
        name: "mcp",
        aliases: &[],
        usage: "/mcp [list|show <name>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>]",
        description: "Manage local MCP server definitions for future tool integrations.",
    },
];

pub fn parse_slash_command(input: &str) -> Result<SlashCommand, CommandParseError> {
    let Some((name, args)) = command_parts(input) else {
        return Err(CommandParseError::NotACommand);
    };
    let normalized_name = name.to_ascii_lowercase();
    match normalized_name.as_str() {
        "help" | "?" => Ok(SlashCommand::Help {
            topic: non_empty(args).map(ToOwned::to_owned),
        }),
        "clear" => require_no_args(args, SlashCommand::Clear, "/clear"),
        "exit" | "quit" | "q" => require_no_args(args, SlashCommand::Exit, "/exit"),
        "model" => Ok(SlashCommand::Model {
            model: non_empty(args).map(ToOwned::to_owned),
        }),
        "models" => require_no_args(args, SlashCommand::Models, "/models"),
        "save" => Ok(SlashCommand::Save {
            path: non_empty(args).map(ToOwned::to_owned),
        }),
        "load" => Ok(SlashCommand::Load {
            path: non_empty(args).map(ToOwned::to_owned),
        }),
        "sessions" | "resume" => require_no_args(args, SlashCommand::Sessions, "/sessions"),
        "export" => Ok(SlashCommand::Export {
            path: non_empty(args).map(ToOwned::to_owned),
        }),
        "config" => parse_config_command(args),
        "status" => require_no_args(args, SlashCommand::Status, "/status"),
        "retry" => require_no_args(args, SlashCommand::Retry, "/retry"),
        "context" => require_no_args(args, SlashCommand::Context, "/context"),
        "branch" | "b" => parse_branch_command(args),
        "branches" => require_no_args(args, SlashCommand::Branches, "/branches"),
        "switch" => parse_switch_command(args),
        "mode" => parse_mode_command(args),
        "plan" => parse_plan_command(args),
        "jobs" | "job" => parse_jobs_command(args),
        "task" | "tasks" => parse_task_command(args),
        "diff" => require_no_args(args, SlashCommand::Diff, "/diff"),
        "undo" => require_no_args(args, SlashCommand::Undo, "/undo"),
        "restore" => Ok(SlashCommand::Restore {
            id: non_empty(args).map(ToOwned::to_owned),
        }),
        "note" => parse_note_command(args),
        "memory" => parse_memory_command(args),
        "recall" => parse_recall_command(args),
        "compact" => require_no_args(args, SlashCommand::Compact, "/compact"),
        "review" => parse_review_command(args),
        "lsp" => parse_lsp_command(args),
        "skills" => require_no_args(args, SlashCommand::Skills, "/skills"),
        "skill" => parse_skill_command(args),
        "mcp" => parse_mcp_command(args),
        _ => Err(CommandParseError::UnknownCommand(name.to_string())),
    }
}

pub fn command_help(topic: Option<&str>) -> String {
    if let Some(topic) = topic.and_then(non_empty) {
        if let Some(info) = find_command(topic) {
            let mut help = format!(
                "{}\n\n{}\n\nUsage: {}",
                info.name, info.description, info.usage
            );
            if !info.aliases.is_empty() {
                let _ = write!(help, "\nAliases: {}", info.aliases.join(", "));
            }
            return help;
        }
        return format!("Unknown command /{topic}");
    }

    let mut output = String::from("MiMo TUI commands\n\n");
    for info in COMMANDS {
        let _ = writeln!(output, "{:<14} {}", info.usage, info.description);
    }
    output
}

pub fn help_overlay_lines(filter: &str) -> Vec<String> {
    let mut lines = vec!["Commands".to_string()];
    for command in filtered_commands(filter) {
        let mut line = format!("  {:<16} {}", command.usage, command.description);
        if !command.aliases.is_empty() {
            let _ = write!(line, "  [{}]", command.aliases.join(", "));
        }
        lines.push(line);
    }

    lines.push(String::new());
    lines.push("Keybindings".to_string());
    let filter = filter.to_ascii_lowercase();
    for binding in KEYBINDINGS {
        if filter.is_empty()
            || binding.chord.to_ascii_lowercase().contains(&filter)
            || binding.description.to_ascii_lowercase().contains(&filter)
        {
            lines.push(format!("  {:<16} {}", binding.chord, binding.description));
        }
    }
    lines
}

pub fn find_command(name: &str) -> Option<&'static CommandInfo> {
    let name = name.trim_start_matches('/').to_ascii_lowercase();
    COMMANDS.iter().find(|command| {
        command.name == name
            || command
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&name))
    })
}

fn filtered_commands(filter: &str) -> Vec<&'static CommandInfo> {
    let filter = filter.to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| {
            filter.is_empty()
                || command.name.contains(&filter)
                || command.usage.to_ascii_lowercase().contains(&filter)
                || command.description.to_ascii_lowercase().contains(&filter)
                || command
                    .aliases
                    .iter()
                    .any(|alias| alias.to_ascii_lowercase().contains(&filter))
        })
        .collect()
}

fn parse_config_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("show") {
        return Ok(SlashCommand::Config(ConfigCommand::Show));
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or_default().to_ascii_lowercase();
    let value = parts.next().unwrap_or_default().trim();
    match name.as_str() {
        "api-key" if !value.is_empty() => Ok(SlashCommand::Config(ConfigCommand::SetApiKey(
            value.to_string(),
        ))),
        "clear-api-key" if value.is_empty() => Ok(SlashCommand::Config(ConfigCommand::ClearApiKey)),
        "base-url" if !value.is_empty() => Ok(SlashCommand::Config(ConfigCommand::SetBaseUrl(
            value.to_string(),
        ))),
        "model" if !value.is_empty() => Ok(SlashCommand::Config(ConfigCommand::SetModel(
            value.to_string(),
        ))),
        "temperature" if !value.is_empty() => Ok(SlashCommand::Config(
            ConfigCommand::SetTemperature(value.to_string()),
        )),
        _ => Err(CommandParseError::Usage(
            "/config [show|api-key <key>|clear-api-key|base-url <url>|model <id>|temperature <value>]",
        )),
    }
}

fn parse_mode_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let Some(value) = non_empty(args) else {
        return Ok(SlashCommand::Mode { mode: None });
    };
    let mode = match value.to_ascii_lowercase().as_str() {
        "plan" => ModeName::Plan,
        "agent" | "chat" | "2" => ModeName::Agent,
        "1" => ModeName::Plan,
        _ => return Err(CommandParseError::Usage("/mode [agent|plan]")),
    };
    Ok(SlashCommand::Mode { mode: Some(mode) })
}

fn parse_branch_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return Ok(SlashCommand::Branch {
            message_index: None,
        });
    }

    if trimmed.split_whitespace().count() != 1 {
        return Err(CommandParseError::Usage("/branch [message-number]"));
    }

    let message_index = trimmed
        .parse::<usize>()
        .map_err(|_| CommandParseError::Usage("/branch [message-number]"))?;
    Ok(SlashCommand::Branch {
        message_index: Some(message_index),
    })
}

fn parse_switch_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().count() != 1 {
        return Err(CommandParseError::Usage("/switch <branch-id>"));
    }

    Ok(SlashCommand::Switch {
        branch_id: trimmed.to_string(),
    })
}

fn parse_plan_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("show") {
        return Ok(SlashCommand::Plan(PlanCommand::Show));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let value = parts.next().unwrap_or_default().trim();

    let command = match action.as_str() {
        "add" if !value.is_empty() => PlanCommand::Add(value.to_string()),
        "done" => PlanCommand::Done(parse_index(value)?),
        "undo" => PlanCommand::Undo(parse_index(value)?),
        "remove" => PlanCommand::Remove(parse_index(value)?),
        "clear" if value.is_empty() => PlanCommand::Clear,
        _ => {
            return Err(CommandParseError::Usage(
                "/plan [show|add <text>|done <n>|undo <n>|remove <n>|clear]",
            ));
        }
    };

    Ok(SlashCommand::Plan(command))
}

fn parse_note_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let Some(text) = non_empty(args) else {
        return Err(CommandParseError::Usage("/note <text>"));
    };
    Ok(SlashCommand::Note {
        text: text.to_string(),
    })
}

fn parse_jobs_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
        return Ok(SlashCommand::Jobs(JobsCommand::List));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let remainder = parts.next().unwrap_or_default().trim();
    let command = match action.as_str() {
        "show" if !remainder.is_empty() => JobsCommand::Show(remainder.to_string()),
        "poll" if !remainder.is_empty() => JobsCommand::Poll(remainder.to_string()),
        "wait" if !remainder.is_empty() => JobsCommand::Wait(remainder.to_string()),
        "cancel" if !remainder.is_empty() => JobsCommand::Cancel(remainder.to_string()),
        "stdin" => {
            let mut parts = remainder.splitn(2, char::is_whitespace);
            let id = parts.next().unwrap_or_default().trim();
            let input = parts.next().unwrap_or_default();
            if id.is_empty() || input.is_empty() {
                return Err(CommandParseError::Usage(
                    "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
                ));
            }
            JobsCommand::Stdin {
                id: id.to_string(),
                input: input.to_string(),
            }
        }
        _ => {
            return Err(CommandParseError::Usage(
                "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
            ));
        }
    };
    Ok(SlashCommand::Jobs(command))
}

fn parse_task_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
        return Ok(SlashCommand::Task(TaskCommand::List));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let value = parts.next().unwrap_or_default().trim();
    let command = match action.as_str() {
        "add" if !value.is_empty() => TaskCommand::Add(value.to_string()),
        "show" if !value.is_empty() => TaskCommand::Show(value.to_string()),
        "cancel" if !value.is_empty() => TaskCommand::Cancel(value.to_string()),
        _ => {
            return Err(CommandParseError::Usage(
                "/task [add <prompt>|list|show <id>|cancel <id>]",
            ));
        }
    };
    Ok(SlashCommand::Task(command))
}

fn parse_memory_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let command = match args.trim().to_ascii_lowercase().as_str() {
        "" | "show" => MemoryCommand::Show,
        "path" => MemoryCommand::Path,
        "clear" => MemoryCommand::Clear,
        "help" => MemoryCommand::Help,
        _ => return Err(CommandParseError::Usage("/memory [show|path|clear|help]")),
    };
    Ok(SlashCommand::Memory(command))
}

fn parse_recall_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let Some(query) = non_empty(args) else {
        return Err(CommandParseError::Usage("/recall <query>"));
    };
    Ok(SlashCommand::Recall {
        query: query.to_string(),
    })
}

fn parse_review_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("workspace") {
        return Ok(SlashCommand::Review(ReviewCommand::Workspace));
    }
    if trimmed.eq_ignore_ascii_case("staged") {
        return Ok(SlashCommand::Review(ReviewCommand::Staged));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let value = parts.next().unwrap_or_default().trim();
    match action.as_str() {
        "path" if !value.is_empty() => {
            Ok(SlashCommand::Review(ReviewCommand::Path(value.to_string())))
        }
        _ => Err(CommandParseError::Usage(
            "/review [workspace|staged|path <path>]",
        )),
    }
}

fn parse_lsp_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let command = match args.trim().to_ascii_lowercase().as_str() {
        "" | "status" => LspCommand::Status,
        "run" => LspCommand::Run,
        "show" => LspCommand::Show,
        "clear" => LspCommand::Clear,
        "on" => LspCommand::On,
        "off" => LspCommand::Off,
        _ => {
            return Err(CommandParseError::Usage(
                "/lsp [status|run|show|clear|on|off]",
            ));
        }
    };
    Ok(SlashCommand::Lsp(command))
}

fn parse_skill_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    let Some(value) = non_empty(trimmed) else {
        return Err(CommandParseError::Usage(
            "/skill [name|install <path-or-url>|show <name>|uninstall <name>]",
        ));
    };
    let mut parts = value.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let remainder = parts.next().unwrap_or_default().trim();
    let command = match action.as_str() {
        "install" if !remainder.is_empty() => SkillCommand::Install(remainder.to_string()),
        "show" if !remainder.is_empty() => SkillCommand::Show(remainder.to_string()),
        "uninstall" if !remainder.is_empty() => SkillCommand::Uninstall(remainder.to_string()),
        _ => SkillCommand::Toggle(value.to_string()),
    };
    Ok(SlashCommand::Skill(command))
}

fn parse_mcp_command(args: &str) -> Result<SlashCommand, CommandParseError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") {
        return Ok(SlashCommand::Mcp(McpCommand::List));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or_default().to_ascii_lowercase();
    let remainder = parts.next().unwrap_or_default().trim();
    let command = match action.as_str() {
        "show" if !remainder.is_empty() => McpCommand::Show(remainder.to_string()),
        "enable" if !remainder.is_empty() => McpCommand::Enable(remainder.to_string()),
        "disable" if !remainder.is_empty() => McpCommand::Disable(remainder.to_string()),
        "remove" if !remainder.is_empty() => McpCommand::Remove(remainder.to_string()),
        "add" => parse_mcp_add_command(remainder)?,
        _ => {
            return Err(CommandParseError::Usage(
                "/mcp [list|show <name>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>]",
            ));
        }
    };
    Ok(SlashCommand::Mcp(command))
}

fn parse_mcp_add_command(args: &str) -> Result<McpCommand, CommandParseError> {
    let mut parts = args.split_whitespace();
    let transport = parts.next().unwrap_or_default().to_ascii_lowercase();
    let name = parts.next().unwrap_or_default();
    if name.is_empty() {
        return Err(CommandParseError::Usage(
            "/mcp [list|show <name>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>]",
        ));
    }
    match transport.as_str() {
        "stdio" => {
            let command = parts.next().unwrap_or_default();
            if command.is_empty() {
                return Err(CommandParseError::Usage(
                    "/mcp [list|show <name>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>]",
                ));
            }
            Ok(McpCommand::AddStdio {
                name: name.to_string(),
                command: command.to_string(),
                args: parts.map(ToOwned::to_owned).collect(),
            })
        }
        "http" => {
            let url = parts.next().unwrap_or_default();
            if url.is_empty() {
                return Err(CommandParseError::Usage(
                    "/mcp [list|show <name>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>]",
                ));
            }
            Ok(McpCommand::AddHttp {
                name: name.to_string(),
                url: url.to_string(),
            })
        }
        _ => Err(CommandParseError::Usage(
            "/mcp [list|show <name>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>]",
        )),
    }
}

fn command_parts(input: &str) -> Option<(&str, &str)> {
    let command = input.trim().strip_prefix('/')?;
    let command = command.trim_start();
    let Some(index) = command.find(char::is_whitespace) else {
        return Some((command, ""));
    };
    Some((&command[..index], command[index..].trim()))
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn require_no_args(
    args: &str,
    command: SlashCommand,
    usage: &'static str,
) -> Result<SlashCommand, CommandParseError> {
    if args.trim().is_empty() {
        Ok(command)
    } else {
        Err(CommandParseError::Usage(usage))
    }
}

fn parse_index(value: &str) -> Result<usize, CommandParseError> {
    value.parse::<usize>().map_err(|_| {
        CommandParseError::Usage("/plan [show|add <text>|done <n>|undo <n>|remove <n>|clear]")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CommandParseError, ConfigCommand, JobsCommand, LspCommand, McpCommand, MemoryCommand,
        ModeName, PlanCommand, ReviewCommand, SkillCommand, SlashCommand, TaskCommand,
        parse_slash_command,
    };

    #[test]
    fn parses_core_commands() {
        assert_eq!(
            parse_slash_command("/help clear"),
            Ok(SlashCommand::Help {
                topic: Some("clear".to_string())
            })
        );
        assert_eq!(
            parse_slash_command("/Mode plan"),
            Ok(SlashCommand::Mode {
                mode: Some(ModeName::Plan)
            })
        );
        assert_eq!(
            parse_slash_command("/config model mimo-v2-flash"),
            Ok(SlashCommand::Config(ConfigCommand::SetModel(
                "mimo-v2-flash".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/plan add Build slash menu"),
            Ok(SlashCommand::Plan(PlanCommand::Add(
                "Build slash menu".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/memory path"),
            Ok(SlashCommand::Memory(MemoryCommand::Path))
        );
        assert_eq!(
            parse_slash_command("/branch"),
            Ok(SlashCommand::Branch {
                message_index: None
            })
        );
        assert_eq!(
            parse_slash_command("/branch 2"),
            Ok(SlashCommand::Branch {
                message_index: Some(2)
            })
        );
        assert_eq!(
            parse_slash_command("/b 2"),
            Ok(SlashCommand::Branch {
                message_index: Some(2)
            })
        );
        assert_eq!(parse_slash_command("/branches"), Ok(SlashCommand::Branches));
        assert_eq!(
            parse_slash_command("/switch branch-2"),
            Ok(SlashCommand::Switch {
                branch_id: "branch-2".to_string()
            })
        );
        assert_eq!(
            parse_slash_command("/jobs stdin shell-1 y{enter}"),
            Ok(SlashCommand::Jobs(JobsCommand::Stdin {
                id: "shell-1".to_string(),
                input: "y{enter}".to_string()
            }))
        );
        assert_eq!(
            parse_slash_command("/task add Review the latest diff"),
            Ok(SlashCommand::Task(TaskCommand::Add(
                "Review the latest diff".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/restore snapshot-2"),
            Ok(SlashCommand::Restore {
                id: Some("snapshot-2".to_string())
            })
        );
        assert_eq!(parse_slash_command("/skills"), Ok(SlashCommand::Skills));
        assert_eq!(
            parse_slash_command("/skill install ./skills/review.md"),
            Ok(SlashCommand::Skill(SkillCommand::Install(
                "./skills/review.md".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/skill review"),
            Ok(SlashCommand::Skill(SkillCommand::Toggle(
                "review".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/mcp add stdio repo-lsp rust-analyzer --stdio"),
            Ok(SlashCommand::Mcp(McpCommand::AddStdio {
                name: "repo-lsp".to_string(),
                command: "rust-analyzer".to_string(),
                args: vec!["--stdio".to_string()],
            }))
        );
        assert_eq!(
            parse_slash_command("/mcp enable repo-lsp"),
            Ok(SlashCommand::Mcp(McpCommand::Enable(
                "repo-lsp".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/review staged"),
            Ok(SlashCommand::Review(ReviewCommand::Staged))
        );
        assert_eq!(
            parse_slash_command("/review path src/main.rs"),
            Ok(SlashCommand::Review(ReviewCommand::Path(
                "src/main.rs".to_string()
            )))
        );
        assert_eq!(
            parse_slash_command("/lsp"),
            Ok(SlashCommand::Lsp(LspCommand::Status))
        );
        assert_eq!(
            parse_slash_command("/lsp on"),
            Ok(SlashCommand::Lsp(LspCommand::On))
        );
    }

    #[test]
    fn reports_usage_errors() {
        assert_eq!(
            parse_slash_command("/clear now"),
            Err(CommandParseError::Usage("/clear"))
        );
        assert_eq!(
            parse_slash_command("/mode fast"),
            Err(CommandParseError::Usage("/mode [agent|plan]"))
        );
        assert_eq!(
            parse_slash_command("/mode 3"),
            Err(CommandParseError::Usage("/mode [agent|plan]"))
        );
        assert_eq!(
            parse_slash_command("/branch nope"),
            Err(CommandParseError::Usage("/branch [message-number]"))
        );
        assert_eq!(
            parse_slash_command("/branch 1 extra"),
            Err(CommandParseError::Usage("/branch [message-number]"))
        );
        assert_eq!(
            parse_slash_command("/switch"),
            Err(CommandParseError::Usage("/switch <branch-id>"))
        );
        assert_eq!(
            parse_slash_command("/switch branch-1 extra"),
            Err(CommandParseError::Usage("/switch <branch-id>"))
        );
    }
}
