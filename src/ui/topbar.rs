use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph},
};

use std::path::{Path, PathBuf};

use crate::app::{App, TOPBAR_H};
use crate::ui::menu::{self, MenuKind};
use crate::ui::{theme, underlay};

fn path_bg() -> Color { theme::input() }

/// Dolphin-style toolbar on the window background: icon buttons, a rounded
/// location field with the folder's icon and clickable breadcrumbs, then
/// search and sort.
pub fn render_topbar(f: &mut Frame, app: &mut App, area: Rect) {
    let row = Rect { x: area.x, y: area.y + area.height / 2, width: area.width, height: 1 };

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(2),   // left pad
            Constraint::Length(3),   // back
            Constraint::Length(2),   // gap
            Constraint::Length(3),   // forward
            Constraint::Length(2),   // gap
            Constraint::Length(3),   // up
            Constraint::Length(3),   // gap
            Constraint::Length(5),   // view mode
            Constraint::Length(3),   // gap
            Constraint::Min(10),     // location field
            Constraint::Length(3),   // gap
            Constraint::Length(3),   // search
            Constraint::Length(2),   // gap
            Constraint::Length(3),   // sort
            Constraint::Length(2),   // right pad
        ])
        .split(row);

    app.topbar_back_rect = chunks[1];
    app.topbar_fwd_rect = chunks[3];
    app.topbar_up_rect = chunks[5];
    app.topbar_view_rect = chunks[7];
    app.topbar_path_rect = chunks[9];
    app.topbar_search_rect = chunks[11];
    app.topbar_sort_rect = chunks[13];

    let menu_kind = app.menu.as_ref().map(|m| m.kind);
    let hover = app.mouse_pos.filter(|_| app.menu.is_none());
    let button = |f: &mut Frame, rect: Rect, label: &str, enabled: bool, active: bool| {
        let hovered = hover.is_some_and(|(c, r)| enabled && contains(rect, c, r));
        let mut style = Style::default().fg(if enabled { theme::text() } else { theme::border() });
        if active {
            style = style.bg(theme::selection()).fg(theme::text_strong());
        } else if hovered {
            style = style.bg(theme::hover());
        }
        f.render_widget(Paragraph::new(label.to_string()).style(style), rect);
    };

    button(f, chunks[1], " \u{f060} ", !app.history_back.is_empty(), false);
    button(f, chunks[3], " \u{f061} ", !app.history_forward.is_empty(), false);
    button(f, chunks[5], " \u{f062} ", app.cwd.parent().is_some(), false);
    let view = format!(" {} \u{f0d7} ", menu::view_mode_icon(app.view_mode));
    button(f, chunks[7], &view, true, menu_kind == Some(MenuKind::View));
    button(f, chunks[11], " \u{f002} ", true, app.search.is_some());
    button(f, chunks[13], " \u{f0dc} ", true, menu_kind == Some(MenuKind::Sort));

    render_location(f, app, chunks[9]);
    // NOTE: menus/completions rendered in ui/mod.rs AFTER the grid (z-order)
}

/// Recessed field: folder icon + the path, or the typed path.
fn render_location(f: &mut Frame, app: &mut App, field: Rect) {
    app.topbar_crumbs.clear();
    if field.width < 4 {
        return;
    }
    let body = Rect { x: field.x + 1, width: field.width - 2, ..field };
    // A rounded box a little taller than the text row (Kitty), else cell bg
    if !underlay::field(app, 0, field, theme::get().input) {
        f.render_widget(Block::default().style(Style::default().bg(path_bg())), field);
    }

    // The folder icon stays while editing, and the text starts where the
    // breadcrumbs do, so nothing shifts
    let icon_name = location_icon(app);
    let icon_rect = Rect { width: 2.min(body.width), ..body };
    crate::ui::sidebar::draw_icon(f, app, &icon_name, icon_rect, None);

    let crumbs_area = Rect { x: body.x + 3, width: body.width.saturating_sub(3), ..body };
    if app.path_bar_editing {
        let line = Line::from(crate::lineedit::spans(&app.path_bar_input, app.path_bar_cursor, Style::default().fg(theme::text_strong())));
        f.render_widget(Paragraph::new(line), crumbs_area);
        return;
    }
    let hover = app.mouse_pos.filter(|_| app.menu.is_none());
    let end = crumbs_area.x + crumbs_area.width;
    let mut x = crumbs_area.x;
    let segs = breadcrumbs(&app.cwd, crumbs_area.width as usize);
    for (label, target) in segs {
        let w = (label.chars().count() as u16).min(end.saturating_sub(x));
        if w == 0 { break; }
        let rect = Rect { x, width: w, ..crumbs_area };
        let hovered = target.is_some() && hover.is_some_and(|(c, r)| contains(rect, c, r));
        let style = if hovered {
            Style::default().fg(theme::text_strong()).add_modifier(Modifier::UNDERLINED)
        } else {
            Style::default().fg(theme::text_strong())
        };
        f.render_widget(Paragraph::new(Span::styled(label, style)), rect);
        if let Some(path) = target {
            app.topbar_crumbs.push((rect, path));
        }
        x += w;
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// Geometry of the path completion dropdown (shared with hit-testing/occlusion).
pub fn completions_rect(app: &App, term: Rect) -> Option<Rect> {
    if !app.path_bar_editing || app.path_completions.is_empty() {
        return None;
    }
    let anchor = app.topbar_path_rect;
    let max_show = app.path_completions.len().min(8) as u16;
    let height = max_show + 2;
    let width = anchor.width.max(20).min(term.width);
    // Drop below the full topbar (not just the content row)
    let y = TOPBAR_H.min(term.height.saturating_sub(height));
    let x = anchor.x.min(term.width.saturating_sub(width));
    Some(Rect { x, y, width, height })
}

/// Called from ui/mod.rs after the grid renders when path bar is active.
pub fn render_path_completions(f: &mut Frame, app: &mut App) {
    let Some(area) = completions_rect(app, f.area()) else {
        app.completions_rect = Rect::default();
        return;
    };
    app.completions_rect = area;

    let max_show = (area.height.saturating_sub(2)) as usize;
    let items: Vec<ListItem> = app.path_completions.iter()
        .take(max_show)
        .enumerate()
        .map(|(i, p)| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or(p.to_str().unwrap_or(""));
            let label = format!(" \u{f07b} {}", name);
            let style = if app.path_completion_sel == Some(i) {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            ListItem::new(label).style(style)
        })
        .collect();

    f.render_widget(Clear, area);
    let mut state = ListState::default().with_selected(app.path_completion_sel);
    f.render_stateful_widget(
        List::new(items)
            .style(Style::default().bg(path_bg()))
            .block(Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme::border()))
                .title(" Path "))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

/// The full path as plain text ("/home/user/Documents/"), split into
/// pieces: each folder name carries the path it opens, slashes carry none.
/// If it doesn't fit, leading folders collapse to "…".
fn breadcrumbs(path: &Path, width: usize) -> Vec<(String, Option<PathBuf>)> {
    let mut base = PathBuf::from("/");
    let mut segs: Vec<(String, PathBuf)> = Vec::new();
    for s in path.strip_prefix("/").unwrap_or(path).iter() {
        base = base.join(s);
        segs.push((s.to_string_lossy().into_owned(), base.clone()));
    }

    let len = |segs: &[(String, PathBuf)]| 1 + segs.iter().map(|s| s.0.chars().count() + 1).sum::<usize>();
    let mut skipped = false;
    while segs.len() > 1 && len(&segs) + 2 > width {
        segs.remove(0);
        skipped = true;
    }
    let mut out = vec![("/".to_string(), Some(PathBuf::from("/")))];
    if skipped {
        out.push(("\u{2026}/".to_string(), None));
    }
    for (label, p) in segs {
        out.push((label, Some(p)));
        out.push(("/".to_string(), None));
    }
    out
}


/// Theme icon for the current folder, shown at the start of the location bar.
pub fn location_icon(app: &App) -> String {
    crate::icons::resolver::folder_icon(&app.cwd)
        .or_else(|| App::xdg_folder_icon(&app.cwd))
        .unwrap_or_else(|| "folder".into())
}
