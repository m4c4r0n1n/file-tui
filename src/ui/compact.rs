use ratatui::{
    Frame,
    layout::Rect,
    style::{Style},
    widgets::{Block, Paragraph},
};

use crate::app::App;
use crate::ui::{grid::centered_grid_area, theme};

pub fn compact_cell_w() -> u16 {
    crate::app::layout().compact_width
}
pub const COMPACT_CELL_H: u16 = 2;

pub fn render_compact(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(Block::default(), area);

    let area = centered_grid_area(area, compact_cell_w());
    let cols = (area.width / compact_cell_w()).max(1);
    app.grid_cols = cols;
    app.grid_area = area;

    let entries_len = app.listing.entries.len();
    let rows_visible = (area.height / COMPACT_CELL_H + 2) as usize;

    for row_idx in 0..rows_visible {
        let abs_row = app.scroll_offset + row_idx;
        for col_idx in 0..cols as usize {
            let entry_idx = abs_row * cols as usize + col_idx;
            if entry_idx >= entries_len { break; }

            let cell_x = area.x + col_idx as u16 * compact_cell_w();
            let cell_y = area.y + row_idx as u16 * COMPACT_CELL_H;
            if cell_y >= area.y + area.height { break; }

            app.item_rects.push((entry_idx, Rect { x: cell_x + 1, y: cell_y, width: compact_cell_w() - 1, height: 1 }));
            let entry = &app.listing.entries[entry_idx];
            let is_selected = app.selection == Some(entry_idx)
                || app.multi_selection.contains(&entry_idx);

            let icon = if entry.is_dir {
                "\u{f07b}"
            } else {
                crate::ui::grid::file_placeholder_icon_pub(
                    entry.path.extension().and_then(|e| e.to_str()).unwrap_or("")
                )
            };

            // Cell = 1-col gutter + " icon name " highlighted as one block
            let inner_w = compact_cell_w() as usize - 1;
            let name = truncate_name(&entry.name, inner_w - 4);
            let text = format!(" {icon} {name}");

            let style = if is_selected {
                Style::default().bg(theme::selection()).fg(theme::text_strong())
            } else {
                Style::default()
            };
            let style = if app.is_cut(&entry.path) { style.add_modifier(ratatui::style::Modifier::DIM) } else { style };

            f.render_widget(
                Paragraph::new(text).style(style),
                Rect { x: cell_x + 1, y: cell_y, width: inner_w as u16, height: 1 },
            );
        }
    }
}

fn truncate_name(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        name.to_string()
    } else {
        let mut s: String = chars[..max.saturating_sub(1)].iter().collect();
        s.push('\u{2026}');
        s
    }
}
