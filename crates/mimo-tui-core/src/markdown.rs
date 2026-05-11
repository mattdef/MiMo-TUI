use ratatui::{
    style::{Color, Modifier, Style},
    text::Line,
};

pub fn render_markdown_lines(content: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;

    for raw_line in content.lines() {
        let trimmed = raw_line.trim_start();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            lines.push(Line::styled(
                format!("  {raw_line}"),
                Style::default().fg(Color::Yellow),
            ));
            continue;
        }

        let style = if in_code_block {
            Style::default().fg(Color::Yellow)
        } else if trimmed.starts_with('#') {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            Style::default().fg(Color::Green)
        } else if trimmed.starts_with('>') {
            Style::default().fg(Color::Magenta)
        } else {
            Style::default()
        };

        lines.push(Line::styled(format!("  {raw_line}"), style));
    }

    if lines.is_empty() {
        lines.push(Line::raw("  "));
    }

    lines
}
