use std::fmt::Write;

use super::keybindings::KEYBINDINGS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeName {
    Chat,
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
    Mode { mode: Option<ModeName> },
    Plan(PlanCommand),
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
        usage: "/model [id]",
        description: "Show or switch the active MiMo model.",
    },
    CommandInfo {
        name: "models",
        aliases: &[],
        usage: "/models",
        description: "Open the MiMo model picker and refresh it from the API when possible.",
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
        description: "Show or update local MiMo-TUI configuration.",
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
        name: "mode",
        aliases: &[],
        usage: "/mode [chat|plan]",
        description: "Show or switch the current MiMo interaction mode.",
    },
    CommandInfo {
        name: "plan",
        aliases: &[],
        usage: "/plan [show|add <text>|done <n>|undo <n>|remove <n>|clear]",
        description: "Inspect or update the local plan checklist.",
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
        "mode" => parse_mode_command(args),
        "plan" => parse_plan_command(args),
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
        "chat" => ModeName::Chat,
        "plan" => ModeName::Plan,
        _ => return Err(CommandParseError::Usage("/mode [chat|plan]")),
    };
    Ok(SlashCommand::Mode { mode: Some(mode) })
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
        CommandParseError, ConfigCommand, ModeName, PlanCommand, SlashCommand, parse_slash_command,
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
    }

    #[test]
    fn reports_usage_errors() {
        assert_eq!(
            parse_slash_command("/clear now"),
            Err(CommandParseError::Usage("/clear"))
        );
        assert_eq!(
            parse_slash_command("/mode fast"),
            Err(CommandParseError::Usage("/mode [chat|plan]"))
        );
    }
}
