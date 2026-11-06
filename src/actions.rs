use std::{
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    app::App,
    config::PlaceConfig,
    events::{ClipOp, Clipboard, RenameState},
    places::PlaceEntry,
    ui::menu::{MenuAction, MenuTarget},
    ui::properties::PropertiesState,
};

/// Execute a menu action against its target.
pub fn execute(app: &mut App, action: MenuAction, target: MenuTarget) {
    match action {
        MenuAction::None => {}
        MenuAction::Drop(op) => crate::dnd::perform(app, op),

        MenuAction::Open => {
            match target {
                MenuTarget::Entry(idx) => {
                    app.selection = Some(idx);
                    app.enter_selected();
                }
                _ => {
                    if let Some(path) = target_path(app, target)
                        && path.is_dir() { app.navigate_to(path); }
                }
            }
        }

        MenuAction::OpenWith(i) => {
            if let Some(desktop_app) = app.open_with.get(i).cloned() {
                crate::apps::launch(&desktop_app, &action_paths(app, target), &terminal_bin(app));
            }
        }

        MenuAction::OpenPath => {
            if let Some(path) = target_path(app, target) {
                app.reveal(&path);
            }
        }
        MenuAction::OpenNewTab => {
            if let Some(path) = target_path(app, target).filter(|p| p.is_dir()) {
                app.new_tab(path, true);
            }
        }
        MenuAction::CopyToOtherPane => to_other_pane(app, true),
        MenuAction::MoveToOtherPane => to_other_pane(app, false),
        MenuAction::OpenPathNewWindow | MenuAction::OpenNewWindow => {
            if let Some(path) = target_path(app, target) {
                open_new_window(&path);
            }
        }

        MenuAction::OpenTerminal => {
            if let Some(dir) = target_dir(app, target) {
                open_terminal(app, &dir);
            }
        }

        MenuAction::Rename => {
            if let MenuTarget::Entry(idx) = target {
                start_rename(app, idx);
            }
        }

        MenuAction::Cut => clip_target(app, target, ClipOp::Cut),
        MenuAction::Copy => clip_target(app, target, ClipOp::Copy),

        MenuAction::CopyLocation => {
            if let Some(path) = target_path(app, target) {
                copy_to_clipboard(&path.to_string_lossy());
            }
        }

        MenuAction::Undo => app.undo(),
        MenuAction::Paste => {
            if let Some(dest) = target_dir(app, target) {
                paste_clipboard(app, &dest);
            }
        }

        MenuAction::Trash => {
            let paths = action_paths(app, target);
            app.fileops.trash(paths);
            app.selection = None;
            app.multi_selection.clear();
        }

        MenuAction::DeletePermanently => {
            let paths = action_paths(app, target);
            confirm_delete(app, paths);
        }

        MenuAction::EmptyTrash => app.fileops.empty_trash(),
        MenuAction::DragOut => crate::dnd::drag_out(app),

        MenuAction::NewFolder => create_new(app, true),
        MenuAction::NewFile => create_new(app, false),

        MenuAction::CompressTarGz => compress(app, target, true),
        MenuAction::CompressZip => compress(app, target, false),
        MenuAction::ExtractHere => extract(app, target, false),
        MenuAction::ExtractToFolder => extract(app, target, true),

        MenuAction::Properties => {
            if let Some(path) = target_path(app, target) {
                app.properties = Some(PropertiesState::new(&path));
            }
        }

        MenuAction::AddToPlaces => add_to_places(app),
        MenuAction::Unmount | MenuAction::SafelyRemove => {
            if let MenuTarget::Device(idx) = target {
                app.unmount_device(idx, action == MenuAction::SafelyRemove);
            }
        }
        MenuAction::RemovePlace => {
            if let MenuTarget::Place(idx) = target {
                remove_place(app, idx);
            }
        }

        MenuAction::SetView(v) => app.set_view_mode(v),
        MenuAction::SetSortField(f) => app.set_sort_field(f),
        MenuAction::SetAscending(asc) => app.set_sort_ascending(asc),
        MenuAction::ToggleFoldersFirst => app.toggle_folders_first(),
        MenuAction::ToggleGroups => app.toggle_groups(),
        MenuAction::ToggleShowHidden => app.toggle_show_hidden(),

        MenuAction::Custom(idx) => run_custom(app, idx, target),

        MenuAction::UserAction(gi, ai) => {
            let Some(action) = app.user_actions.get(gi).and_then(|g| g.actions.get(ai)) else { return; };
            crate::user_actions::run(action, &action_paths(app, target), &app.cwd);
        }
        MenuAction::SendToDevice(i) => {
            if let Some(device) = app.kdeconnect_devices.get(i) {
                crate::user_actions::kdeconnect_share(device, &action_paths(app, target));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

/// Kitty options every window of the app is launched with (`--kitty`):
/// no padding, and Shift/Ctrl+Shift clicks passed to the app.
const KITTY_ARGS: &[&str] = &[
    "--app-id=file-tui",
    "-o", "window_margin_width=0",
    "-o", "window_padding_width=0",
    "-o", "mouse_map=shift+left press grabbed",
    "-o", "mouse_map=shift+left click grabbed",
    "-o", "mouse_map=shift+left doublepress grabbed",
    "-o", "mouse_map=shift+left triplepress grabbed",
    "-o", "mouse_map=ctrl+shift+left press grabbed",
    "-o", "mouse_map=ctrl+shift+left release grabbed",
    // Tab switching keys go to the app's own tabs
    "-o", "map ctrl+tab",
    "-o", "map ctrl+shift+tab",
];

/// Another window of the app at `path` (a file opens its folder with it
/// selected).
pub fn open_new_window(path: &Path) {
    if let Some(mut cmd) = kitty_command(Some(path)) {
        crate::launch::detach(&mut cmd);
    }
}

/// Set in windows started by `kitty_command`, so the app knows kitty has
/// its options.
pub const KITTY_ENV: &str = "FILE_BROWSER_TUI_KITTY";

/// A kitty window running this app with `KITTY_ARGS`, at `path` (a file
/// opens its folder with it selected) or the current folder.
pub fn kitty_command(path: Option<&Path>) -> Option<Command> {
    let exe = std::env::current_exe().ok()?;
    let mut cmd = Command::new("kitty");
    cmd.args(KITTY_ARGS).env(KITTY_ENV, "1");
    if let Some(path) = path {
        let dir = if path.is_dir() { path } else { path.parent().unwrap_or(path) };
        cmd.arg(format!("--directory={}", dir.display()));
    }
    cmd.arg(exe);
    if let Some(path) = path {
        cmd.arg(path);
    }
    Some(cmd)
}


fn target_path(app: &App, target: MenuTarget) -> Option<PathBuf> {
    match target {
        MenuTarget::Entry(idx) => app.listing.entries.get(idx).map(|e| e.path.clone()),
        MenuTarget::Empty | MenuTarget::SidebarEmpty => Some(app.cwd.clone()),
        MenuTarget::Place(idx) => app.places.get(idx).map(|p| PathBuf::from(&p.path)),
        MenuTarget::Device(idx) => app.devices.get(idx).map(|d| PathBuf::from(&d.path)),
    }
}

/// Files an action applies to: the whole selection when the right-clicked
/// entry is part of it, else just the target.
fn action_paths(app: &App, target: MenuTarget) -> Vec<PathBuf> {
    if let MenuTarget::Entry(idx) = target
        && app.multi_selection.contains(&idx)
    {
        return selected_paths(app);
    }
    target_path(app, target).into_iter().collect()
}

/// Directory the action operates in (the dir itself for dir entries, cwd otherwise).
fn target_dir(app: &App, target: MenuTarget) -> Option<PathBuf> {
    let path = target_path(app, target)?;
    if path.is_dir() { Some(path) } else { path.parent().map(|p| p.to_path_buf()) }
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

fn clip_target(app: &mut App, target: MenuTarget, op: ClipOp) {
    if let Some(path) = target_path(app, target) {
        app.clipboard = Some(Clipboard { paths: vec![path], op });
    }
}

/// Returns the currently selected paths: multi_selection if non-empty, else single selection.
pub fn selected_paths(app: &App) -> Vec<PathBuf> {
    if !app.multi_selection.is_empty() {
        let mut paths: Vec<PathBuf> = app.multi_selection.iter()
            .filter_map(|&i| app.listing.entries.get(i))
            .map(|e| e.path.clone())
            .collect();
        paths.sort();
        paths
    } else {
        app.selection
            .and_then(|i| app.listing.entries.get(i))
            .map(|e| vec![e.path.clone()])
            .unwrap_or_default()
    }
}

pub fn clip_selection(app: &mut App, op: ClipOp) {
    let paths = selected_paths(app);
    if paths.is_empty() {
        return;
    }
    // Also on the system clipboard, so other apps can paste the files
    let list: String = paths.iter().map(|p| crate::thumbs::file_uri(p) + "\r\n").collect();
    crate::clipboard::write(Some("text/uri-list"), list.as_bytes());
    let n = paths.len();
    let what = if n == 1 { "1 item".to_string() } else { format!("{n} items") };
    app.notify(match op {
        ClipOp::Copy => format!("Copied {what}"),
        ClipOp::Cut => format!("Cut {what}"),
    });
    app.clipboard = Some(Clipboard { paths, op });
}

pub fn system_clipboard_types() -> Vec<String> {
    crate::clipboard::types()
}

/// Paste into `dest`: our own copied/cut files, else the system clipboard's
/// files, else its image or text after asking for a file name (Dolphin's
/// "Paste Clipboard Contents").
pub fn paste_clipboard(app: &mut App, dest: &Path) {
    // Ours only while the system clipboard still holds it (or can't be
    // read): something copied elsewhere since then wins
    let ours = app.clipboard.as_ref().is_some_and(|clip| {
        let types = system_clipboard_types();
        types.is_empty()
            || crate::clipboard::read(Some("text/uri-list"))
                .map(|l| crate::dnd::parse_uri_list(&String::from_utf8_lossy(&l)))
                .is_some_and(|paths| paths == clip.paths)
    });
    if !ours {
        app.clipboard = None;
        paste_system(app, dest);
        return;
    }
    let Some(clip) = app.clipboard.clone() else { return; };
    match clip.op {
        ClipOp::Copy => app.fileops.copy(clip.paths, dest),
        ClipOp::Cut => {
            app.fileops.move_to(clip.paths, dest);
            app.clipboard = None;
        }
    }
}

fn paste_system(app: &mut App, dest: &Path) {
    let types = system_clipboard_types();
    let has = |t: &str| types.iter().any(|x| x == t);
    if has("text/uri-list")
        && let Some(list) = crate::clipboard::read(Some("text/uri-list"))
    {
        let paths = crate::dnd::parse_uri_list(&String::from_utf8_lossy(&list));
        if !paths.is_empty() {
            app.fileops.copy(paths, dest);
            return;
        }
    }
    let image = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"]
        .into_iter().find(|t| has(t));
    let (data, stem, ext) = if let Some(t) = image {
        (crate::clipboard::read(Some(t)), "pasted picture", t.trim_start_matches("image/").replace("jpeg", "jpg"))
    } else if types.iter().any(|t| t.starts_with("text/") || t == "UTF8_STRING" || t == "STRING") {
        (crate::clipboard::read(None), "pasted text", "txt".to_string())
    } else {
        (None, "", String::new())
    };
    let Some(data) = data.filter(|d| !d.is_empty()) else {
        app.notify("Nothing to paste".into());
        return;
    };
    let mut name = format!("{stem}.{ext}");
    for n in 2.. {
        if !dest.join(&name).exists() {
            break;
        }
        name = format!("{stem} ({n}).{ext}");
    }
    open_name_prompt(app, crate::events::PromptKind::Paste(data), dest, name);
}

fn open_name_prompt(app: &mut App, kind: crate::events::PromptKind, dest: &Path, placeholder: String) {
    app.name_prompt = Some(crate::events::NamePrompt {
        kind,
        dest: dest.to_path_buf(),
        cursor_pos: 0,
        input: String::new(),
        invalid: false,
        error: String::new(),
        placeholder,
    });
}

/// Create what the name prompt is for; a bad or taken name keeps it open.
pub fn commit_name_prompt(app: &mut App) {
    use crate::events::PromptKind;
    let Some(state) = &mut app.name_prompt else { return; };
    let typed = state.input.trim();
    let name = if typed.is_empty() {
        state.placeholder.clone()
    } else {
        // A pasted picture or text keeps its type when only a name is typed
        let ext = Path::new(&state.placeholder).extension().map(|e| e.to_string_lossy().to_string());
        match ext {
            Some(ext) if matches!(state.kind, PromptKind::Paste(_)) && Path::new(typed).extension().is_none() => format!("{typed}.{ext}"),
            _ => typed.to_string(),
        }
    };
    let name = name.as_str();
    let path = state.dest.join(name);
    let error = if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        Some(format!("\"{name}\" is not a valid name"))
    } else if path.symlink_metadata().is_ok() {
        Some(format!("\"{name}\" already exists here"))
    } else {
        None
    };
    if let Some(error) = error {
        state.invalid = true;
        state.error = error;
        return;
    }
    let state_kind_label = match &state.kind {
        PromptKind::NewFolder => format!("new folder {name}"),
        PromptKind::NewFile => format!("new file {name}"),
        PromptKind::Paste(_) => format!("pasted {name}"),
    };
    let result = match &state.kind {
        PromptKind::NewFolder => std::fs::create_dir(&path),
        PromptKind::NewFile => std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(|_| ()),
        PromptKind::Paste(data) => std::fs::write(&path, data),
    };
    if let Err(e) = result {
        tracing::error!("create {}: {e}", path.display());
        state.invalid = true;
        state.error = e.to_string();
        return;
    }
    app.name_prompt = None;
    app.remember_undo(state_kind_label, vec![crate::fileops::Undo::Created(path.clone())]);
    if path.parent() == Some(app.cwd.as_path()) {
        app.reload_dir();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(idx) = app.listing.entries.iter().position(|e| e.name == name) {
            app.multi_selection.clear();
            app.selection = Some(idx);
            app.selection_anchor = Some(idx);
            app.ensure_selection_visible();
        }
    }
}

/// Copy text to the system clipboard: OSC 52 (works in Kitty) + the
/// desktop's clipboard tool.
fn copy_to_clipboard(text: &str) {
    let b64 = base64_encode(text.as_bytes());
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();

    crate::clipboard::write(None, text.as_bytes());
}

fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---------------------------------------------------------------------------
// File operations
// ---------------------------------------------------------------------------

fn start_rename(app: &mut App, idx: usize) {
    if let Some(entry) = app.listing.entries.get(idx) {
        app.rename_state = Some(RenameState::new(idx, &entry.name.clone()));
        app.selection = Some(idx);
    }
}

pub fn rename_selection(app: &mut App) {
    if let Some(idx) = app.selection {
        start_rename(app, idx);
    }
}

pub fn confirm_delete(app: &mut App, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let display_name = if paths.len() == 1 {
        paths[0].file_name().and_then(|n| n.to_str()).unwrap_or("this item").to_string()
    } else {
        format!("{} items", paths.len())
    };
    app.confirm_delete = Some(crate::events::ConfirmDeleteState { paths, display_name });
}

pub fn trash_selection(app: &mut App) {
    let paths = selected_paths(app);
    if crate::fileops::in_trash(&app.cwd) {
        confirm_delete(app, paths);
        return;
    }
    app.fileops.trash(paths);
    app.selection = None;
    app.multi_selection.clear();
}

pub fn permanent_delete(app: &mut App) {
    if let Some(cd) = app.confirm_delete.take() {
        app.fileops.delete(cd.paths);
        app.selection = None;
        app.multi_selection.clear();
    }
}

/// Find an unused "New Folder"/"New File (2)" style name in dir.
fn unique_name(dir: &Path, base: &str) -> PathBuf {
    let candidate = dir.join(base);
    if !candidate.exists() {
        return candidate;
    }
    for n in 2.. {
        let candidate = dir.join(format!("{base} ({n})"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Ask for the new folder's or file's name (created on confirming).
fn create_new(app: &mut App, is_dir: bool) {
    let base = if is_dir { "New Folder" } else { "New File" };
    let suggestion = unique_name(&app.cwd, base);
    let name = suggestion.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let kind = if is_dir { crate::events::PromptKind::NewFolder } else { crate::events::PromptKind::NewFile };
    let cwd = app.cwd.clone();
    open_name_prompt(app, kind, &cwd, name);
}

fn compress(app: &mut App, target: MenuTarget, targz: bool) {
    let Some(src) = target_path(app, target) else { return; };
    let Some(parent) = src.parent().map(|p| p.to_path_buf()) else { return; };
    let Some(name) = src.file_name().map(|n| n.to_string_lossy().to_string()) else { return; };

    if targz {
        let out = unique_name(&parent, &format!("{name}.tar.gz"));
        crate::launch::detach(Command::new("tar")
            .args(["-czf", out.to_string_lossy().as_ref(), "-C", parent.to_string_lossy().as_ref(), &name]));
    } else {
        let out = unique_name(&parent, &format!("{name}.zip"));
        let out_name = out.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        crate::launch::detach(Command::new("zip").current_dir(&parent).args(["-r", &out_name, &name]));
    }
}

/// Copy or move the selection to the folder shown on the other side of a
/// split.
pub fn to_other_pane(app: &mut App, copy: bool) {
    let Some(dest) = app.other_pane_cwd() else {
        app.notify("Split the view (F3) to copy or move to the other side".into());
        return;
    };
    let paths = selected_paths(app);
    if paths.is_empty() || dest == app.cwd {
        return;
    }
    if copy {
        app.fileops.copy(paths, &dest);
    } else {
        app.fileops.move_to(paths, &dest);
    }
}

fn extract(app: &mut App, target: MenuTarget, own_folder: bool) {
    let archives: Vec<PathBuf> = action_paths(app, target).into_iter()
        .filter(|p| crate::archive::is_archive(p))
        .collect();
    for archive in archives {
        let name = archive.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        app.notify(format!("Extracting {name}\u{2026}"));
        let tx = app.job_tx.clone();
        std::thread::spawn(move || {
            let folder = own_folder.then(|| crate::archive::stem(&archive));
            let notice = match crate::archive::extract(&archive, folder.as_deref()) {
                Ok(made) => format!("Extracted {name} to {}", made.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
                Err(e) => format!("Couldn't extract {name}: {e}"),
            };
            let _ = tx.send(notice);
        });
    }
}

fn terminal_command(app: &App) -> String {
    if !app.config.terminal_command.is_empty() {
        app.config.terminal_command.clone()
    } else {
        std::env::var("TERMINAL").unwrap_or_else(|_| "kitty".into())
    }
}

/// Terminal program for apps that need one (Terminal=true).
fn terminal_bin(app: &App) -> String {
    terminal_command(app).split_whitespace().next().unwrap_or("kitty").to_string()
}

fn open_terminal(app: &App, dir: &Path) {
    let cmd = terminal_command(app);
    let mut parts = cmd.split_whitespace();
    let Some(bin) = parts.next() else { return; };
    crate::launch::detach(Command::new(bin).args(parts).current_dir(dir));
}

// ---------------------------------------------------------------------------
// Places
// ---------------------------------------------------------------------------

fn add_to_places(app: &mut App) {
    let path = app.cwd.to_string_lossy().to_string();
    if app.places.iter().any(|p| p.path == path) {
        return;
    }
    let name = app.cwd.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "/".into());
    app.places.push(PlaceEntry { name: name.clone(), path: path.clone(), icon: None });
    app.config.places.push(PlaceConfig { name, path, icon: None });
    let _ = app.config.save();
}

fn remove_place(app: &mut App, idx: usize) {
    if idx < app.places.len() {
        let removed = app.places.remove(idx);
        app.config.places.retain(|p| p.path != removed.path);
        let _ = app.config.save();
    }
}

// ---------------------------------------------------------------------------
// Custom config-defined entries
// ---------------------------------------------------------------------------

fn run_custom(app: &App, idx: usize, target: MenuTarget) {
    let Some(entry) = app.config.context_menu.custom.get(idx) else { return; };
    let Some(path) = target_path(app, target) else { return; };
    let dir = if path.is_dir() { path.clone() } else {
        path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| app.cwd.clone())
    };

    let cmd = entry.command
        .replace("%f", &shell_quote(&path.to_string_lossy()))
        .replace("%d", &shell_quote(&dir.to_string_lossy()));

    crate::launch::detach(Command::new("sh").args(["-c", &cmd]).current_dir(&dir));
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

