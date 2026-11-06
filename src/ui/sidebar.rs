use std::path::PathBuf;

use ratatui::{
    Frame,
    layout::Rect,
    buffer::CellDiffOption,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};
use ratatui_image::{StatefulImage, picker::ProtocolType};

use crate::app::App;
use crate::events::SidebarTarget;
use crate::ui::{kitty, theme};

/// Rows per place. On Kitty the name is centered across both (text sizing
/// protocol); elsewhere it sits on the first. A device's usage bar goes on
/// the second, under its name.
const ITEM_H: u16 = 2;
/// Icon height in rows, centered in the item.
pub const ICON_ROWS: f32 = 2.0;

/// Places panel in Dolphin's layout: muted section headers, a theme icon
/// per row, and a usage bar under each device.
pub fn render_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let inner = Rect { x: area.x + 1, y: area.y, width: area.width.saturating_sub(2), height: area.height };
    app.sidebar_area = inner;

    // Resize handle: a short grabber mid-height (the whole edge column
    // drags, see handler), brighter on hover
    if area.width > 0 && area.height > 0 {
        let x = area.right() - 1;
        let hot = app.sidebar_dragging || app.mouse_pos.is_some_and(|(c, r)| c == x && r >= area.y);
        let color = if hot { theme::accent() } else { theme::border() };
        let h = area.height.min(3);
        let grip = Rect { x, y: area.y + (area.height - h) / 2, width: 1, height: h };
        f.render_widget(Paragraph::new(vec![Line::from("\u{2503}"); h as usize]).style(Style::default().fg(color)), grip);
    }

    let hover = app.mouse_pos.and_then(|(c, r)| target_at(app, c, r));
    app.eject_rects.clear();

    let sections = sections(app);
    // Kitty's centered names are written after the frame, over anything
    // drawn on top of them: rows under a menu or dialog get plain text
    let menu_rects = app.menu.as_ref()
        .map(|m| crate::ui::menu::covered(m, f.area(), false && app.picker.protocol_type() == ProtocolType::Kitty))
        .unwrap_or_default();
    let dialog = app.show_help || app.properties.is_some() || app.name_prompt.is_some() || app.conflict_prompt.is_some()
        || app.confirm_delete.is_some();

    let font = app.picker.font_size();
    let kitty = app.picker.protocol_type() == ProtocolType::Kitty;
    let icon_w = (ICON_ROWS * font.height as f32 / font.width.max(1) as f32).ceil() as u16;
    // Roomy rows when they fit, tighter ones on short terminals
    let fixed: u16 = sections.iter().map(|(_, r)| r.iter().filter(|r| r.usage.is_some()).count() as u16).sum::<u16>()
        + sections.len() as u16 * 2;
    let items = sections.iter().map(|(_, r)| r.len() as u16).sum::<u16>().max(1);
    let item_h = (1..=ITEM_H).rev()
        .find(|h| fixed + items * h <= inner.height)
        .unwrap_or(1);
    let width = inner.width;
    let bottom = inner.y + inner.height;
    let mut y = inner.y;
    let mut row_map: Vec<Option<SidebarTarget>> = Vec::new();

    for (si, (title, rows)) in sections.iter().enumerate() {
        if si > 0 && y < bottom {
            y += 1;
            row_map.push(None);
        }
        if y >= bottom { break; }
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {title}"), Style::default().fg(theme::muted()))),
            Rect { x: inner.x, y, width, height: 1 },
        );
        y += 1;
        row_map.push(None);

        for row in rows {
            // A device needs a row under its name for the usage bar
            let h = if row.usage.is_some() { item_h.max(2) } else { item_h };
            if y + h > bottom { break; }
            let rect = Rect { x: inner.x, y, width, height: h };
            let drag = app.place_drag.filter(|d| d.active);
            let is_drop = matches!((drag, row.target),
                (Some(d), SidebarTarget::Place(j)) if d.target == Some(j) && d.from != j)
                || matches!((&app.drag_state, row.target),
                    (crate::events::DragState::Dragging { hover_place: Some(i), .. }, SidebarTarget::Place(j)) if *i == j);
            let bg = if is_drop {
                Some(theme::get().hover)
            } else if row.current {
                Some(theme::get().selection)
            } else if hover == Some(row.target) {
                Some(theme::get().hover)
            } else {
                None
            };
            if let Some(c) = bg {
                f.render_widget(Block::default().style(Style::default().bg(theme::rgb(c))), rect);
            }
            // Drop target of a place being dragged: accent bar on its left
            if is_drop {
                f.render_widget(
                    Paragraph::new(vec![Line::from("\u{258c}"); h as usize]).style(Style::default().fg(theme::accent())),
                    Rect { width: 1, ..rect },
                );
            }
            let fg = if row.dim { theme::get().muted } else if row.current { theme::text_strong_rgb() } else { theme::get().text };

            draw_icon(f, app, &row.icon, Rect { x: inner.x + 1, y, width: icon_w, height: h }, bg);
            let text_x = inner.x + 1 + icon_w + 1;
            let mut text_w = (inner.x + width).saturating_sub(text_x + 1);
            if row.eject && let SidebarTarget::Device(i) = row.target && text_w > 6 {
                // Two columns at the far right, a row tall, centered
                let r = Rect { x: inner.x + width - 3, y: y + (h - 1) / 2, width: 2, height: 1 };
                let hot = app.mouse_pos.is_some_and(|(c, rr)| c >= r.x && c < r.right() && rr == r.y);
                let eject_bg = if hot { Some(theme::get().highlight) } else { bg };
                if hot {
                    f.render_widget(Block::default().style(Style::default().bg(theme::rgb(theme::get().highlight))), r);
                }
                draw_icon(f, app, "media-eject", r, eject_bg);
                app.eject_rects.push((r, i));
                text_w -= 3;
            }
            let name: String = row.name.chars().take(text_w as usize).collect();
            let name_y = y;
            let name_rect = Rect { x: text_x, y, width: text_w, height: h };
            let covered = dialog || menu_rects.iter().any(|m| m.intersects(name_rect));
            if false && kitty && !covered {
                // Kitty can center normal-size text across both rows
                let (seq, used) = kitty::centered_text(text_x, y, h, text_w, &name, fg, bg);
                let buf = f.buffer_mut();
                for cy in y..y + h {
                    for cx in text_x..text_x + used {
                        buf[(cx, cy)].set_symbol(" ").set_diff_option(CellDiffOption::Skip);
                    }
                }
                app.post_draw.push(seq);
            } else {
                f.render_widget(
                    Paragraph::new(Span::styled(name, Style::default().fg(theme::rgb(fg)))),
                    Rect { x: text_x, y: name_y, width: text_w, height: 1 },
                );
            }

            if let Some(usage) = row.usage {
                f.render_widget(Paragraph::new(usage_bar(usage, text_w)), Rect { x: text_x, y: name_y + 1, width: text_w, height: 1 });
            }
            for _ in 0..h {
                row_map.push(Some(row.target));
            }
            y += h;
        }
    }
    app.sidebar_rows = row_map;
}

struct Row {
    target: SidebarTarget,
    icon: String,
    name: String,
    current: bool,
    dim: bool,
    usage: Option<f32>,
    /// An unmountable device: an eject button at the right end.
    eject: bool,
}

/// The sidebar's sections and rows, as drawn.
fn sections(app: &App) -> Vec<(&'static str, Vec<Row>)> {
    let cwd = &app.cwd;
    let mut sections: Vec<(&'static str, Vec<Row>)> = Vec::new();

    let places = app.places.iter().enumerate().map(|(i, p)| Row {
        target: SidebarTarget::Place(i),
        icon: place_icon(&p.path, p.icon.as_deref()),
        name: p.name.clone(),
        current: std::path::Path::new(&p.path) == cwd,
        dim: false,
        usage: None,
        eject: false,
    }).collect();
    sections.push(("Places", places));

    let mut devices: Vec<Row> = app.devices.iter().enumerate().map(|(i, d)| Row {
        target: SidebarTarget::Device(i),
        icon: d.icon().into(),
        name: d.name.clone(),
        current: std::path::Path::new(&d.path) == cwd,
        dim: false,
        usage: d.usage,
        eject: d.can_unmount(),
    }).collect();
    devices.extend(app.kdeconnect_devices.iter().enumerate().map(|(i, d)| Row {
        target: SidebarTarget::Phone(i),
        icon: if d.reachable { "smartphone".into() } else { "smartphonedisconnected".into() },
        name: d.name.clone(),
        current: app.kdeconnect_mounts.get(&d.id).is_some_and(|m| cwd.starts_with(m)),
        dim: !d.reachable,
        usage: None,
        eject: false,
    }));
    if !devices.is_empty() {
        sections.push(("Devices", devices));
    }

    sections
}

/// Ask for every icon the first frame shows (sidebar, location), so
/// startup can wait for them instead of drawing without.
pub fn request_icons(app: &mut App) {
    if app.text_mode {
        return;
    }
    let mut names: Vec<String> = sections(app).into_iter().flat_map(|(_, rows)| rows).map(|r| r.icon).collect();
    names.push(crate::ui::topbar::location_icon(app));
    for name in names {
        request_icon(app, &name);
    }
}

/// Thin two-tone bar hugging the top of its row, right under the device
/// name (Dolphin's free-space indicator); red when nearly full.
fn usage_bar(usage: f32, width: u16) -> Line<'static> {
    let filled = ((usage.clamp(0.0, 1.0) * width as f32).round() as usize).min(width as usize);
    let color = if usage > 0.9 { theme::rgb(theme::get().error) } else { theme::accent() };
    Line::from(vec![
        // Upper one-quarter block (kitty draws these itself)
        Span::styled("\u{1fb82}".repeat(filled), Style::default().fg(color)),
        Span::styled("\u{1fb82}".repeat(width as usize - filled), Style::default().fg(theme::border())),
    ])
}

/// Draw a theme icon by name (cached separately from file icons).
fn text_glyph(name: &str) -> &'static str {
    let name = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
    if has(&["trash", "recycle"]) { "\u{f1f8}" }
    else if has(&["home"]) { "\u{f015}" }
    else if has(&["download"]) { "\u{f019}" }
    else if has(&["music"]) { "\u{f001}" }
    else if has(&["picture"]) { "\u{f03e}" }
    else if has(&["video"]) { "\u{f03d}" }
    else if has(&["document"]) { "\u{f15c}" }
    else if has(&["develop"]) { "\u{f121}" }
    else if has(&["eject"]) { "\u{f052}" }
    else if has(&["find"]) { "\u{f002}" }
    else if has(&["drive", "disk", "phone"]) { "\u{f0a0}" }
    else { "\u{f07b}" }
}

pub fn draw_icon(f: &mut Frame, app: &mut App, name: &str, rect: Rect, bg: Option<(u8, u8, u8)>) {
    if app.text_mode {
        let at = Rect { y: rect.y + rect.height.saturating_sub(1) / 2, height: 1, ..rect };
        let mut style = Style::default().fg(theme::accent());
        if let Some(c) = bg {
            style = style.bg(theme::rgb(c));
        }
        f.render_widget(Paragraph::new(text_glyph(name)).alignment(ratatui::layout::Alignment::Center).style(style), at);
        return;
    }
    // Sidebar icons are keyed by theme name, in their own cache
    let key = request_icon(app, name);
    let Some(proto) = app.sidebar_icons.get_mut(&app.picker, &key, rect.width, rect.height, bg, false) else { return; };
    f.render_widget(Clear, rect);
    if let Some(c) = bg {
        f.render_widget(Block::default().style(Style::default().bg(theme::rgb(c))), rect);
    }
    f.render_stateful_widget(StatefulImage::new(), rect, proto);
    if app.picker.protocol_type() == ProtocolType::Kitty {
        kitty::split_placeholder_rows(f.buffer_mut(), rect, &mut app.kitty_transmits);
    }
}

fn request_icon(app: &mut App, name: &str) -> PathBuf {
    let key = PathBuf::from(format!("sidebar-icon/{name}"));
    let px = (app.picker.font_size().height as f32 * ICON_ROWS) as u32;
    app.sidebar_icons.request(&key, true, px.max(24), Some(name.to_string()));
    key
}

/// Map a mouse position to a sidebar target using the rendered row map.
pub fn target_at(app: &App, col: u16, row: u16) -> Option<SidebarTarget> {
    let area = app.sidebar_area;
    if col + 1 < area.x || col > area.x + area.width { return None; }
    if row < area.y || row >= area.y + area.height { return None; }
    app.sidebar_rows.get((row - area.y) as usize).copied().flatten()
}

/// Whether a position is anywhere inside the sidebar (for empty-area menus).
pub fn contains(app: &App, col: u16, row: u16) -> bool {
    let area = app.sidebar_area;
    col + 1 >= area.x && col < area.x + area.width + 1 && row + 1 >= area.y && row < area.y + area.height
}

/// Theme icon name for a place: its custom folder icon, the configured
/// hint, then XDG folders.
fn place_icon(path: &str, hint: Option<&str>) -> String {
    // A custom folder icon (folder_icons) wins, matching the grid
    if let Some(custom) = crate::icons::resolver::folder_icon(std::path::Path::new(path)) {
        return custom;
    }
    if let Some(h) = hint {
        return h.to_string();
    }
    let p = std::path::Path::new(path);
    let xdg = [
        (dirs::home_dir(), "user-home"),
        (dirs::desktop_dir(), "user-desktop"),
        (dirs::download_dir(), "folder-download"),
        (dirs::audio_dir(), "folder-music"),
        (dirs::picture_dir(), "folder-pictures"),
        (dirs::video_dir(), "folder-videos"),
        (dirs::document_dir(), "folder-documents"),
        (dirs::template_dir(), "folder-templates"),
        (dirs::public_dir(), "folder-publicshare"),
    ];
    if let Some((_, name)) = xdg.iter().find(|(d, _)| d.as_deref() == Some(p)) {
        return name.to_string();
    }
    if path.contains("Trash") { "user-trash".into() } else { "folder".into() }
}
