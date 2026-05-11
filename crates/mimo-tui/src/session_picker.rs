use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

use mimo_state::session_store::SessionEntry;

pub fn session_lines(entries: &[SessionEntry], selected: usize) -> Vec<Line<'static>> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let prefix = if index == selected { "> " } else { "  " };
            let mut style = Style::default();
            if index == selected {
                style = style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
            }
            Line::styled(
                format!(
                    "{prefix}{}  [{} | {} messages | saved {}]",
                    entry.title, entry.model, entry.message_count, entry.saved_at_epoch
                ),
                style,
            )
        })
        .collect()
}
