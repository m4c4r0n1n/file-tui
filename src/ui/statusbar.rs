use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::App;
use crate::ui::{details::format_size, theme};

pub fn render_statusbar(f: &mut Frame, app: &mut App, area: Rect) {
    app.cancel_rect = None;
    if let Some(p) = app.fileops.progress.clone()
        // Quick jobs finish before they'd flash a progress bar
        && p.started.elapsed() >= std::time::Duration::from_millis(250)
    {
        render_progress(f, app, area, &p);
        return;
    }

    let muted = Style::default().fg(theme::muted());
    let sep = || Span::styled("  ·  ", muted);
    let entries = &app.listing.entries;

    let mut left: Vec<Span> = vec![Span::raw(" ")];
    let selected: Vec<usize> = if !app.multi_selection.is_empty() {
        app.multi_selection.iter().copied().collect()
    } else {
        app.selection.into_iter().collect()
    };

    let searching = app.search.as_ref().filter(|s| s.has_query());

    match selected.as_slice() {
        [] if let Some(s) = searching => {
            let n = entries.len();
            left.push(Span::raw(if n == 1 { "1 result".to_string() } else { format!("{n} results") }));
            if s.running {
                left.push(Span::styled("  searching\u{2026}", muted));
            } else if s.truncated {
                left.push(Span::styled(format!("  (first {})", crate::search::MAX_RESULTS), muted));
            }
        }
        [] => {
            let dirs = entries.iter().filter(|e| e.is_dir).count();
            let files = entries.len() - dirs;
            left.push(Span::raw(plural(entries.len(), "item")));
            if !entries.is_empty() {
                left.push(Span::styled(
                    format!(" ({}, {})", plural(dirs, "folder"), plural(files, "file")),
                    muted,
                ));
            }
        }
        [i] => {
            if let Some(e) = entries.get(*i) {
                // In results, show where the file lives
                let label = if searching.is_some() { display_path(&e.path) } else { e.name.clone() };
                left.push(Span::raw(format!("\u{201c}{label}\u{201d}")));
                if !e.is_dir {
                    left.push(sep());
                    left.push(Span::raw(format_size(e.size, e.is_dir)));
                }
                if let Some(mime) = e.mime_type.as_deref() {
                    left.push(sep());
                    left.push(Span::styled(mime.to_string(), muted));
                }
            }
        }
        many => {
            let total: u64 = many.iter()
                .filter_map(|&i| entries.get(i))
                .filter_map(|e| e.size)
                .sum();
            left.push(Span::raw(format!("{} selected", many.len())));
            left.push(sep());
            left.push(Span::raw(format_size(Some(total), false)));
        }
    }

    if let Some((text, until)) = &app.notice
        && std::time::Instant::now() < *until
    {
        left = vec![Span::raw(" "), Span::styled(text.clone(), Style::default().fg(theme::rgb(theme::get().warning)))];
    }

    let mut right: Vec<Span> = Vec::new();
    if app.show_hidden {
        right.push(Span::styled("hidden files shown", muted));
        right.push(sep());
    }
    if let Some(free) = app.free_space {
        right.push(Span::raw(format!("{} free", format_size(Some(free), false))));
    }
    right.push(Span::raw(" "));

    f.render_widget(Paragraph::new(Line::from(left)), area);
    f.render_widget(Paragraph::new(Line::from(right)).alignment(Alignment::Right), area);
}

/// A running file operation: what and how far on the left, a bar with the
/// percentage and a cancel button on the right.
fn render_progress(f: &mut Frame, app: &mut App, area: Rect, p: &crate::fileops::Progress) {
    let muted = Style::default().fg(theme::muted());
    let sep = || Span::styled("  \u{b7}  ", muted);
    let mut left = vec![Span::raw(" "), Span::raw(p.verb().to_string())];
    if p.scanning {
        left.push(Span::styled("  counting files\u{2026}", muted));
    } else {
        if !p.current.is_empty() {
            left.push(Span::raw(format!(" \u{201c}{}\u{201d}", p.current)));
        }
        if p.total_items > 1 {
            left.push(sep());
            left.push(Span::raw(format!("{} of {}", p.done_items.min(p.total_items), p.total_items)));
        }
        if p.total_bytes > 0 {
            left.push(sep());
            left.push(Span::raw(format!(
                "{} of {}",
                format_size(Some(p.done_bytes), false),
                format_size(Some(p.total_bytes), false),
            )));
        }
        if let Some(rem) = p.remaining() {
            let s = rem.as_secs();
            left.push(sep());
            left.push(Span::styled(
                if s < 60 { format!("{s} s left") } else { format!("{} min left", s.div_ceil(60)) },
                muted,
            ));
        }
    }

    // Right: [bar] 45%  ✕
    let pct = format!(" {:>3}%  ", (p.fraction() * 100.0).round() as u32);
    let bar_w = 24u16.min(area.width / 4);
    let right_w = bar_w + pct.chars().count() as u16 + 2;
    let x = area.right().saturating_sub(right_w);
    let bar = Rect { x, y: area.y, width: bar_w, height: 1 };
    let done = ((bar_w as f64) * p.fraction()).round() as u16;
    let (cw, ch) = { let s = app.picker.font_size(); (s.width as u32, s.height as u32) };
    let h = (ch as f32 * 0.3).round().max(2.0) as u32;
    let y = bar.y as u32 * ch + (ch - h) / 2;
    let pill = |w: u16| (bar.x as u32 * cw, y, w as u32 * cw, h);
    let kitty = crate::ui::underlay::bar(app, 8, pill(bar_w), theme::get().border);
    if kitty {
        if done > 0 {
            crate::ui::underlay::bar(app, 9, pill(done), theme::get().highlight);
        }
    } else {
        let line = Line::from(vec![
            Span::styled("\u{2501}".repeat(done as usize), Style::default().fg(theme::rgb(theme::get().highlight))),
            Span::styled("\u{2501}".repeat((bar_w - done) as usize), Style::default().fg(theme::border())),
        ]);
        f.render_widget(Paragraph::new(line), bar);
    }
    let cancel = Rect { x: area.right().saturating_sub(2), y: area.y, width: 1, height: 1 };
    app.cancel_rect = Some(cancel);
    f.render_widget(Paragraph::new(Line::from(left)), Rect { width: x.saturating_sub(area.x + 1), ..area });
    f.render_widget(Paragraph::new(pct), Rect { x: bar.right(), width: right_w - bar_w, ..area });
    f.render_widget(Paragraph::new(Span::styled("\u{2715}", Style::default().fg(theme::text_strong()))), cancel);
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// Path with the home directory shown as ~.
pub fn display_path(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(|p| p.to_path_buf())) {
        Some(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

