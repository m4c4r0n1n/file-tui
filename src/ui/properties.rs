use std::{
    os::unix::fs::MetadataExt,
    path::Path,
    sync::mpsc,
    time::SystemTime,
};

use chrono::{DateTime, Local};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::ui::theme;

const PROPS_W: u16 = 58;
fn props_bg() -> Color { theme::panel() }

pub struct PropertiesState {
    pub rows: Vec<(String, String)>,
    /// Background dir-size walk results: (items, bytes), final value wins.
    size_rx: Option<mpsc::Receiver<(u64, u64)>>,
    /// The permission checkboxes, when the item is ours to change.
    pub perms: Option<PermEdit>,
    /// Where each checkbox was drawn (index as in `PermEdit::cursor`).
    pub hits: std::cell::RefCell<Vec<(Rect, usize)>>,
}

/// Editing an item's mode: read/write/execute for owner, group and
/// others (cursor 0..9, row by row), then "apply to contents" (9) for
/// folders.
pub struct PermEdit {
    pub path: std::path::PathBuf,
    pub original: u32,
    pub mode: u32,
    pub is_dir: bool,
    pub recursive: bool,
    pub cursor: usize,
    pub error: Option<String>,
}

impl PermEdit {
    fn bit(i: usize) -> u32 {
        // Owner r,w,x = 0o400, 0o200, 0o100, then group, then others
        0o400 >> i
    }

    pub fn slots(&self) -> usize {
        if self.is_dir { 10 } else { 9 }
    }

    pub fn toggle(&mut self, i: usize) {
        match i {
            0..9 => self.mode ^= Self::bit(i),
            9 if self.is_dir => self.recursive = !self.recursive,
            _ => {}
        }
        self.error = None;
    }

    pub fn changed(&self) -> bool {
        self.mode != self.original
    }

    /// Set the new mode; with `recursive`, change the same bits (only those
    /// that changed) on everything inside too.
    pub fn apply(&self) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        let set = |p: &Path, mode: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("{}: {e}", p.display()));
        set(&self.path, self.mode)?;
        if !(self.is_dir && self.recursive) {
            return Ok(());
        }
        let (on, off) = (self.mode & !self.original & 0o777, self.original & !self.mode & 0o777);
        let mut failed = Vec::new();
        let mut stack = vec![self.path.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let Ok(meta) = entry.path().symlink_metadata() else { continue; };
                if meta.file_type().is_symlink() {
                    continue;
                }
                if let Err(e) = set(&entry.path(), (meta.mode() & 0o7777 | on) & !off) {
                    failed.push(e);
                }
                if meta.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        match failed.as_slice() {
            [] => Ok(()),
            [one] => Err(one.clone()),
            [first, rest @ ..] => Err(format!("{first} (+{} more)", rest.len())),
        }
    }
}

impl PropertiesState {
    pub fn new(path: &Path) -> Self {
        let meta = std::fs::symlink_metadata(path).ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);

        let name = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let location = path.parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let type_str = if is_dir {
            "Folder (inode/directory)".to_string()
        } else if meta.as_ref().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            "Symbolic link".to_string()
        } else {
            crate::mime::detect(path).unwrap_or_else(|| "application/octet-stream".into())
        };

        let size_str = if is_dir {
            "Calculating\u{2026}".to_string()
        } else {
            meta.as_ref()
                .map(|m| format!("{} ({} bytes)", human_size(m.len()), m.len()))
                .unwrap_or_else(|| "\u{2014}".into())
        };

        let fmt_time = |t: Option<SystemTime>| t
            .map(|t| {
                let dt: DateTime<Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_else(|| "\u{2014}".into());

        let (perms, owner) = meta.as_ref().map(|m| {
            let mode = m.mode();
            let perms = format!("{} ({:03o})", mode_string(mode), mode & 0o7777);
            let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(m.uid()))
                .ok().flatten().map(|u| u.name)
                .unwrap_or_else(|| m.uid().to_string());
            let group = nix::unistd::Group::from_gid(nix::unistd::Gid::from_raw(m.gid()))
                .ok().flatten().map(|g| g.name)
                .unwrap_or_else(|| m.gid().to_string());
            (perms, format!("{user}:{group}"))
        }).unwrap_or_else(|| ("\u{2014}".into(), "\u{2014}".into()));

        let free = nix::sys::statvfs::statvfs(path)
            .map(|vfs| {
                let free = vfs.blocks_available() * vfs.fragment_size();
                let total = vfs.blocks() * vfs.fragment_size();
                format!("{} free of {}", human_size(free), human_size(total))
            })
            .unwrap_or_else(|_| "\u{2014}".into());

        let rows = vec![
            ("Name".into(), name),
            ("Type".into(), type_str),
            ("Location".into(), location),
            ("Size".into(), size_str),
            ("Modified".into(), fmt_time(meta.as_ref().and_then(|m| m.modified().ok()))),
            ("Created".into(), fmt_time(meta.as_ref().and_then(|m| m.created().ok()))),
            ("Accessed".into(), fmt_time(meta.as_ref().and_then(|m| m.accessed().ok()))),
            ("Permissions".into(), perms),
            ("Owner".into(), owner),
            ("Device".into(), free),
        ];

        let size_rx = if is_dir {
            let (tx, rx) = mpsc::channel();
            let path = path.to_path_buf();
            std::thread::spawn(move || {
                let mut items: u64 = 0;
                let mut bytes: u64 = 0;
                walk(&path, &mut items, &mut bytes, &tx);
                let _ = tx.send((items, bytes));
            });
            Some(rx)
        } else {
            None
        };

        let ours = meta.as_ref().is_some_and(|m| m.uid() == nix::unistd::getuid().as_raw() && !m.file_type().is_symlink());
        let perms = ours.then(|| {
            let mode = meta.as_ref().map(|m| m.mode() & 0o7777).unwrap_or(0);
            PermEdit { path: path.to_path_buf(), original: mode, mode, is_dir, recursive: false, cursor: 0, error: None }
        });
        Self { rows, size_rx, perms, hits: Default::default() }
    }

    /// Drain async dir-size updates (called from App::drain_channels).
    pub fn poll(&mut self) {
        let Some(rx) = &self.size_rx else { return; };
        let mut latest = None;
        while let Ok(v) = rx.try_recv() {
            latest = Some(v);
        }
        if let Some((items, bytes)) = latest
            && let Some(row) = self.rows.iter_mut().find(|(k, _)| k == "Size") {
                row.1 = format!("{} ({} items)", human_size(bytes), items);
            }
    }
}

fn walk(dir: &Path, items: &mut u64, bytes: &mut u64, tx: &mpsc::Sender<(u64, u64)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return; };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else { continue; };
        *items += 1;
        if (*items).is_multiple_of(2048) {
            let _ = tx.send((*items, *bytes));
        }
        if meta.is_dir() {
            walk(&entry.path(), items, bytes, tx);
        } else {
            *bytes += meta.len();
        }
    }
}

fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 0o7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

pub fn human_size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b => format!("{:.1} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

/// Lines the permission editor adds: a blank, the header, three rows, the
/// folder checkbox, and the status line.
fn perm_lines(state: &PropertiesState) -> u16 {
    state.perms.as_ref().map_or(0, |p| 6 + u16::from(p.is_dir))
}

pub fn properties_rect(state: &PropertiesState, term: Rect) -> Rect {
    let width = PROPS_W.min(term.width);
    let height = (state.rows.len() as u16 + perm_lines(state) + 2).min(term.height);
    let x = term.width.saturating_sub(width) / 2;
    let y = term.height.saturating_sub(height) / 2;
    Rect { x, y, width, height }
}

pub fn render_properties(f: &mut Frame, state: &PropertiesState) {
    let area = properties_rect(state, f.area());
    f.render_widget(Clear, area);

    let label_w = 12;
    let val_w = area.width.saturating_sub(label_w as u16 + 4) as usize;
    let mut lines: Vec<Line> = state.rows.iter().map(|(k, v)| {
        let mut val = v.clone();
        if val.chars().count() > val_w && val_w > 1 {
            val = val.chars().take(val_w - 1).collect::<String>() + "\u{2026}";
        }
        Line::from(vec![
            Span::styled(
                format!(" {k:<label_w$}"),
                Style::default().bg(props_bg()).add_modifier(Modifier::BOLD | Modifier::DIM),
            ),
            Span::styled(val, Style::default().bg(props_bg())),
        ])
    }).collect();

    let mut hits = Vec::new();
    if let Some(p) = &state.perms {
        let bg = Style::default().bg(props_bg());
        let dim = bg.add_modifier(Modifier::BOLD | Modifier::DIM);
        let box_at = |i: usize, on: bool| {
            let mut st = bg;
            if p.cursor == i {
                st = st.bg(theme::rgb(theme::get().selection)).add_modifier(Modifier::BOLD);
            }
            Span::styled(if on { "[x]" } else { "[ ]" }, st)
        };
        let col_x = |c: usize| area.x + 1 + 1 + label_w as u16 + c as u16 * 8;
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled(format!(" {:<label_w$}", "Access"), dim),
            Span::styled("Read    Write   Execute", dim),
        ]));
        for (r, who) in ["Owner", "Group", "Others"].iter().enumerate() {
            let mut spans = vec![Span::styled(format!(" {who:<label_w$}"), bg)];
            for c in 0..3 {
                let i = r * 3 + c;
                spans.push(box_at(i, p.mode & PermEdit::bit(i) != 0));
                spans.push(Span::styled("     ", bg));
                let y = area.y + 1 + lines.len() as u16;
                hits.push((Rect { x: col_x(c), y, width: 3, height: 1 }, i));
            }
            lines.push(Line::from(spans));
        }
        if p.is_dir {
            let y = area.y + 1 + lines.len() as u16;
            hits.push((Rect { x: col_x(0), y, width: 24, height: 1 }, 9));
            lines.push(Line::from(vec![
                Span::styled(format!(" {:<label_w$}", ""), bg),
                box_at(9, p.recursive),
                Span::styled(" Apply to contents", bg),
            ]));
        }
        let status = match (&p.error, p.changed()) {
            (Some(e), _) => Span::styled(format!(" {e}"), bg.fg(theme::rgb(theme::get().error))),
            (None, true) => Span::styled(format!(" {} ({:03o})  Enter applies, Esc cancels", mode_string(p.mode), p.mode & 0o7777), bg.add_modifier(Modifier::BOLD)),
            (None, false) => Span::styled(" Arrows and Space or click to change", dim),
        };
        lines.push(Line::from(status));
    }
    *state.hits.borrow_mut() = hits;

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Properties (Esc to close) ")
                .border_style(Style::default().fg(theme::border()).bg(props_bg()))
                .style(Style::default().bg(props_bg())),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn recursive_apply_changes_only_the_toggled_bits() {
        let root = std::env::temp_dir().join(format!("fbt-perms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let set = |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o777;
        std::fs::write(root.join("script.sh"), "").unwrap();
        std::fs::write(root.join("sub/notes.txt"), "").unwrap();
        set(&root.join("script.sh"), 0o755);
        set(&root.join("sub/notes.txt"), 0o644);
        set(&root, 0o755);

        let mut st = PropertiesState::new(&root);
        let p = st.perms.as_mut().expect("ours to edit");
        assert_eq!(p.slots(), 10);
        p.toggle(5); // group execute off
        p.toggle(7); // others write on
        p.toggle(9); // apply to contents
        assert!(p.changed());
        p.apply().unwrap();
        assert_eq!(mode(&root), 0o747);
        assert_eq!(mode(&root.join("script.sh")), 0o747);
        assert_eq!(mode(&root.join("sub")), 0o747);
        // Plain files keep their own (lack of) execute bits
        assert_eq!(mode(&root.join("sub/notes.txt")), 0o646);
        let _ = std::fs::remove_dir_all(&root);
    }
}
