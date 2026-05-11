use mimo_tui_core::commands::{COMMANDS, CommandInfo};

#[derive(Debug, Clone)]
pub struct SlashMenuEntry {
    pub command: &'static CommandInfo,
}

impl SlashMenuEntry {
    pub fn insertion_text(&self) -> String {
        if self.command.usage.contains('<') || self.command.usage.contains('[') {
            format!("/{} ", self.command.name)
        } else {
            format!("/{}", self.command.name)
        }
    }
}

pub fn visible_entries(input: &str) -> Vec<SlashMenuEntry> {
    let Some(query) = current_query(input) else {
        return Vec::new();
    };
    COMMANDS
        .iter()
        .filter(|command| matches_query(command, query))
        .map(|command| SlashMenuEntry { command })
        .collect()
}

pub fn is_active(input: &str) -> bool {
    current_query(input).is_some()
}

pub fn autocomplete_input(input: &str) -> Option<String> {
    let query = current_query(input)?;
    let entries = visible_entries(input);
    if entries.is_empty() {
        return None;
    }

    if let Some(exact) = entries.iter().find(|entry| {
        entry.command.name.eq_ignore_ascii_case(query)
            || entry
                .command
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(query))
    }) {
        return Some(exact.insertion_text());
    }

    if entries.len() == 1 {
        return Some(entries[0].insertion_text());
    }

    let names = entries
        .iter()
        .map(|entry| entry.command.name)
        .collect::<Vec<_>>();
    let prefix = longest_common_prefix(&names);
    (prefix.len() > query.len()).then(|| format!("/{prefix}"))
}

fn current_query(input: &str) -> Option<&str> {
    let trimmed = input.trim_start();
    let command = trimmed.strip_prefix('/')?;
    if command.contains(char::is_whitespace) {
        return None;
    }
    Some(command)
}

fn matches_query(command: &CommandInfo, query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    query.is_empty()
        || command.name.contains(&query)
        || command
            .usage
            .trim_start_matches('/')
            .to_ascii_lowercase()
            .starts_with(&query)
        || command
            .aliases
            .iter()
            .any(|alias| alias.to_ascii_lowercase().contains(&query))
}

fn longest_common_prefix(names: &[&str]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for name in names.iter().skip(1) {
        while !name.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                return prefix;
            }
        }
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::{autocomplete_input, visible_entries};

    #[test]
    fn filters_known_commands() {
        let entries = visible_entries("/mo");
        assert!(entries.iter().any(|entry| entry.command.name == "model"));
        assert!(entries.iter().any(|entry| entry.command.name == "mode"));
    }

    #[test]
    fn completes_unique_command_with_space() {
        assert_eq!(autocomplete_input("/cle"), Some("/clear".to_string()));
        assert_eq!(autocomplete_input("/mode"), Some("/mode ".to_string()));
    }
}
