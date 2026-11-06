use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::app::App;
use crate::ui::theme;

/// Keys that aren't configurable, and the mouse.
const FIXED: &[(&str, &str)] = &[
    ("Arrows", "Move selection (Shift extends)"),
    ("Enter", "Open file or enter folder"),
    ("Esc", "Clear selection / close search"),
    ("Double-click", "Open"),
    ("Right-click", "Context menu"),
    ("Drag", "Move (Ctrl copies) / select"),
];

/// Help rows: the bound keys per action (from the keymap), then the fixed ones.
fn help_lines(app: &App) -> Vec<(String, String)> {
    let mut lines = vec![("Keys".to_string(), String::new())];
    for (action, combos) in &app.keymap.bound {
        if combos.is_empty() {
            continue;
        }
        let desc = crate::keys::ACTIONS.iter().find(|a| a.1 == *action).map_or("", |a| a.3);
        let keys: Vec<String> = combos.iter().map(|c| c.label()).collect();
        lines.push((format!("  {}", keys.join(", ")), desc.to_string()));
    }
    lines.push((String::new(), String::new()));
    lines.push(("Fixed".to_string(), String::new()));
    lines.extend(FIXED.iter().map(|(k, d)| (format!("  {k}"), d.to_string())));
    lines
}

/// Geometry of the help popup (shared with the occlusion pre-pass).
pub fn help_rect(app: &App, term: Rect) -> Rect {
    let width: u16 = 60u16.min(term.width);
    let height: u16 = (help_lines(app).len() as u16 + 2).min(term.height);
    let x = term.width.saturating_sub(width) / 2;
    let y = term.height.saturating_sub(height) / 2;
    Rect { x, y, width, height }
}

pub fn render_help(f: &mut Frame, app: &App) {
    let area = help_rect(app, f.area());
    f.render_widget(Clear, area);

    let lines: Vec<Line> = help_lines(app).into_iter().map(|(key, desc)| {
        if key.is_empty() {
            Line::raw("")
        } else if desc.is_empty() {
            // Section header
            Line::from(Span::styled(key, Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)))
        } else {
            Line::from(vec![
                Span::styled(format!("{:<26}", key), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(desc),
            ])
        }
    }).collect();

    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(theme::panel()))
            .block(Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::border()))
                .title(" Keys (any key closes) ")),
        area,
    );
}
