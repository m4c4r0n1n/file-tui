pub mod compact;
pub mod details;
pub mod drag_ghost;
pub mod groups;
pub mod scrollbar;
pub mod grid;
pub mod help;
pub mod kitty;
pub mod menu;
pub mod properties;
pub mod searchbar;
pub mod sidebar;
pub mod tabbar;
pub mod statusbar;
pub mod theme;
pub mod topbar;
pub mod underlay;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use std::io::Write;

use crate::app::{App, TOPBAR_H, ViewMode};

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(TOPBAR_H),
            // Search field row plus room for its box to reach below it
            Constraint::Length(if app.search.is_some() { 2 } else { 0 }),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    let topbar_area = vertical[0];
    let search_area = vertical[1];
    let content_area = vertical[2];
    let status_area = vertical[3];

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(app.sidebar_w),
            Constraint::Min(0),
        ])
        .split(content_area);

    let sidebar_area = horizontal[0];
    let main_area = horizontal[1];

    app.main_area = main_area;

    // Render base layers
    topbar::render_topbar(f, app, topbar_area);
    sidebar::render_sidebar(f, app, sidebar_area);
    statusbar::render_statusbar(f, app, status_area);
    searchbar::render_searchbar(f, app, search_area);

    // Tabs over the views, when there's more than one
    let mut main_area = main_area;
    app.tab_rects.clear();
    if app.tabs.len() > 1 && main_area.height > 2 {
        tabbar::render_tabbar(f, app, Rect { height: 1, ..main_area });
        main_area.y += 1;
        main_area.height -= 1;
    }
    // Split: two panes with a line between, the one in use drawn last so
    // its geometry is what the handlers see
    let panes: Vec<Rect> = if app.split() {
        let left_w = main_area.width / 2;
        let line = Rect { x: main_area.x + left_w, width: 1, ..main_area };
        f.render_widget(
            Paragraph::new(vec![Line::raw("\u{2502}"); line.height as usize]).style(Style::default().fg(theme::border())),
            line,
        );
        vec![
            Rect { width: left_w, ..main_area },
            Rect { x: line.x + 1, width: main_area.width.saturating_sub(left_w + 1), ..main_area },
        ]
    } else {
        vec![main_area]
    };
    app.pane_rects = panes.clone();
    let split = panes.len() > 1;
    let active = app.tabs[app.active_tab].active.min(panes.len() - 1);
    if split {
        let other = panes[1 - active];
        app.with_other_pane(|a| render_view(f, a, other, true, false));
    }
    render_view(f, app, panes[active], split, true);
    render_band(f, app);

    // Overlays rendered AFTER the grid so they appear on top of everything
    if let Some(mut m) = app.menu.take() {
        let panel = false;
        let text = menu::render_menu(f, &mut m, panel);
        app.post_draw.extend(text);
        let slots = menu::icon_slots(&m);
        let (rect, sub) = (m.rect, m.submenu_rect);
        let lit = menu::highlight_rects(&m);
        app.menu = Some(m);
        if panel {
            let (fill, line) = menu::panel();
            underlay::panel(app, 2, rect, fill, line);
            if sub.width > 0 {
                underlay::panel(app, 3, sub, fill, line);
            }
            // Highlights sit the same distance from the panel edge on all sides
            let inset = underlay::highlight_inset(app);
            for (slot, r) in [(4, lit.0), (5, lit.1)] {
                if let Some(r) = r {
                    underlay::highlight(app, slot, r, inset, inset, menu::highlight());
                }
            }
        }
        for (icon, rect, hot) in slots {
            let bg = match (panel, hot) {
                (true, _) => None,
                (false, true) => Some(menu::icon_bg().1),
                (false, false) => Some(menu::icon_bg().0),
            };
            sidebar::draw_icon(f, app, &icon, rect, bg);
        }
    }
    drag_ghost::render_drag_ghost(f, app);
    if app.path_bar_editing {
        topbar::render_path_completions(f, app);
    } else {
        app.completions_rect = Rect::default();
    }
    if let Some(ref props) = app.properties {
        properties::render_properties(f, props);
    }
    if app.show_help {
        help::render_help(f, app);
    }
    app.prompt_buttons = None;
    if app.name_prompt.is_some() {
        render_name_prompt(f, app);
    }
    app.conflict_buttons.clear();
    if app.conflict_prompt.is_some() {
        render_conflict_prompt(f, app);
    }
    if app.confirm_delete.is_some() {
        render_confirm_delete(f, app);
    }

    // Image uploads lifted out of placeholder cells go out before this
    // frame's cell updates, so the placeholders always have an image.
    underlay::finish_frame(app);

    if !app.kitty_transmits.is_empty() {
        let mut out = std::io::stdout().lock();
        for t in app.kitty_transmits.drain(..) {
            let _ = out.write_all(t.as_bytes());
        }
        let _ = out.flush();
    }
}

/// The rubber-band rectangle: a translucent box on Kitty, an outline elsewhere.
fn render_band(f: &mut Frame, app: &mut App) {
    let r = app.band.as_ref()
        .map(|&(a, b, _)| band_rect(a, b).intersection(app.grid_area))
        .filter(|r| r.width > 0 && r.height > 0);
    if !underlay::selection_band(app, r, theme::get().highlight) && let Some(r) = r {
        f.render_widget(
            Block::default().borders(ratatui::widgets::Borders::ALL).border_style(Style::default().fg(theme::rgb(theme::get().highlight))),
            r,
        );
    }
}

/// A folder view in `area`, under its own header when split.
fn render_view(f: &mut Frame, app: &mut App, mut area: Rect, header: bool, active: bool) {
    if header && area.height > 2 {
        tabbar::render_pane_header(f, Rect { height: 1, ..area }, &app.cwd, active);
        area.y += 1;
        area.height -= 1;
    }
    app.main_area = area;
    app.item_rects.clear();
    match app.view_mode {
        ViewMode::Icons   => grid::render_grid(f, app, area),
        ViewMode::Compact => compact::render_compact(f, app, area),
        ViewMode::Details => details::render_details(f, app, area),
    }
    scrollbar::render(f, app, area);
}

pub fn band_rect((ax, ay): (u16, u16), (bx, by): (u16, u16)) -> Rect {
    Rect { x: ax.min(bx), y: ay.min(by), width: ax.abs_diff(bx) + 1, height: ay.abs_diff(by) + 1 }
}

fn confirm_delete_rect(term: Rect) -> Rect {
    let w = 44u16.min(term.width);
    let h = 7u16.min(term.height);
    Rect {
        x: term.width.saturating_sub(w) / 2,
        y: term.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

/// Dolphin's Create Folder / Create File / Paste Clipboard Contents
/// dialogs: a name field with Create and Cancel buttons.
fn render_name_prompt(f: &mut Frame, app: &mut App) {
    use crate::events::PromptKind;
    let Some(ref p) = app.name_prompt else { return; };
    let (title, label, verb) = match p.kind {
        PromptKind::NewFolder => (" New Folder ", "Name for the new folder:", "Create"),
        PromptKind::NewFile => (" New File ", "Name for the new file:", "Create"),
        PromptKind::Paste(_) => (" Paste Clipboard Contents ", "Filename for clipboard content:", "Save"),
    };
    let term = f.area();
    let w = 52u16.min(term.width);
    let h = 7u16.min(term.height);
    let area = Rect { x: term.width.saturating_sub(w) / 2, y: term.height.saturating_sub(h) / 2, width: w, height: h };
    let bg = Style::default().bg(theme::panel()).fg(theme::text());

    f.render_widget(Clear, area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::border()))
            .title(title)
            .style(bg),
        area,
    );

    // Input field scrolled so the cursor stays visible
    let field_w = w.saturating_sub(6) as usize;
    let mut shown = p.input.clone();
    shown.insert(p.cursor_pos.min(shown.len()), crate::lineedit::bar_cursor());
    let chars: Vec<char> = shown.chars().collect();
    let cur = shown[..p.cursor_pos.min(p.input.len())].chars().count();
    let start = (cur + 1).saturating_sub(field_w);
    let mut visible: String = chars.iter().skip(start).take(field_w).collect();
    let field_bg = if p.invalid { theme::get().error } else { theme::get().input };
    let mut field_fg = theme::text_strong();
    if p.input.is_empty() {
        // Greyed suggestion, used if nothing is typed
        visible = format!("{}{}", crate::lineedit::bar_cursor(), p.placeholder).chars().take(field_w).collect();
        field_fg = theme::muted();
    }

    let lines = vec![
        Line::raw(format!("  {label}")),
        Line::raw(""),
        Line::from(vec![
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled(format!("{visible:<field_w$}"), Style::default().bg(theme::rgb(field_bg)).fg(field_fg)),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).style(bg),
        Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) },
    );

    // Buttons, right-aligned like a dialog's: the default one highlighted
    let y = area.bottom().saturating_sub(2);
    let ok_label = format!(" {verb} ");
    let cancel_label = " Cancel ";
    let cancel = Rect { x: area.right().saturating_sub(3 + cancel_label.len() as u16), y, width: cancel_label.len() as u16, height: 1 };
    let ok = Rect { x: cancel.x.saturating_sub(2 + ok_label.len() as u16), y, width: ok_label.len() as u16, height: 1 };
    f.render_widget(Paragraph::new(ok_label).style(Style::default().bg(theme::selection()).fg(theme::text_strong()).add_modifier(Modifier::BOLD)), ok);
    f.render_widget(Paragraph::new(cancel_label).style(Style::default().bg(theme::input()).fg(theme::text())), cancel);
    app.prompt_buttons = Some((ok, cancel));
}

/// Dolphin's "File Already Exists": what clashes with what, a field for a
/// new name, Apply to All, and the choices (their Alt+letter underlined).
fn render_conflict_prompt(f: &mut Frame, app: &mut App) {
    use crate::events::ConflictButton as B;
    use ratatui::text::Span;
    let Some(ref p) = app.conflict_prompt else { return; };
    let c = &p.conflict;
    let name = |path: &std::path::Path| path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    let dir = c.dst.parent().map(name).unwrap_or_default();
    let title = if c.both_dirs { " Folder Already Exists " } else { " File Already Exists " };
    let what = if c.dst.is_dir() { "A folder" } else { "A file" };

    let term = f.area();
    let w = 70u16.min(term.width);
    let h = 11u16.min(term.height);
    let area = Rect { x: term.width.saturating_sub(w) / 2, y: term.height.saturating_sub(h) / 2, width: w, height: h };
    let bg = Style::default().bg(theme::panel()).fg(theme::text());
    f.render_widget(Clear, area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::border()))
            .title(title)
            .style(bg),
        area,
    );

    let inner_w = w.saturating_sub(6) as usize;
    let fit = |s: String| -> String {
        if s.chars().count() <= inner_w { s } else { format!("{}\u{2026}", s.chars().take(inner_w.saturating_sub(1)).collect::<String>()) }
    };
    let field_w = inner_w;
    let mut shown = p.input.clone();
    shown.insert(p.cursor_pos.min(shown.len()), crate::lineedit::bar_cursor());
    let chars: Vec<char> = shown.chars().collect();
    let cur = shown[..p.cursor_pos.min(p.input.len())].chars().count();
    let start = (cur + 1).saturating_sub(field_w);
    let visible: String = chars.iter().skip(start).take(field_w).collect();
    let field_bg = if p.invalid { theme::get().error } else { theme::get().input };
    let muted = Style::default().fg(theme::muted());
    let verb = if c.op == crate::fileops::Op::Move { "Moving" } else { "Copying" };
    let lines = vec![
        Line::raw(fit(format!("  {what} named \"{}\" already exists in \"{dir}\".", name(&c.dst)))),
        Line::raw(""),
        Line::from(vec![Span::styled(format!("  {:<10}", verb), muted), Span::raw(fit(p.src_info.clone()))]),
        Line::from(vec![Span::styled(format!("  {:<10}", "Existing"), muted), Span::raw(fit(p.dst_info.clone()))]),
        Line::raw(""),
        Line::raw("  New name:"),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(format!("{visible:<field_w$}"), Style::default().bg(theme::rgb(field_bg)).fg(theme::text_strong())),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines).style(bg),
        Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: area.height.saturating_sub(2) },
    );

    // Buttons right-aligned, Rename (Enter) the default; Apply to All on the left
    let y = area.bottom().saturating_sub(2);
    let mut buttons: Vec<(&str, usize, B)> = Vec::new();
    if c.both_dirs {
        buttons.push(("Write Into", 0, B::Replace));
    } else if c.can_replace {
        buttons.push(("Overwrite", 0, B::Replace));
    }
    buttons.extend([("Skip", 0, B::Skip), ("Rename", 0, B::Rename), ("Cancel", usize::MAX, B::Cancel)]);
    let mut x = area.right().saturating_sub(2);
    let mut placed = Vec::new();
    for &(label, key, b) in buttons.iter().rev() {
        let bw = label.chars().count() as u16 + 2;
        x = x.saturating_sub(bw + 1);
        placed.push((Rect { x, y, width: bw, height: 1 }, label, key, b));
    }
    for (r, label, key, b) in placed {
        let style = if b == B::Rename {
            Style::default().bg(theme::selection()).fg(theme::text_strong()).add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(theme::input()).fg(theme::text())
        };
        let mut spans = vec![Span::styled(" ", style)];
        for (i, ch) in label.chars().enumerate() {
            let st = if i == key { style.add_modifier(Modifier::UNDERLINED) } else { style };
            spans.push(Span::styled(ch.to_string(), st));
        }
        spans.push(Span::styled(" ", style));
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        app.conflict_buttons.push((r, b));
    }
    if c.more {
        let label = format!("[{}] Apply to all", if p.all { "x" } else { " " });
        let r = Rect { x: area.x + 3, y, width: label.chars().count() as u16, height: 1 };
        let spans: Vec<Span> = label.chars().enumerate()
            .map(|(i, ch)| Span::styled(ch.to_string(), if i == 4 { Style::default().add_modifier(Modifier::UNDERLINED) } else { Style::default() }))
            .collect();
        f.render_widget(Paragraph::new(Line::from(spans)).style(bg), r);
        app.conflict_buttons.push((r, B::AllToggle));
    }
}

fn render_confirm_delete(f: &mut Frame, app: &App) {
    let Some(ref cd) = app.confirm_delete else { return; };
    let area = confirm_delete_rect(f.area());

    let name = if cd.display_name.chars().count() > 28 {
        format!("{}…", cd.display_name.chars().take(27).collect::<String>())
    } else {
        cd.display_name.clone()
    };

    f.render_widget(Clear, area);
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::rgb(theme::get().error)))
            .title(" Permanently Delete ")
            .style(Style::default().bg(theme::danger_bg()).fg(theme::text_strong())),
        area,
    );

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let lines = vec![
        Line::raw(format!("  Delete \"{}\"?", name)),
        Line::raw(""),
        Line::raw("  This cannot be undone."),
        Line::raw(""),
        Line::styled(
            "  [Enter / Y]  Yes, delete     [Esc / N]  Cancel",
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];

    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme::danger_bg()).fg(theme::text_strong())),
        inner,
    );
}
