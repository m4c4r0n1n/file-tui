//! A slim scrollbar at the right edge of the file view, like Dolphin's: a
//! rounded image placed to the pixel on Kitty, a ┃ bar elsewhere.

use ratatui::{Frame, layout::Rect, style::Style, widgets::Paragraph};

use crate::app::{App, ViewMode};
use crate::ui::{compact::COMPACT_CELL_H, theme, underlay};

/// Where the scrollbar is and what it spans, for clicks and drags.
#[derive(Debug, Clone, Copy)]
pub struct Geom {
    pub x: u16,
    pub top: u16,
    pub height: u16,
    /// Content and visible heights, and the scroll position, in lines.
    content: f32,
    viewport: f32,
}

/// (content, viewport, position, top row, track height), in terminal rows.
fn metrics(app: &App) -> (f32, f32, f32, u16, u16) {
    let area = app.grid_area;
    let n = app.listing.entries.len();
    let cols = app.grid_cols.max(1) as usize;
    match app.view_mode {
        ViewMode::Icons => (
            app.icons_height() as f32, area.height as f32,
            app.scroll_pos, area.y, area.height,
        ),
        ViewMode::Compact => (
            (n.div_ceil(cols) * COMPACT_CELL_H as usize) as f32, area.height as f32,
            (app.scroll_offset * COMPACT_CELL_H as usize) as f32, area.y, area.height,
        ),
        ViewMode::Details => {
            let h = area.height.saturating_sub(1);
            (app.details_rows.len().max(n) as f32, h as f32, app.scroll_offset as f32, area.y + 1, h)
        }
    }
}

pub fn render(f: &mut Frame, app: &mut App, main: Rect) {
    let (content, viewport, pos, top, height) = metrics(app);
    app.scrollbar = None;
    if content <= viewport || height < 2 || main.width < 4 {
        return;
    }
    let x = main.right() - 1;
    let track = height as f32;
    let thumb_h = (track * viewport / content).clamp(1.5, track);
    let thumb_y = (track - thumb_h) * (pos / (content - viewport)).clamp(0.0, 1.0);
    let color = if app.scrollbar_dragging { theme::get().highlight } else { theme::get().scrollbar };
    app.scrollbar = Some(Geom { x, top, height, content, viewport });

    let font = app.picker.font_size();
    let (cw, ch) = (font.width.max(1) as f32, font.height.max(1) as f32);
    let w = (cw * 0.4).round().max(3.0);
    let px = (x as f32 * cw + cw - w - (cw * 0.15).round()) as u32;
    let py = ((top as f32 + thumb_y) * ch).round() as u32;
    if underlay::bar(app, 6, (px, py, w as u32, (thumb_h * ch).round() as u32), color) {
        return;
    }
    let (start, len) = (thumb_y.round() as u16, thumb_h.round().max(1.0) as u16);
    f.render_widget(
        Paragraph::new("\u{2503}\n".repeat(len as usize)).style(Style::default().fg(theme::rgb(color))),
        Rect { x, y: top + start.min(height - 1), width: 1, height: len.min(height - start.min(height - 1)) },
    );
}

/// Scroll so the thumb centers on `row` (click or drag on the track).
pub fn scroll_to(app: &mut App, row: u16) {
    let Some(g) = app.scrollbar else { return; };
    let frac = (row.saturating_sub(g.top) as f32 + 0.5) / g.height as f32;
    let pos = (frac * g.content - g.viewport / 2.0).clamp(0.0, g.content - g.viewport);
    match app.view_mode {
        ViewMode::Icons => {
            app.scroll_target = pos;
            app.scroll_pos = pos;
        }
        ViewMode::Compact => app.scroll_offset = (pos / COMPACT_CELL_H as f32).round() as usize,
        ViewMode::Details => app.scroll_offset = pos.round() as usize,
    }
}

/// Whether a press at this cell is on the scrollbar.
pub fn hit(app: &App, col: u16, row: u16) -> bool {
    app.scrollbar.is_some_and(|g| col == g.x && row >= g.top && row < g.top + g.height)
}

