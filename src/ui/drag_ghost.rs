use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::ui::theme;
use crate::{app::App, events::DragState};

/// Geometry of the drag ghost (shared with the occlusion pre-pass).
pub fn ghost_rect(app: &App, term: Rect) -> Option<Rect> {
    let DragState::Dragging { item_idx, current_pos: (cx, cy), .. } = &app.drag_state else {
        return None;
    };
    let name = app.listing.entries.get(*item_idx).map(|e| e.name.as_str()).unwrap_or("???");
    let width = name.chars().count().min(24) as u16 + 4;
    let height = 3u16;
    let x = cx.saturating_sub(width / 2).min(term.width.saturating_sub(width));
    let y = cy.saturating_sub(1).min(term.height.saturating_sub(height));
    Some(Rect { x, y, width, height })
}

pub fn render_drag_ghost(f: &mut Frame, app: &App) {
    let Some(area) = ghost_rect(app, f.area()) else { return; };
    let DragState::Dragging { item_idx, hover_target, .. } = &app.drag_state else { return; };

    let name = app.listing.entries
        .get(*item_idx)
        .map(|e| e.name.as_str())
        .unwrap_or("???");
    let display = format!(" {} ", name);

    let (bg, fg) = if hover_target.is_some() {
        (theme::rgb(theme::get().drop_target), theme::drop_target_text())  // valid drop target
    } else {
        (theme::rgb(theme::get().ghost), theme::text_strong())   // neutral ghost
    };

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(display)
            .style(Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().bg(bg).fg(fg))),
        area,
    );
}
