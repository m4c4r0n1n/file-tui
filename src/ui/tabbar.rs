//! The tab bar (only shown with more than one tab) and the header over
//! each side of a split view.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{app::App, ui::theme};

const MAX_TAB_W: u16 = 28;

pub fn render_tabbar(f: &mut Frame, app: &mut App, area: Rect) {
    app.tab_rects.clear();
    let n = app.tabs.len() as u16;
    let tab_w = (area.width / n.max(1)).clamp(8, MAX_TAB_W);
    f.render_widget(Paragraph::new("").style(Style::default().bg(theme::panel())), area);
    for i in 0..app.tabs.len() {
        let x = area.x + i as u16 * tab_w;
        if x + 4 > area.right() {
            break;
        }
        let rect = Rect { x, y: area.y, width: tab_w.min(area.right() - x), height: 1 };
        let active = i == app.active_tab;
        let style = if active {
            Style::default().bg(theme::selection()).fg(theme::text_strong()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(theme::panel()).fg(theme::muted())
        };
        let name_w = rect.width.saturating_sub(4) as usize;
        let mut name: String = app.tab_title(i);
        if name.chars().count() > name_w {
            name = name.chars().take(name_w.saturating_sub(1)).collect::<String>() + "\u{2026}";
        }
        let pad = name_w.saturating_sub(name.chars().count());
        let line = Line::from(vec![
            Span::styled(format!(" {name}{:pad$} ", ""), style),
            Span::styled("\u{f00d} ", style.remove_modifier(Modifier::BOLD).fg(theme::muted())),
        ]);
        f.render_widget(Paragraph::new(line), rect);
        let close = Rect { x: rect.right().saturating_sub(2), y: rect.y, width: 2, height: 1 };
        app.tab_rects.push((i, rect, close));
    }
}

/// One row over a split pane: its folder, bright on the side in use.
pub fn render_pane_header(f: &mut Frame, area: Rect, cwd: &std::path::Path, active: bool) {
    let style = if active {
        Style::default().bg(theme::panel()).fg(theme::accent()).add_modifier(Modifier::BOLD)
    } else {
        Style::default().bg(theme::panel()).fg(theme::muted())
    };
    let text = cwd.to_string_lossy().to_string();
    let w = area.width.saturating_sub(2) as usize;
    let count = text.chars().count();
    let shown = if count > w { format!("\u{2026}{}", text.chars().skip(count + 1 - w).collect::<String>()) } else { text };
    f.render_widget(Paragraph::new(format!(" {shown}")).style(style), area);
}
