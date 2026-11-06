use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, StatefulWidget, Widget},
};
use ratatui_image::{StatefulImage, picker::ProtocolType};

use crate::app::App;
use crate::events::{ClipOp, DragState};
use crate::ui::{kitty, theme::{self, rgb}};


/// Inset a content pane for a grid of `cell_w`-wide cells: at least one
/// padding column per side, leftover width split evenly so the grid sits
/// centered instead of piling the slack on the right.
pub fn centered_grid_area(area: Rect, cell_w: u16) -> Rect {
    let usable = area.width.saturating_sub(2);
    let cols = (usable / cell_w).max(1);
    let width = (cols * cell_w).min(area.width);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        width,
        ..area
    }
}

pub fn cell_at_pos(
    (col, row): (u16, u16),
    grid_area: Rect, (scroll_offset, sub): (usize, u16),
    cols: u16, entry_count: usize,
    (cell_w, cell_h): (u16, u16),
) -> Option<usize> {
    if col < grid_area.x || row < grid_area.y { return None; }
    if col >= grid_area.x + grid_area.width || row >= grid_area.y + grid_area.height { return None; }
    let cell_col = (col - grid_area.x) / cell_w;
    if cell_col >= cols { return None; }
    let cell_row = (row - grid_area.y + sub) / cell_h;
    let idx = (scroll_offset + cell_row as usize) * cols as usize + cell_col as usize;
    if idx < entry_count { Some(idx) } else { None }
}

/// The entry on a Details line (None for headers and empty space).
pub fn details_row_at_pos(
    row: u16, grid_area: Rect, scroll_offset: usize, rows: &[crate::ui::groups::DRow],
) -> Option<usize> {
    if row < grid_area.y + 1 { return None; }
    if row >= grid_area.y + grid_area.height { return None; }
    match rows.get(scroll_offset + (row - grid_area.y - 1) as usize) {
        Some(crate::ui::groups::DRow::Entry(i)) => Some(*i),
        _ => None,
    }
}

pub fn render_grid(f: &mut Frame, app: &mut App, area: Rect) {
    let cell_w = app.cell_w;
    let cell_h = app.cell_h;
    let icon_rows = app.icon_rows;

    f.render_widget(Block::default(), area);

    // Spread the leftover width between columns (Dolphin's layout) instead
    // of leaving it as margins; items keep their size within the pitch
    let usable = area.width.saturating_sub(2);
    let cols = (usable / cell_w).max(1);
    let pitch = (usable / cols).max(cell_w).min(area.width);
    let width = (cols * pitch).min(area.width);
    let area = Rect { x: area.x + area.width.saturating_sub(width) / 2, width, ..area };
    app.grid_cols = cols;
    app.grid_area = area;
    app.grid_pitch = pitch;
    let item_off = (pitch - cell_w) / 2;

    let entries_len = app.listing.entries.len();
    if entries_len == 0 {
        f.render_widget(
            Paragraph::new("Folder is empty").style(Style::default().fg(theme::muted())),
            Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: 1 },
        );
        return;
    }

    // Square-in-pixels icon box (see app::icon_dims)
    let img_w_cells = app.img_w.min(cell_w);
    let img_x_off = (cell_w.saturating_sub(img_w_cells)) / 2;

    let rows_visible = (area.height / cell_h + 2) as usize;
    // One column of breathing room on each side so neighbouring labels never touch
    let label_w = if cell_w >= 6 { cell_w as usize - 1 } else { cell_w as usize };

    app.layout_icons(cols);
    let hovered = if app.menu.is_none() && matches!(app.drag_state, DragState::Idle) {
        app.mouse_pos.and_then(|(c, r)| icon_at_pos(app, c, r))
    } else {
        None
    };

    // Rows are offset by the smooth-scroll position; items cut by the top
    // or bottom edge are drawn whole off-screen and only their visible rows
    // copied in (Kitty placeholders address each cell, so images clip too)
    let geo = CellGeo { cell_w, cell_h, icon_rows, img_w_cells, img_x_off, label_w };
    // Bands from the one at the top, shifted by how far into it we are
    let _ = rows_visible;
    let Some(first) = app.icon_bands.get(app.scroll_offset) else { return; };
    let origin = first.y as i32 + app.scroll_sub as i32;
    let renaming = app.rename_state.as_ref().map(|r| r.item_idx);
    let mut rename_cell = None;
    for bi in app.scroll_offset..app.icon_bands.len() {
        let band = app.icon_bands[bi].clone();
        let y = area.y as i32 + band.y as i32 - origin;
        if y >= area.bottom() as i32 { break; }
        if let Some(label) = &band.header {
            // Label and line at the bottom of the band (a gap row above)
            draw_group_header(f.buffer_mut(), area, y + band.h as i32 - 2, label);
            continue;
        }
        for (col_idx, entry_idx) in (band.start..band.end).enumerate() {
            let cell_x = area.x + col_idx as u16 * pitch + item_off;
            if renaming == Some(entry_idx) {
                rename_cell = Some((cell_x, y));
            }

            let top = y.max(area.y as i32) as u16;
            let bottom = (y + cell_h as i32).min(area.bottom() as i32) as u16;
            app.item_rects.push((entry_idx, Rect { x: cell_x, y: top, width: cell_w, height: bottom.saturating_sub(top) }));
            if y >= area.y as i32 && y + cell_h as i32 <= area.bottom() as i32 {
                let cell_rect = Rect { x: cell_x, y: y as u16, width: cell_w, height: cell_h };
                draw_cell(f.buffer_mut(), app, entry_idx, cell_rect, hovered, &geo);
                continue;
            }
            let cell_rect = Rect { x: cell_x, y: 0, width: cell_w, height: cell_h };
            let mut scratch = Buffer::empty(cell_rect);
            draw_cell(&mut scratch, app, entry_idx, cell_rect, hovered, &geo);
            let buf = f.buffer_mut();
            for r in 0..cell_h {
                let sy = y + r as i32;
                if sy < area.y as i32 || sy >= area.bottom() as i32 { continue; }
                for x in cell_x..cell_x + cell_w {
                    if let (Some(src), Some(dst)) = (scratch.cell((x, r)), buf.cell_mut((x, sy as u16))) {
                        *dst = src.clone();
                    }
                }
            }
        }
    }
    if let Some((x, y)) = rename_cell {
        draw_rename_box(f.buffer_mut(), app, area, x, y + icon_rows as i32, cell_w);
    }
}

/// The name being edited, whole: a field under the icon that grows down
/// over the items below as the name needs more lines, outlined so it's
/// plainly an edit and not just a selection.
fn draw_rename_box(buf: &mut Buffer, app: &App, area: Rect, cell_x: u16, label_y: i32, cell_w: u16) {
    let Some(rename) = &app.rename_state else { return; };
    let x = cell_x.saturating_sub(1).max(area.x);
    let right = (cell_x + cell_w + 1).min(area.right());
    let width = right - x;
    let text_w = width.saturating_sub(2) as usize;
    let mut shown = rename.input.clone();
    shown.insert(rename.cursor_pos.min(shown.len()), crate::lineedit::bar_cursor());
    let lines = wrap_label(&shown, text_w, 64);
    let height = (lines.len() as u16 + 1).min(area.height);
    // Kept on screen: pushed up if it would run off the bottom
    let y = label_y.min(area.bottom() as i32 - height as i32).max(area.y as i32) as u16;
    let rect = Rect { x, y, width, height };
    let field = if rename.invalid { theme::rgb(theme::get().error) } else { theme::input() };
    let style = Style::default().bg(field).fg(theme::text_strong());
    Clear.render(rect, buf);
    Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::accent()).bg(field))
        .style(style)
        .render(rect, buf);
    let inner = Rect { x: x + 1, y, width: width.saturating_sub(2), height: height.saturating_sub(1) };
    Paragraph::new(lines_to_text_centered(&lines, text_w)).style(style).render(inner, buf);
}

/// A group's header in Icons view: its label, then a thin line under it
/// across the view (rows outside `area` clipped).
fn draw_group_header(buf: &mut Buffer, area: Rect, y: i32, label: &str) {
    let x = area.x + 1;
    let w = area.width.saturating_sub(2);
    if (area.y as i32..area.bottom() as i32).contains(&y) {
        let text: String = label.chars().take(w as usize).collect();
        buf.set_string(x, y as u16, text, Style::default().fg(theme::text()).add_modifier(ratatui::style::Modifier::BOLD));
    }
    let line_y = y + 1;
    if (area.y as i32..area.bottom() as i32).contains(&line_y) {
        buf.set_string(x, line_y as u16, "\u{2500}".repeat(w as usize), Style::default().fg(theme::border()));
    }
}

/// The entry under a position in Icons view (None between items and on
/// group headers).
pub fn icon_at_pos(app: &App, col: u16, row: u16) -> Option<usize> {
    let area = app.grid_area;
    if !area.contains(ratatui::layout::Position { x: col, y: row }) { return None; }
    let first = app.icon_bands.get(app.scroll_offset)?;
    let y = first.y + app.scroll_sub as usize + (row - area.y) as usize;
    let band = app.icon_bands.get(app.band_at(y))?;
    if band.header.is_some() { return None; }
    let pitch = app.grid_pitch.max(1);
    let c = ((col - area.x) / pitch) as usize;
    let idx = band.start + c;
    if c >= app.grid_cols as usize || idx >= band.end {
        return None;
    }
    // Only the icon and the name's text count, as in Dolphin: the space
    // around them starts a rubber band
    let cell_w = app.cell_w;
    let x = ((col - area.x) % pitch) as i32 - (pitch.saturating_sub(cell_w) / 2) as i32;
    let ry = (y - band.y) as u16;
    if ry < app.icon_rows {
        let img_w = app.img_w.min(cell_w) as i32;
        let img_x = (cell_w as i32 - img_w) / 2;
        return (x >= img_x && x < img_x + img_w).then_some(idx);
    }
    let label_w = if cell_w >= 6 { cell_w as usize - 1 } else { cell_w as usize };
    let lines = wrap_label(&app.listing.entries[idx].name, label_w, app.cell_h.saturating_sub(app.icon_rows) as usize);
    let line = lines.get((ry - app.icon_rows) as usize)?;
    let w = unicode_width::UnicodeWidthStr::width(line.as_str()) as i32;
    let start = (cell_w as i32 - w).max(0) / 2;
    // A column of slack either side, so a short name isn't a tiny target
    (x >= start - 1 && x <= start + w).then_some(idx)
}

struct CellGeo {
    cell_w: u16,
    cell_h: u16,
    icon_rows: u16,
    img_w_cells: u16,
    img_x_off: u16,
    label_w: usize,
}

fn draw_cell(buf: &mut Buffer, app: &mut App, entry_idx: usize, cell_rect: Rect, hovered: Option<usize>, geo: &CellGeo) {
    let CellGeo { cell_w, cell_h, icon_rows, img_w_cells, img_x_off, label_w } = *geo;
    let (cell_x, cell_y) = (cell_rect.x, cell_rect.y);

    // Centered, square-ish image rect
    let icon_rect = Rect {
        x: cell_x + img_x_off,
        y: cell_y,
        width: img_w_cells,
        height: icon_rows,
    };

    let label_rect = Rect {
        x: cell_x,
        y: cell_y + icon_rows,
        width: cell_w,
        height: cell_h.saturating_sub(icon_rows),
    };

    let is_selected = app.selection == Some(entry_idx)
        || app.multi_selection.contains(&entry_idx);
    let is_drag_src = matches!(&app.drag_state,
        DragState::Armed { item_idx, .. } | DragState::Dragging { item_idx, .. }
        if *item_idx == entry_idx
    );
    let is_drag_target = matches!(&app.drag_state,
        DragState::Dragging { hover_target: Some(t), .. }
        if *t == entry_idx
    ) || app.dnd.hover_entry == Some(entry_idx);
    let is_cut = app.clipboard.as_ref()
        .map(|c| c.op == ClipOp::Cut
            && c.paths.contains(&app.listing.entries[entry_idx].path))
        .unwrap_or(false);

    // The icon image is composited onto the same bg color, so the
    // highlight runs behind the icon as one full-cell rectangle.
    let cell_bg: Option<(u8, u8, u8)> = if is_selected {
        Some(theme::get().selection)
    } else if is_drag_target {
        Some(theme::get().drop_target)
    } else if hovered == Some(entry_idx) {
        Some(theme::get().hover)
    } else {
        None
    };

    if let Some(bg) = cell_bg {
        Block::default().style(Style::default().bg(rgb(bg))).render(cell_rect, buf);
    } else if is_drag_src || is_cut {
        Block::default().style(Style::default().add_modifier(Modifier::DIM)).render(cell_rect, buf);
    }

    // Render icon/thumbnail image (skipped + placement-deleted under overlays)
    render_cell_image(buf, app, entry_idx, icon_rect, cell_bg);

    let entry = &app.listing.entries[entry_idx];
    let label_rows = cell_h.saturating_sub(icon_rows) as usize;

    // Inline rename
    if let Some(ref rename) = app.rename_state
        && rename.item_idx == entry_idx {
            // Thin bar marks the cursor inside the wrapped name
            let bg = if rename.invalid { theme::get().error } else { theme::get().selection };
            let style = Style::default().bg(rgb(bg)).fg(theme::text_strong()).add_modifier(Modifier::BOLD);
            let mut shown = rename.input.clone();
            shown.insert(rename.cursor_pos.min(shown.len()), crate::lineedit::bar_cursor());
            let lines = wrap_label(&shown, label_w, label_rows);
            Paragraph::new(lines_to_text_centered(&lines, cell_w as usize))
                .style(style)
                .render(label_rect, buf);
            return;
        }

    let label_fg = if is_selected {
        Style::default().bg(theme::selection()).fg(theme::text_strong())
    } else if is_drag_target {
        Style::default().bg(rgb(theme::get().drop_target)).fg(theme::drop_target_text())
    } else if hovered == Some(entry_idx) {
        Style::default().bg(theme::hover())
    } else {
        Style::default()
    };

    let label_fg = if app.is_cut(&entry.path) { label_fg.add_modifier(Modifier::DIM) } else { label_fg };
    let label_lines = wrap_label(&entry.name, label_w, label_rows);
    Paragraph::new(lines_to_text_styled_centered(&label_lines, label_fg, cell_w as usize)).render(label_rect, buf);
}

pub(crate) fn render_cell_image(
    buf: &mut Buffer,
    app: &mut App,
    entry_idx: usize,
    icon_rect: Rect,
    bg: Option<(u8, u8, u8)>,
) {
    if icon_rect.width == 0 || icon_rect.height == 0 { return; }

    let entry_path = app.listing.entries[entry_idx].path.clone();
    let is_media = app.listing.entries[entry_idx].mime_type.as_deref()
        .is_some_and(|m| app.previews.covers(m))
        && !app.thumb_cache.failed(&entry_path);

    let (cols, rows) = (icon_rect.width, icon_rect.height);
    let dim = app.is_cut(&entry_path);

    let drew = {
        let proto = if is_media {
            app.thumb_cache.get_mut(&app.picker, &entry_path, cols, rows, bg, dim)
        } else {
            app.icon_cache.get_mut(&app.picker, &entry_path, cols, rows, bg, dim)
        };
        if let Some(proto) = proto {
            Clear.render(icon_rect, buf);
            if let Some(c) = bg {
                Block::default().style(Style::default().bg(rgb(c))).render(icon_rect, buf);
            }
            StatefulImage::new().render(icon_rect, buf, proto);
            // Per-cell placeholders: overlays drawn later cover the image
            // cell by cell instead of the image having to be hidden.
            if app.picker.protocol_type() == ProtocolType::Kitty {
                kitty::split_placeholder_rows(buf, icon_rect, &mut app.kitty_transmits);
            }
            true
        } else {
            false
        }
    };

    if drew {
        app.frame_images.insert(entry_path, (icon_rect, bg, dim));
    } else if std::time::Instant::now() >= app.icons_grace_until {
        // Placeholder nerd font icon, centered in the cell
        let entry = &app.listing.entries[entry_idx];
        let placeholder = if entry.is_dir {
            "\u{f07b}"
        } else {
            file_placeholder_icon(entry.path.extension().and_then(|e| e.to_str()).unwrap_or(""))
        };
        // Center 2-wide glyph horizontally and vertically within icon_rect
        let px = icon_rect.x + icon_rect.width.saturating_sub(2) / 2;
        let py = icon_rect.y + icon_rect.height / 2;
        if px + 2 <= icon_rect.x + icon_rect.width {
            Paragraph::new(placeholder).style(Style::default().add_modifier(Modifier::DIM)).render(Rect { x: px, y: py, width: 2, height: 1 }, buf);
        }
    }
}

/// Wrap a name into at most `max_lines` lines of `max_cols` display-columns each.
/// Last line gets an ellipsis if the name is still too long.
fn wrap_label(name: &str, max_cols: usize, max_lines: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthChar;
    if max_cols == 0 || max_lines == 0 { return vec![]; }
    if name.is_empty() { return vec![String::new()]; }

    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut line_w = 0usize;
    let chars: Vec<char> = name.chars().collect();
    let total = chars.len();
    let mut i = 0;

    while i < total && lines.len() < max_lines {
        let ch = chars[i];
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        let is_last_line = lines.len() == max_lines - 1;

        if line_w + cw > max_cols {
            if is_last_line && i < total {
                // Truncate with ellipsis: replace last char with '…'
                while line_w + 1 > max_cols.saturating_sub(1) && !line.is_empty() {
                    if let Some(last) = line.pop() {
                        line_w -= UnicodeWidthChar::width(last).unwrap_or(1);
                    }
                }
                line.push('\u{2026}');
                lines.push(line);
                return lines;
            }
            lines.push(line.clone());
            line.clear();
            line_w = 0;
        }

        if line_w + cw <= max_cols {
            line.push(ch);
            line_w += cw;
            i += 1;
        }
    }

    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    }
    lines
}

/// Pad a string to be visually centered in `col_width` display columns.
fn center_line(s: &str, col_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let w = UnicodeWidthStr::width(s);
    if w >= col_width { return s.to_string(); }
    let pad = (col_width - w) / 2;
    format!("{:pad$}{s}", "")
}

fn lines_to_text_centered<'a>(lines: &'a [String], col_width: usize) -> Text<'a> {
    lines.iter()
        .map(|l| Line::raw(center_line(l, col_width)))
        .collect::<Vec<_>>().into()
}

fn lines_to_text_styled_centered<'a>(lines: &'a [String], style: Style, col_width: usize) -> Text<'a> {
    lines.iter()
        .map(|l| Line::styled(center_line(l, col_width), style))
        .collect::<Vec<_>>().into()
}

pub fn file_placeholder_icon_pub(ext: &str) -> &'static str {
    file_placeholder_icon(ext)
}

fn file_placeholder_icon(ext: &str) -> &'static str {
    match ext.to_lowercase().as_str() {
        "rs"                                           => "\u{e7a8}",
        "py"                                           => "\u{e73c}",
        "js" | "ts"                                    => "\u{e74e}",
        "html" | "htm"                                 => "\u{e736}",
        "css" | "scss"                                 => "\u{e749}",
        "json" | "toml" | "yaml" | "yml"               => "\u{e60b}",
        "md"                                           => "\u{f48a}",
        "pdf"                                          => "\u{f1c1}",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "\u{f03e}",
        "mp4" | "mkv" | "avi" | "mov" | "webm"        => "\u{f03d}",
        "mp3" | "flac" | "ogg" | "wav" | "aac"        => "\u{f001}",
        "zip" | "tar" | "gz" | "xz" | "bz2" | "zst"  => "\u{f1c6}",
        _                                              => "\u{f15b}",
    }
}



