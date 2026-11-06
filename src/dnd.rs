//! Kitty's drag and drop protocol (OSC 72, kitty 0.47+): files dropped onto
//! the window from other programs, and the selection dragged out to them.
//!
//! crossterm has no OSC input parser, so each message from the terminal
//! reaches us as a burst of key events (Alt+']', '7', '2', ';', ..., Alt+'\'):
//! `Dnd::swallow` reassembles those before they can act as shortcuts.

use std::{io::Write, path::PathBuf};

use base64::Engine;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::ui::menu::{MenuAction, MenuItem, MenuKind, MenuState, MenuTarget};

const URI_LIST: &str = "text/uri-list";
const CHUNK: usize = 4096;

#[derive(Default)]
pub struct Dnd {
    /// An OSC being reassembled from key events.
    collecting: Option<String>,
    /// Types offered by the drag currently over the window.
    offered: Vec<String>,
    /// Last answer to a move event: (operation, target), resent on change.
    replied: Option<(u8, Option<PathBuf>)>,
    /// Grid entry highlighted as the drop target.
    pub hover_entry: Option<usize>,
    /// A drop whose uri-list is being received: (index, cell, data so far).
    reading: Option<(usize, (u16, u16), String)>,
    /// Paths of a drag we started (dropping them back on us skips the data).
    outgoing: Option<Vec<PathBuf>>,
    /// Dropped paths and destination, waiting for the Move/Copy/Link menu.
    job: Option<(Vec<PathBuf>, PathBuf)>,
}

fn send(msg: &str) {
    let mut out = std::io::stdout().lock();
    let _ = write!(out, "\x1b]72;{msg}\x1b\\");
    let _ = out.flush();
}

/// Send data under `meta` the way kitty's own kitten does: unpadded base64
/// in chunks (the last with m=0), then an empty message marking the end.
/// Kitty's stream decoder rejects '=' padding.
fn send_data(meta: &str, data: &[u8]) {
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(data);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        send(&format!("{meta}:m={more};{}", std::str::from_utf8(chunk).unwrap_or_default()));
    }
    send(meta);
}

/// Start accepting drops and offering drags.
pub fn enable() {
    send("t=a");
    send("t=o:x=1");
}

pub fn disable() {
    send("t=A");
    send("t=o:x=2");
}

impl Dnd {
    pub fn collecting(&self) -> bool {
        self.collecting.is_some()
    }
}

/// Feed a key event. Returns true when it was part of a DnD message (and
/// must not be handled as a key).
pub fn swallow(app: &mut App, key: &KeyEvent) -> bool {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let Some(buf) = app.dnd.collecting.as_mut() else {
        if key.code == KeyCode::Char(']') && alt {
            app.dnd.collecting = Some(String::new());
            return true;
        }
        return false;
    };
    match key.code {
        // ST (ESC \) or BEL ends the message
        KeyCode::Char('\\') if alt => {}
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {}
        KeyCode::Char(c) if !alt => {
            buf.push(c);
            // Not ours after all (a real Alt+]): stop and let this key through
            if buf.len() <= 3 && !"72;".starts_with(buf.as_str()) {
                app.dnd.collecting = None;
                return false;
            }
            return true;
        }
        _ => {
            app.dnd.collecting = None;
            return false;
        }
    }
    let msg = app.dnd.collecting.take().unwrap_or_default();
    if let Some(body) = msg.strip_prefix("72;") {
        handle(app, body);
    }
    true
}

fn handle(app: &mut App, body: &str) {
    tracing::debug!("dnd: {}", body.chars().take(200).collect::<String>());
    let (meta, payload) = body.split_once(';').unwrap_or((body, ""));
    let key = |k: &str| meta.split(':').find_map(|kv| kv.strip_prefix(k)?.strip_prefix('='));
    let int = |k: &str| key(k).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
    match key("t").unwrap_or("a") {
        "m" => drag_moved(app, int("x"), int("y"), payload),
        "M" => dropped(app, int("x"), int("y"), payload),
        "r" => received(app, int("x") as usize, payload, int("m") == 1),
        "R" => {
            tracing::warn!("drop failed: {payload}");
            app.dnd.reading = None;
        }
        "o" => start_drag(app, int("x") as u16, int("y") as u16),
        "e" => drag_event(app, int("x"), int("y")),
        // The system drag is running: it owns the pointer now
        "E" if payload == "OK" => app.drag_state = crate::events::DragState::Idle,
        "E" => {
            // Not started: the in-window drag carries on as before
            tracing::warn!("drag failed: {payload}");
            app.dnd.outgoing = None;
        }
        _ => {}
    }
}

/// Where a drop at this cell would go: a folder under it, a place in the
/// sidebar, or the current folder.
fn drop_dest(app: &App, col: u16, row: u16) -> (Option<PathBuf>, Option<usize>) {
    if let Some(crate::events::SidebarTarget::Place(j)) = crate::ui::sidebar::target_at(app, col, row) {
        return (app.places.get(j).map(|p| PathBuf::from(&p.path)), None);
    }
    if crate::ui::sidebar::contains(app, col, row) {
        return (None, None);
    }
    if let Some(i) = crate::handler::hit_test(app, col, row)
        && let Some(e) = app.listing.entries.get(i)
        && e.is_dir
    {
        return (Some(e.path.clone()), Some(i));
    }
    let in_main = app.main_area.contains(ratatui::layout::Position { x: col, y: row });
    (in_main.then(|| app.cwd.clone()), None)
}

fn drag_moved(app: &mut App, x: i64, y: i64, mimes: &str) {
    if x < 0 {
        // Left the window
        app.dnd.hover_entry = None;
        app.dnd.replied = None;
        app.dnd.offered.clear();
        return;
    }
    if !mimes.is_empty() {
        app.dnd.offered = mimes.split(' ').map(String::from).collect();
    }
    let (dest, entry) = drop_dest(app, x as u16, y as u16);
    app.dnd.hover_entry = entry;
    let ok = app.dnd.offered.iter().any(|m| m == URI_LIST) && dest.is_some();
    let want = (u8::from(ok), dest);
    if app.dnd.replied.as_ref() != Some(&want) {
        send(&if ok { format!("t=m:o=1;{URI_LIST}") } else { "t=m:o=0".into() });
        app.dnd.replied = Some(want);
    }
}

fn dropped(app: &mut App, x: i64, y: i64, mimes: &str) {
    app.dnd.hover_entry = None;
    app.dnd.replied = None;
    let pos = (x.max(0) as u16, y.max(0) as u16);
    // Our own drag: the terminal won't hand us its data, but we know it
    if let Some(paths) = app.dnd.outgoing.clone() {
        send("t=r:o=1");
        offer_drop(app, paths, pos);
        return;
    }
    match mimes.split(' ').position(|m| m == URI_LIST) {
        Some(i) => {
            app.dnd.reading = Some((i + 1, pos, String::new()));
            send(&format!("t=r:x={}", i + 1));
        }
        None => send("t=r:o=0"),
    }
}

fn received(app: &mut App, idx: usize, chunk: &str, more: bool) {
    let Some((want, pos, data)) = app.dnd.reading.as_mut() else { return; };
    if *want != idx {
        return;
    }
    data.push_str(chunk);
    if more || !chunk.is_empty() {
        return;
    }
    let (pos, data) = (*pos, std::mem::take(data));
    app.dnd.reading = None;
    send("t=r:o=1");
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(data.trim_end_matches('='))
        .unwrap_or_default();
    let paths = parse_uri_list(&String::from_utf8_lossy(&bytes));
    if !paths.is_empty() {
        offer_drop(app, paths, pos);
    }
}

/// Ask what to do with the dropped files, like Dolphin.
fn offer_drop(app: &mut App, paths: Vec<PathBuf>, (col, row): (u16, u16)) {
    let (Some(dest), _) = drop_dest(app, col, row) else { return; };
    // Dropped back where they already are
    if paths.iter().all(|p| p.parent() == Some(dest.as_path()) || *p == dest) {
        return;
    }
    let items = vec![
        MenuItem::new("\u{f0c4}  Move Here", MenuAction::Drop(DropOp::Move)),
        MenuItem::new("\u{f0c5}  Copy Here", MenuAction::Drop(DropOp::Copy)),
        MenuItem::new("\u{f0c1}  Link Here", MenuAction::Drop(DropOp::Link)),
        MenuItem::sep(),
        MenuItem::new("\u{f00d}  Cancel", MenuAction::None),
    ];
    app.dnd.job = Some((paths, dest));
    app.menu = Some(MenuState::new(MenuKind::Context, MenuTarget::Empty, items, (col, row)));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropOp {
    Move,
    Copy,
    Link,
}

/// Run the chosen drop operation.
pub fn perform(app: &mut App, op: DropOp) {
    let Some((paths, dest)) = app.dnd.job.take() else { return; };
    let paths: Vec<PathBuf> = paths.into_iter().filter(|src| *src != dest && !dest.starts_with(src)).collect();
    match op {
        DropOp::Link => {
            for src in paths {
                if let Some(name) = src.file_name() {
                    let _ = std::os::unix::fs::symlink(&src, crate::fileops::free_name(&dest.join(name)));
                }
            }
        }
        DropOp::Move => app.fileops.move_to(paths, &dest),
        DropOp::Copy => app.fileops.copy(paths, &dest),
    }
}

/// The terminal saw a drag gesture at this cell: offer the item under it
/// (with the rest of the selection if it's part of it).
fn start_drag(app: &mut App, col: u16, row: u16) {
    let Some(idx) = crate::handler::hit_test(app, col, row) else { return; };
    let Some(entry) = app.listing.entries.get(idx) else { return; };
    let paths = if app.multi_selection.contains(&idx) || (app.selection == Some(idx) && !app.multi_selection.is_empty()) {
        crate::actions::selected_paths(app)
    } else {
        vec![entry.path.clone()]
    };
    let uris: String = paths.iter().map(|p| {
        let mut uri = crate::thumbs::file_uri(p);
        if p.is_dir() {
            uri.push('/');
        }
        uri + "\r\n"
    }).collect();
    let text: String = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n");

    send(&format!("t=o:o=3;{URI_LIST} text/plain"));
    send_data("t=p:x=0", uris.as_bytes());
    send_data("t=p:x=1", text.as_bytes());
    if let Some(img) = app.icon_cache.image(&paths[0]) {
        let img = img.thumbnail(96, 96).to_rgba8();
        send_data(&format!("t=p:x=-1:y=32:X={}:Y={}", img.width(), img.height()), img.as_raw());
    }
    send("t=P:x=-1");
    app.dnd.outgoing = Some(paths);
}

fn drag_event(app: &mut App, x: i64, y: i64) {
    match x {
        // Finished (dropped or cancelled)
        4 => app.dnd.outgoing = None,
        // Data wanted for a type we didn't pre-send in full
        5 => {
            let Some(paths) = app.dnd.outgoing.clone() else { return; };
            let data = if y == 0 {
                paths.iter().map(|p| crate::thumbs::file_uri(p) + "\r\n").collect::<String>()
            } else {
                paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n")
            };
            send_data(&format!("t=e:y={y}"), data.as_bytes());
        }
        _ => {}
    }
}

pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|uri| {
            let rest = uri.strip_prefix("file://")?;
            // Optional host part: file://localhost/path
            let path = &rest[rest.find('/')?..];
            Some(PathBuf::from(percent_decode(path)))
        })
        .collect()
}

fn percent_decode(s: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    std::ffi::OsString::from_vec(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_list() {
        let paths = parse_uri_list("# comment\r\nfile:///home/u/a%20b.txt\r\nfile://localhost/tmp/x/\r\nhttps://e.com/\r\n");
        assert_eq!(paths, vec![PathBuf::from("/home/u/a b.txt"), PathBuf::from("/tmp/x/")]);
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn reassembles_key_burst() {
        let dir = std::env::temp_dir().join("fbt-dnd");
        let _ = std::fs::create_dir_all(dir.join("sub"));
        std::env::set_current_dir(&dir).unwrap();
        let picker = ratatui_image::picker::Picker::halfblocks();
        let (_a, dr) = std::sync::mpsc::channel();
        let (_b, mr) = std::sync::mpsc::channel();
        let mut app = crate::app::App::new(picker, crate::config::Config::default(), dr, mr).unwrap();
        app.main_area = ratatui::layout::Rect { x: 20, y: 3, width: 80, height: 26 };
        let mut leaked = 0;
        let mut feed = |app: &mut crate::app::App, s: &str| {
            let mut keys = vec![KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)];
            for c in s.chars() {
                let m = if c.is_ascii_uppercase() { KeyModifiers::SHIFT } else { KeyModifiers::NONE };
                keys.push(KeyEvent::new(KeyCode::Char(c), m));
            }
            keys.push(KeyEvent::new(KeyCode::Char('\\'), KeyModifiers::ALT));
            for k in keys { if !swallow(app, &k) { leaked += 1; } }
        };
        feed(&mut app, "72;t=m:x=50:y=20:X=0:Y=0:o=3;text/uri-list text/plain");
        assert_eq!(app.dnd.offered, vec!["text/uri-list", "text/plain"]);
        assert!(app.dnd.replied.as_ref().is_some_and(|r| r.0 == 1));
        feed(&mut app, "72;t=M:x=50:y=20:X=0:Y=0:o=3;text/plain text/uri-list");
        assert_eq!(app.dnd.reading.as_ref().map(|r| r.0), Some(2));
        let b64 = base64::engine::general_purpose::STANDARD.encode("file:///etc/hostname\r\n");
        feed(&mut app, &format!("72;t=r:x=2:m=1;{b64}"));
        feed(&mut app, "72;t=r:x=2:m=0;");
        assert_eq!(leaked, 0);
        assert!(app.menu.is_some());
        assert_eq!(app.dnd.job.as_ref().map(|j| j.0.clone()), Some(vec![PathBuf::from("/etc/hostname")]));
        // A real Alt+] followed by a key lets the key through
        assert!(swallow(&mut app, &KeyEvent::new(KeyCode::Char(']'), KeyModifiers::ALT)));
        assert!(!swallow(&mut app, &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)));
    }
}
