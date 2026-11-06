use chrono::{DateTime, Local};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Row, Table},
};

use crate::app::{App, SortField};
use crate::ui::{groups::{self, DRow}, theme};

pub fn render_details(f: &mut Frame, app: &mut App, area: Rect) {
    f.render_widget(Block::default(), area);

    // One padding column on each side, matching the grid views
    let area = Rect {
        x: area.x + 1,
        y: area.y,
        width: area.width.saturating_sub(2),
        height: area.height,
    };
    app.grid_cols = 1;
    app.grid_area = area;

    let searching = app.search.as_ref().is_some_and(|s| s.has_query());
    app.details_rows = if app.grouped() {
        groups::rows(&app.listing.entries, app.sort.field)
    } else {
        (0..app.listing.entries.len()).map(DRow::Entry).collect()
    };
    let hovered = if app.menu.is_none() {
        app.mouse_pos.and_then(|(_, r)| crate::ui::grid::details_row_at_pos(r, area, app.scroll_offset, &app.details_rows))
            .filter(|_| app.mouse_pos.is_some_and(|(c, _)| c >= area.x && c < area.right()))
    } else {
        None
    };
    let header_style = Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD);
    // Clickable headers; the active sort column shows its direction
    let arrow = if app.sort.ascending { " \u{25b4}" } else { " \u{25be}" };
    let title = |label: &str, field: SortField| {
        if app.sort.field == field { format!("{label}{arrow}") } else { label.to_string() }
    };
    let header = Row::new(vec![
        Cell::from(format!(" {}", title("Name", SortField::Name))),
        Cell::from(Line::from(title("Size", SortField::Size)).alignment(Alignment::Right)),
        Cell::from(title("Modified", SortField::Modified)),
        Cell::from(if searching { "Location".to_string() } else { title("Type", SortField::Type) }),
    ]).style(header_style).height(1);

    // Rows start at scroll_offset so the table, scrolling and click mapping
    // (details_row_at_pos) all agree on which entry is on which line.
    let visible = area.height.saturating_sub(1) as usize;
    let lines: Vec<DRow> = app.details_rows.iter().skip(app.scroll_offset).take(visible).cloned().collect();
    let shown: Vec<usize> = lines.iter().filter_map(|l| if let DRow::Entry(i) = l { Some(*i) } else { None }).collect();

    // Folder "sizes" are item counts, read for the visible rows only
    let show_hidden = app.show_hidden;
    for e in shown.iter().map(|&i| &app.listing.entries[i]).filter(|e| e.is_dir) {
        app.dir_counts.entry(e.path.clone()).or_insert_with(|| {
            std::fs::read_dir(&e.path).ok().map(|rd| rd.filter_map(|d| d.ok())
                .filter(|d| show_hidden || !d.file_name().to_string_lossy().starts_with('.'))
                .count())
        });
    }

    let rows: Vec<Row> = lines.iter()
        .map(|line| {
            let DRow::Entry(i) = *line else { return Row::new(Vec::<Cell>::new()).height(1); };
            let e = &app.listing.entries[i];
            // indent, expander, icon (drawn as an image below), name
            let expander = match (e.is_dir, app.expanded.contains(&e.path)) {
                (true, true) => "\u{f107} ",
                (true, false) => "\u{f105} ",
                _ => "  ",
            };
            let name = Line::from(vec![
                Span::raw(format!(" {}", "  ".repeat(e.depth as usize))),
                Span::styled(expander, Style::default().fg(theme::muted())),
                Span::raw(format!("   {}", e.name)),
            ]);
            let size = if e.is_dir {
                match app.dir_counts.get(&e.path).copied().flatten() {
                    Some(1) => "1 item".to_string(),
                    Some(n) => format!("{n} items"),
                    None => "\u{2014}".to_string(),
                }
            } else {
                format_size(e.size, false)
            };
            let modified = e.modified
                .map(|t| { let dt: DateTime<Local> = t.into(); dt.format("%Y-%m-%d %H:%M").to_string() })
                .unwrap_or_default();
            let type_str = if searching {
                e.path.parent().map(crate::ui::statusbar::display_path).unwrap_or_default()
            } else if e.is_dir {
                "folder".to_string()
            } else {
                e.mime_type.clone()
                    .or_else(|| crate::mime::guess_by_name(&e.path))
                    .unwrap_or_else(|| "file".to_string())
            };
            let type_str = if e.is_symlink && !searching { format!("{type_str} (link)") } else { type_str };

            let is_selected = app.selection == Some(i) || app.multi_selection.contains(&i);
            let row_style = if is_selected {
                Style::default().bg(theme::selection()).fg(theme::text_strong())
            } else if hovered == Some(i) {
                Style::default().bg(theme::hover())
            } else {
                Style::default()
            };
            let row_style = if app.is_cut(&e.path) { row_style.add_modifier(Modifier::DIM) } else { row_style };
            let muted = if is_selected { Style::default() } else { Style::default().fg(theme::muted()) };

            Row::new(vec![
                Cell::from(name),
                Cell::from(Line::from(size).alignment(Alignment::Right)),
                Cell::from(modified).style(muted),
                Cell::from(type_str).style(muted),
            ])
            .style(row_style)
            .height(1)
        }).collect();

    let widths = [
        Constraint::Fill(3),
        Constraint::Length(11),
        Constraint::Length(16),
        Constraint::Fill(1),
    ];
    // Same split the table makes, for header hit-testing
    let cols = Layout::horizontal(widths).spacing(2).split(Rect { height: 1, ..area });
    let fields = [SortField::Name, SortField::Size, SortField::Modified, SortField::Type];
    app.details_headers = cols.iter().zip(fields).take(if searching { 3 } else { 4 }).map(|(r, f)| (*r, f)).collect();

    let table = Table::new(rows, widths)
        .header(header)
        .column_spacing(2);

    f.render_widget(table, area);

    // Group headers: the label, then a rule across the row
    for (row, line) in lines.iter().enumerate() {
        let DRow::Group(label) = line else { continue; };
        let y = area.y + 1 + row as u16;
        let rule = (area.width as usize).saturating_sub(label.chars().count() + 3);
        f.render_widget(
            ratatui::widgets::Paragraph::new(Line::from(vec![
                Span::styled(format!(" {label} "), Style::default().fg(theme::accent()).add_modifier(Modifier::BOLD)),
                Span::styled("\u{2500}".repeat(rule), Style::default().fg(theme::border())),
            ])),
            Rect { x: area.x, y, width: area.width, height: 1 },
        );
    }

    // Real icons/thumbnails, one row tall, after each row's expander
    for (row, line) in lines.iter().enumerate() {
        let DRow::Entry(i) = *line else { continue; };
        app.item_rects.push((i, Rect { x: area.x, y: area.y + 1 + row as u16, width: area.width, height: 1 }));
        let depth = app.listing.entries[i].depth;
        let is_selected = app.selection == Some(i) || app.multi_selection.contains(&i);
        let bg = if is_selected {
            Some(theme::get().selection)
        } else if hovered == Some(i) {
            Some(theme::get().hover)
        } else {
            None
        };
        let x = expander_x(area, depth) + 2;
        if x + 2 > area.right() { continue; }
        let rect = Rect { x, y: area.y + 1 + row as u16, width: 2, height: 1 };
        crate::ui::grid::render_cell_image(f.buffer_mut(), app, i, rect, bg);
    }
}

/// Column of a row's expander arrow (the icon follows 2 columns later).
pub fn expander_x(area: Rect, depth: u16) -> u16 {
    area.x + 1 + depth * 2
}

pub fn format_size(size: Option<u64>, is_dir: bool) -> String {
    if is_dir { return "\u{2014}".to_string(); }
    match size {
        None => "\u{2014}".to_string(),
        Some(b) if b < 1024 => format!("{b} B"),
        Some(b) if b < 1024 * 1024 => format!("{:.1} KiB", b as f64 / 1024.0),
        Some(b) if b < 1024 * 1024 * 1024 => format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0)),
        Some(b) => format!("{:.1} GiB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

