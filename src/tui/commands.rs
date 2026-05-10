use std::fmt::Write;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandInfo {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct KeybindingInfo {
    pub chord: &'static str,
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
        aliases: &[],
        usage: "/sessions",
        description: "List local saved sessions.",
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
];

pub const KEYBINDINGS: &[KeybindingInfo] = &[
    KeybindingInfo {
        chord: "Enter",
        description: "Send the draft or execute a slash command.",
    },
    KeybindingInfo {
        chord: "Ctrl+J / Shift+Enter",
        description: "Insert a newline in the draft.",
    },
    KeybindingInfo {
        chord: "Ctrl+U",
        description: "Clear the current draft.",
    },
    KeybindingInfo {
        chord: "Left / Right",
        description: "Move the cursor inside the draft.",
    },
    KeybindingInfo {
        chord: "Home / End",
        description: "Jump to the start or end of the current line.",
    },
    KeybindingInfo {
        chord: "Ctrl+A / Ctrl+E",
        description: "Jump to the start or end of the current line.",
    },
    KeybindingInfo {
        chord: "Delete / Backspace",
        description: "Delete text around the cursor.",
    },
    KeybindingInfo {
        chord: "Up / Down",
        description: "Scroll the conversation or the help overlay.",
    },
    KeybindingInfo {
        chord: "PageUp / PageDown",
        description: "Scroll by one page.",
    },
    KeybindingInfo {
        chord: "Ctrl+Home / Ctrl+End",
        description: "Jump to the top or bottom of the conversation.",
    },
    KeybindingInfo {
        chord: "F1 / ?",
        description: "Open the searchable help overlay.",
    },
    KeybindingInfo {
        chord: "Esc / Ctrl+C",
        description: "Close help or quit MiMo TUI.",
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
        "sessions" => require_no_args(args, SlashCommand::Sessions, "/sessions"),
        "export" => Ok(SlashCommand::Export {
            path: non_empty(args).map(ToOwned::to_owned),
        }),
        "config" => parse_config_command(args),
        "status" => require_no_args(args, SlashCommand::Status, "/status"),
        "retry" => require_no_args(args, SlashCommand::Retry, "/retry"),
        "context" => require_no_args(args, SlashCommand::Context, "/context"),
        "mode" => parse_mode_command(args),
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

#[cfg(test)]
mod tests {
    use super::{CommandParseError, ConfigCommand, ModeName, SlashCommand, parse_slash_command};

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
