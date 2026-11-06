use ratatui::{
    Frame,
    buffer::CellDiffOption,
    crossterm::event::KeyCode,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use strum::IntoEnumIterator;

use crate::app::{App, SortField, ViewMode};
use crate::ui::{kitty, theme};

fn menu_bg() -> Color { theme::panel() }
fn menu_sel() -> Color { theme::rgb(theme::get().highlight) }

// ---------------------------------------------------------------------------
// Menu data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    None,
    DragOut,
    Open,
    /// Open with `App::open_with[i]`.
    OpenWith(usize),
    OpenTerminal,
    OpenPath,
    OpenPathNewWindow,
    OpenNewWindow,
    OpenNewTab,
    CopyToOtherPane,
    MoveToOtherPane,
    Rename,
    Cut,
    Copy,
    CopyLocation,
    Paste,
    Undo,
    Trash,
    DeletePermanently,
    EmptyTrash,
    NewFolder,
    NewFile,
    CompressTarGz,
    /// Extract the archive(s): autodetecting a folder, or into one named after it.
    ExtractHere,
    ExtractToFolder,
    CompressZip,
    Properties,
    AddToPlaces,
    RemovePlace,
    Unmount,
    /// Unmount and power off the drive (Dolphin's Safely Remove).
    SafelyRemove,
    SetView(ViewMode),
    SetSortField(SortField),
    SetAscending(bool),
    ToggleFoldersFirst,
    ToggleShowHidden,
    ToggleGroups,
    Custom(usize),
    /// (group, action) index into `App::user_actions`.
    UserAction(usize, usize),
    /// Index into `App::kdeconnect_devices`.
    SendToDevice(usize),
    /// What to do with files dropped from another program.
    Drop(crate::dnd::DropOp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    Entry(usize),
    Empty,
    Place(usize),
    Device(usize),
    SidebarEmpty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Context,
    View,
    Sort,
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    /// Theme icon (or path) drawn as an image in the glyph slot.
    pub icon: Option<String>,
    pub action: MenuAction,
    pub enabled: bool,
    pub submenu: Vec<MenuItem>,
    pub separator: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, action: MenuAction) -> Self {
        Self { label: label.into(), icon: None, action, enabled: true, submenu: Vec::new(), separator: false }
    }

    pub fn sub(label: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self { label: label.into(), icon: None, action: MenuAction::None, enabled: true, submenu: items, separator: false }
    }

    pub fn sep() -> Self {
        Self { label: String::new(), icon: None, action: MenuAction::None, enabled: true, submenu: Vec::new(), separator: true }
    }

    /// An item showing an icon image instead of a glyph.
    pub fn with_icon(label: &str, icon: Option<String>, action: MenuAction) -> Self {
        // Three columns: the 2-wide image and a space, like "glyph  "
        let mut item = Self::new(format!("   {label}"), action);
        item.icon = icon;
        item
    }

    pub fn enabled_if(mut self, cond: bool) -> Self {
        self.enabled = cond;
        self
    }

    fn selectable(&self) -> bool {
        !self.separator && self.enabled
    }
}

#[derive(Debug, Clone)]
pub struct MenuState {
    pub kind: MenuKind,
    pub target: MenuTarget,
    pub items: Vec<MenuItem>,
    pub pos: (u16, u16),
    pub selected: usize,
    pub submenu_open: bool,
    pub submenu_selected: usize,
    /// Actual rendered rects, used for hit-testing and occlusion.
    pub rect: Rect,
    pub submenu_rect: Rect,
    /// Kitty layout: two-row items with vertically centered text.
    pub tall: bool,
}

impl MenuState {
    pub fn new(kind: MenuKind, target: MenuTarget, items: Vec<MenuItem>, pos: (u16, u16)) -> Self {
        let selected = items.iter().position(|i| i.selectable()).unwrap_or(0);
        Self {
            kind,
            target,
            items,
            pos,
            selected,
            submenu_open: false,
            submenu_selected: 0,
            rect: Rect::default(),
            submenu_rect: Rect::default(),
            tall: false,
        }
    }
}

pub enum MenuOutcome {
    Consumed,
    Close,
    /// Close the menu and run the action. Bool = click landed outside (close only).
    Action(MenuAction),
    /// Click landed outside the menu.
    Outside,
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// Build the context menu items for a right-click target.
pub fn build_context_menu(app: &App, target: MenuTarget) -> Vec<MenuItem> {
    let has_clip = app.clipboard.is_some() || !crate::actions::system_clipboard_types().is_empty();
    let mut items = Vec::new();

    match target {
        MenuTarget::Entry(idx) => {
            let Some(entry) = app.listing.entries.get(idx) else { return items; };
            let is_dir = entry.is_dir;

            // Files open with the default app (named, with its icon, like
            // Dolphin); folders open here. Other capable apps go in Open With.
            let apps = &app.open_with;
            match apps.first() {
                Some(default) if !is_dir => items.push(MenuItem::with_icon(
                    &format!("Open with {}", default.name), default.icon.clone(), MenuAction::OpenWith(0),
                )),
                _ => items.push(MenuItem::new("\u{f07c}  Open", MenuAction::Open)),
            }
            let skip = usize::from(!is_dir);
            if apps.len() > skip {
                let others = apps.iter().enumerate().skip(skip)
                    .map(|(i, a)| MenuItem::with_icon(&a.name, a.icon.clone(), MenuAction::OpenWith(i)))
                    .collect();
                items.push(MenuItem::sub("\u{f08e}  Open With", others));
            }
            if is_dir {
                items.push(MenuItem::new("\u{f2d2}  Open in New Tab", MenuAction::OpenNewTab));
                items.push(MenuItem::new("\u{f2d0}  Open in New Window", MenuAction::OpenNewWindow));
                items.push(MenuItem::new("\u{f120}  Open in Terminal", MenuAction::OpenTerminal));
            }
            // Search results live elsewhere: go to where they are
            if app.search.as_ref().is_some_and(|s| s.has_query()) {
                items.push(MenuItem::sep());
                items.push(MenuItem::new("\u{f115}  Open Path", MenuAction::OpenPath));
                items.push(MenuItem::new("\u{f2d0}  Open Path in New Window", MenuAction::OpenPathNewWindow));
            }
            items.push(MenuItem::sep());
            items.push(MenuItem::new("\u{f0c4}  Cut", MenuAction::Cut));
            items.push(MenuItem::new("\u{f0c5}  Copy", MenuAction::Copy));
            items.push(MenuItem::new("\u{f0c1}  Copy Location", MenuAction::CopyLocation));
            if is_dir {
                items.push(MenuItem::new("\u{f0ea}  Paste Into Folder", MenuAction::Paste).enabled_if(has_clip));
            }
            items.push(MenuItem::sep());
            if app.split() {
                items.push(MenuItem::new("\u{f0c5}  Copy to Other Side", MenuAction::CopyToOtherPane));
                items.push(MenuItem::new("\u{f0b2}  Move to Other Side", MenuAction::MoveToOtherPane));
                items.push(MenuItem::sep());
            }
            items.push(MenuItem::new("\u{f044}  Rename", MenuAction::Rename));
            if crate::fileops::in_trash(&app.cwd) {
                items.push(MenuItem::new("\u{f1f8}  Permanently Delete", MenuAction::DeletePermanently));
            } else {
                items.push(MenuItem::new("\u{f1f8}  Move to Trash", MenuAction::Trash));
            }
            items.push(MenuItem::sep());
            if !is_dir && crate::archive::is_archive(&entry.path) {
                let folder = crate::archive::stem(&entry.path);
                items.push(MenuItem::sub("\u{f187}  Extract", vec![
                    MenuItem::new("Here, Autodetect Subfolder", MenuAction::ExtractHere),
                    MenuItem::new(&format!("Into \"{folder}\""), MenuAction::ExtractToFolder),
                ]));
            }
            items.push(MenuItem::sub("\u{f1c6}  Compress", vec![
                MenuItem::new("tar.gz", MenuAction::CompressTarGz),
                MenuItem::new("zip", MenuAction::CompressZip),
            ]));
            items.push(MenuItem::new("\u{f0b2}  Drag to App", MenuAction::DragOut));
            push_custom_items(app, &mut items, if is_dir { "dir" } else { "file" });
            let mime = if is_dir {
                "inode/directory".to_string()
            } else {
                entry.mime_type.clone()
                    .or_else(|| crate::mime::guess_by_name(&entry.path))
                    .unwrap_or_else(|| "application/octet-stream".into())
            };
            push_user_actions(app, &mut items, &mime);
            if !is_dir {
                push_send_to_device(app, &mut items);
            }
            items.push(MenuItem::sep());
            items.push(MenuItem::new("\u{f05a}  Properties", MenuAction::Properties));
        }

        MenuTarget::Empty => {
            items.push(MenuItem::sub("\u{f067}  Create New", vec![
                MenuItem::new("\u{f07b}  Folder", MenuAction::NewFolder),
                MenuItem::new("\u{f15b}  File", MenuAction::NewFile),
            ]));
            items.push(MenuItem::sep());
            items.push(MenuItem::new("\u{f0ea}  Paste", MenuAction::Paste).enabled_if(has_clip));
            match app.undo_stack.last() {
                Some((label, _)) => items.push(MenuItem::new(&format!("\u{f0e2}  Undo: {label}"), MenuAction::Undo)),
                None => items.push(MenuItem::new("\u{f0e2}  Undo", MenuAction::Undo).enabled_if(false)),
            }
            items.push(MenuItem::new("\u{f120}  Open Terminal Here", MenuAction::OpenTerminal));
            push_custom_items(app, &mut items, "empty");
            push_user_actions(app, &mut items, "inode/directory");
            items.push(MenuItem::sep());
            items.push(MenuItem::new("\u{f05a}  Properties", MenuAction::Properties));
        }

        MenuTarget::Place(idx) => {
            items.push(MenuItem::new("\u{f07c}  Open", MenuAction::Open));
            items.push(MenuItem::new("\u{f2d2}  Open in New Tab", MenuAction::OpenNewTab));
            items.push(MenuItem::new("\u{f2d0}  Open in New Window", MenuAction::OpenNewWindow));
            let is_trash = app.places.get(idx)
                .map(|p| p.icon.as_deref() == Some("user-trash") || p.path.ends_with("Trash/files"))
                .unwrap_or(false);
            if is_trash {
                items.push(MenuItem::new("\u{f1f8}  Empty Trash", MenuAction::EmptyTrash));
            }
            items.push(MenuItem::sep());
            items.push(MenuItem::new("\u{f068}  Remove from Places", MenuAction::RemovePlace));
            items.push(MenuItem::new("\u{f067}  Add Current Dir to Places", MenuAction::AddToPlaces));
        }

        MenuTarget::Device(idx) => {
            items.push(MenuItem::new("\u{f07c}  Open", MenuAction::Open));
            items.push(MenuItem::new("\u{f2d2}  Open in New Tab", MenuAction::OpenNewTab));
            items.push(MenuItem::new("\u{f2d0}  Open in New Window", MenuAction::OpenNewWindow));
            items.push(MenuItem::new("\u{f120}  Open Terminal Here", MenuAction::OpenTerminal));
            if let Some(dev) = app.devices.get(idx).filter(|d| d.can_unmount()) {
                items.push(MenuItem::sep());
                items.push(MenuItem::with_icon("Unmount", Some("media-eject".into()), MenuAction::Unmount));
                if dev.kind == crate::fs::mounts::MountKind::Media && dev.source.starts_with("/dev/") {
                    items.push(MenuItem::new("\u{f287}  Safely Remove", MenuAction::SafelyRemove));
                }
            }
            items.push(MenuItem::sep());
            items.push(MenuItem::new("\u{f05a}  Properties", MenuAction::Properties));
        }

        MenuTarget::SidebarEmpty => {
            items.push(MenuItem::new("\u{f067}  Add Current Dir to Places", MenuAction::AddToPlaces));
        }
    }

    items
}

fn push_custom_items(app: &App, items: &mut Vec<MenuItem>, target_kind: &str) {
    let custom = &app.config.context_menu.custom;
    let mut added_sep = false;
    for (i, entry) in custom.iter().enumerate() {
        let matches = entry.targets.is_empty()
            || entry.targets.iter().any(|t| t == target_kind);
        if matches {
            if !added_sep {
                items.push(MenuItem::sep());
                added_sep = true;
            }
            items.push(MenuItem::new(format!("\u{f085}  {}", entry.label), MenuAction::Custom(i)));
        }
    }
}

/// Actions from ~/.config/file-tui/actions matching `mime`: top-level
/// ones inline, the rest grouped by submenu name (in file order).
fn push_user_actions(app: &App, items: &mut Vec<MenuItem>, mime: &str) {
    let mut top: Vec<MenuItem> = Vec::new();
    let mut subs: Vec<(String, Vec<MenuItem>)> = Vec::new();
    for (gi, group) in app.user_actions.iter().enumerate() {
        if !group.applies_to(mime) {
            continue;
        }
        let entries = group.actions.iter().enumerate().map(|(ai, a)| {
            MenuItem::new(
                format!("{}  {}", action_glyph(a.icon.as_deref()), a.label),
                MenuAction::UserAction(gi, ai),
            )
        });
        match &group.submenu {
            None => top.extend(entries),
            Some(name) => match subs.iter_mut().find(|(n, _)| n == name) {
                Some((_, v)) => v.extend(entries),
                None => subs.push((name.clone(), entries.collect())),
            },
        }
    }
    if top.is_empty() && subs.is_empty() {
        return;
    }
    // A user action named like a built-in item replaces it (e.g. an
    // imported "Open in Terminal" service menu)
    let plain = |label: &str| label.split_once("  ").map_or(label, |(_, l)| l).trim().to_lowercase();
    let user_labels: Vec<String> = top.iter().map(|i| plain(&i.label)).collect();
    items.retain(|i| i.separator || !user_labels.contains(&plain(&i.label)));
    items.push(MenuItem::sep());
    items.extend(top);
    for (name, sub) in subs {
        items.push(MenuItem::sub(format!("\u{f0ae}  {name}"), sub));
    }
}

/// "Send to Device" submenu listing reachable KDE Connect devices.
fn push_send_to_device(app: &App, items: &mut Vec<MenuItem>) {
    if app.kdeconnect_devices.is_empty() {
        return;
    }
    let devices = app.kdeconnect_devices.iter().enumerate()
        .map(|(i, d)| {
            let label = if d.reachable { d.name.clone() } else { format!("{} (offline)", d.name) };
            let mut item = MenuItem::new(format!("\u{f10b}  {label}"), MenuAction::SendToDevice(i));
            item.enabled = d.reachable;
            item
        })
        .collect();
    items.push(MenuItem::sub("\u{f1d8}  Send to Device", devices));
}

/// Nerd-font glyph for a freedesktop icon name used by an action.
fn action_glyph(icon: Option<&str>) -> &'static str {
    let icon = icon.unwrap_or("");
    match icon {
        i if i.contains("rename") => "\u{f044}",
        i if i.contains("terminal") || i.contains("konsole") => "\u{f120}",
        i if i.contains("wallpaper") => "\u{f03e}",
        i if i.starts_with("video") => "\u{f03d}",
        i if i.starts_with("image") => "\u{f03e}",
        i if i.starts_with("audio") => "\u{f001}",
        i if i.contains("file-manager") || i.contains("folder") => "\u{f07c}",
        i if i.contains("archive") || i.contains("compress") => "\u{f1c6}",
        _ => "\u{f085}",
    }
}

/// Topbar "view" dropdown as a menu.
pub fn build_view_menu(app: &App) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = ViewMode::iter().map(|v| {
        let check = if v == app.view_mode { "\u{f00c} " } else { "  " };
        let icon = view_mode_icon(v);
        MenuItem::new(format!("{check}{icon} {v}"), MenuAction::SetView(v))
    }).collect();
    items.push(MenuItem::sep());
    let check = if app.groups_on() { "\u{f00c} " } else { "  " };
    items.push(MenuItem::new(format!("{check}\u{f0c9} Show in Groups"), MenuAction::ToggleGroups)
        .enabled_if(app.view_mode != ViewMode::Compact));
    items
}

pub fn view_mode_icon(v: ViewMode) -> &'static str {
    match v {
        ViewMode::Icons   => "\u{f009}",
        ViewMode::Compact => "\u{f00b}",
        ViewMode::Details => "\u{f0ca}",
    }
}

/// Topbar "sort" dropdown as a menu.
pub fn build_sort_menu(app: &App) -> Vec<MenuItem> {
    let radio = |on: bool| if on { "\u{f111} " } else { "\u{f10c} " };
    let check = |on: bool| if on { "\u{f14a} " } else { "\u{f096} " };

    let mut items: Vec<MenuItem> = SortField::iter().map(|sf| {
        MenuItem::new(format!("{}{sf}", radio(sf == app.sort.field)), MenuAction::SetSortField(sf))
    }).collect();

    items.push(MenuItem::sep());
    items.push(MenuItem::new(format!("{}A\u{2192}Z", radio(app.sort.ascending)), MenuAction::SetAscending(true)));
    items.push(MenuItem::new(format!("{}Z\u{2192}A", radio(!app.sort.ascending)), MenuAction::SetAscending(false)));
    items.push(MenuItem::sep());
    items.push(MenuItem::new(format!("{}Folders first", check(app.folders_first)), MenuAction::ToggleFoldersFirst));
    items.push(MenuItem::new(format!("{}Show hidden", check(app.show_hidden)), MenuAction::ToggleShowHidden));
    items
}

// ---------------------------------------------------------------------------
// Geometry (shared by render, occlusion pre-pass, and hit-testing)
// ---------------------------------------------------------------------------

fn item_h(item: &MenuItem, tall: bool) -> u16 {
    if tall && !item.separator { 2 } else { 1 }
}

/// Rows above the first item: a border/padding row, none for tall menus
/// (their items carry their own half-row margins).
fn top(tall: bool) -> u16 {
    u16::from(!tall)
}

/// Each item's (y, height) inside a menu at `rect`.
fn item_rows(rect: Rect, items: &[MenuItem], tall: bool) -> Vec<(u16, u16)> {
    let mut y = rect.y + top(tall);
    items.iter().map(|item| {
        let h = item_h(item, tall);
        y += h;
        (y - h, h)
    }).collect()
}

fn menu_size(items: &[MenuItem], tall: bool) -> (u16, u16) {
    let w = items.iter()
        .map(|i| {
            let arrow = if i.submenu.is_empty() { 0 } else { 2 };
            if tall {
                // Centered text: leading (icon) spaces, then the chunks
                // kitty::centered_text lays out, each with a spare column
                let label = i.label.trim_start();
                let lead = (i.label.chars().count() - label.chars().count()) as u16;
                lead + kitty::centered_width(label.chars().count(), item_h(i, tall)) + arrow
            } else {
                i.label.chars().count() as u16 + arrow
            }
        })
        .max()
        .unwrap_or(8) + 4;
    let h = items.iter().map(|i| item_h(i, tall)).sum::<u16>() + 2 * top(tall);
    (w, h)
}

pub fn menu_rect(menu: &MenuState, term: Rect) -> Rect {
    let (w, h) = menu_size(&menu.items, menu.tall);
    let w = w.min(term.width);
    let h = h.min(term.height);
    let x = menu.pos.0.min(term.width.saturating_sub(w));
    let y = menu.pos.1.min(term.height.saturating_sub(h));
    Rect { x, y, width: w, height: h }
}

/// Where the menu (and its open submenu) will be drawn this frame, for
/// layers rendered before it.
pub fn covered(menu: &MenuState, term: Rect, tall: bool) -> Vec<Rect> {
    let mut m = menu.clone();
    m.tall = tall;
    let mut out = vec![menu_rect(&m, term)];
    if m.submenu_open && m.items.get(m.selected).is_some_and(|i| !i.submenu.is_empty()) {
        out.push(submenu_rect(&m, term));
    }
    out
}

pub fn submenu_rect(menu: &MenuState, term: Rect) -> Rect {
    let parent = menu_rect(menu, term);
    let items = &menu.items[menu.selected].submenu;
    let (w, h) = menu_size(items, menu.tall);
    let w = w.min(term.width);
    let h = h.min(term.height);
    let mut x = parent.x + parent.width;
    if x + w > term.width {
        x = parent.x.saturating_sub(w);
    }
    // First item level with the parent item
    let item_y = item_rows(parent, &menu.items, menu.tall).get(menu.selected).map_or(parent.y, |r| r.0);
    let y = item_y.min(term.height.saturating_sub(h));
    Rect { x, y, width: w, height: h }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Where to draw each visible item's icon image: (icon, cell rect, highlighted).
pub fn icon_slots(menu: &MenuState) -> Vec<(String, Rect, bool)> {
    fn list(rect: Rect, items: &[MenuItem], tall: bool, selected: Option<usize>, out: &mut Vec<(String, Rect, bool)>) {
        for (i, (item, (y, h))) in items.iter().zip(item_rows(rect, items, tall)).enumerate() {
            let Some(icon) = item.icon.clone() else { continue; };
            if y + h + top(tall) <= rect.bottom() && rect.width > 4 {
                out.push((icon, Rect { x: rect.x + 2, y, width: 2, height: h }, selected == Some(i) && item.enabled));
            }
        }
    }
    let mut out = Vec::new();
    list(menu.rect, &menu.items, menu.tall, (!menu.submenu_open).then_some(menu.selected), &mut out);
    if menu.submenu_open && let Some(parent) = menu.items.get(menu.selected) {
        list(menu.submenu_rect, &parent.submenu, menu.tall, Some(menu.submenu_selected), &mut out);
    }
    out
}

/// Rows of the highlighted items in a tall menu: the parent's item (kept
/// lit while its submenu is open) and the submenu's.
pub fn highlight_rects(menu: &MenuState) -> (Option<Rect>, Option<Rect>) {
    let at = |rect: Rect, items: &[MenuItem], i: usize| {
        items.get(i).filter(|it| it.selectable())?;
        let (y, h) = item_rows(rect, items, menu.tall)[i];
        (y + h <= rect.bottom()).then_some(Rect { x: rect.x, y, width: rect.width, height: h })
    };
    let main = at(menu.rect, &menu.items, menu.selected);
    let sub = if menu.submenu_open {
        at(menu.submenu_rect, &menu.items[menu.selected].submenu, menu.submenu_selected)
    } else {
        None
    };
    (main, sub)
}

/// Background colors behind menu icons: (normal, highlighted).
pub fn icon_bg() -> ((u8, u8, u8), (u8, u8, u8)) { (theme::get().panel, theme::get().highlight) }
/// Panel drawn under the menu on Kitty (see ui::underlay): fill, outline.
pub fn panel() -> ((u8, u8, u8), (u8, u8, u8)) { (theme::get().panel, theme::get().border) }
/// Selection highlight in panel menus.
pub fn highlight() -> (u8, u8, u8) { theme::get().highlight }

/// `tall`: Kitty draws the background, outline and highlight as images
/// under the cells, and the text centered across two-row items. Returns
/// escape sequences to write after the frame.
pub fn render_menu(f: &mut Frame, menu: &mut MenuState, tall: bool) -> Vec<String> {
    let term = f.area();
    menu.tall = tall;
    let rect = menu_rect(menu, term);
    menu.rect = rect;
    let mut out = Vec::new();
    let parent_sel = if menu.submenu_open && !tall { None } else { Some(menu.selected) };
    render_item_list(f, rect, &menu.items, parent_sel, tall, &mut out);

    if menu.submenu_open {
        let sub_rect = submenu_rect(menu, term);
        menu.submenu_rect = sub_rect;
        let items = menu.items[menu.selected].submenu.clone();
        render_item_list(f, sub_rect, &items, Some(menu.submenu_selected), tall, &mut out);
    } else {
        menu.submenu_rect = Rect::default();
    }
    out
}

fn render_item_list(f: &mut Frame, rect: Rect, items: &[MenuItem], selected: Option<usize>, tall: bool, out: &mut Vec<String>) {
    f.render_widget(Clear, rect);
    if tall {
        render_tall(f, rect, items, selected, out);
        return;
    }

    let inner_w = rect.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = items.iter().enumerate().map(|(i, item)| {
        if item.separator {
            return Line::styled("\u{2500}".repeat(inner_w), Style::default().fg(theme::border()).bg(menu_bg()));
        }
        let arrow = if item.submenu.is_empty() { "" } else { " \u{f0da}" };
        let pad = inner_w
            .saturating_sub(item.label.chars().count() + arrow.chars().count() + 1);
        let text = format!(" {}{}{}", item.label, " ".repeat(pad), arrow);

        let mut style = Style::default().bg(menu_bg());
        if !item.enabled {
            style = style.add_modifier(Modifier::DIM);
        } else if selected == Some(i) {
            style = style.bg(menu_sel()).fg(theme::text_strong()).add_modifier(Modifier::BOLD);
        }
        Line::from(Span::styled(text, style))
    }).collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::border()).bg(menu_bg()))
        .style(Style::default().bg(menu_bg()));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}

fn render_tall(f: &mut Frame, rect: Rect, items: &[MenuItem], selected: Option<usize>, out: &mut Vec<String>) {
    let (text_x, right) = (rect.x + 2, rect.right().saturating_sub(2));
    for (i, (item, (y, h))) in items.iter().zip(item_rows(rect, items, true)).enumerate() {
        if y + h > rect.bottom() {
            break;
        }
        if item.separator {
            let w = rect.width.saturating_sub(2);
            f.render_widget(
                Paragraph::new("\u{2500}".repeat(w as usize)).style(Style::default().fg(theme::border())),
                Rect { x: rect.x + 1, y, width: w, height: 1 },
            );
            continue;
        }
        let fg = if !item.enabled {
            theme::get().muted
        } else if selected == Some(i) {
            theme::text_strong_rgb()
        } else {
            theme::get().text
        };
        let arrow_w = if item.submenu.is_empty() { 0 } else { 2 };
        let mut emit = |x: u16, w: u16, text: &str| {
            if w == 0 { return; }
            let (seq, used) = kitty::centered_text(x, y, h, w, text, fg, None);
            let buf = f.buffer_mut();
            for cy in y..y + h {
                for cx in x..x + used {
                    buf[(cx, cy)].set_symbol(" ").set_diff_option(CellDiffOption::Skip);
                }
            }
            out.push(seq);
        };
        // Icon items keep their leading spaces free for the image
        let label = item.label.trim_start();
        let lead = (item.label.chars().count() - label.chars().count()) as u16;
        emit(text_x + lead, right.saturating_sub(text_x + lead + arrow_w), label);
        if arrow_w > 0 {
            emit(right - arrow_w, arrow_w, "\u{f0da}");
        }
    }
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

fn step_selection(items: &[MenuItem], current: usize, delta: i32) -> usize {
    let len = items.len();
    if len == 0 { return 0; }
    let mut idx = current as i32;
    for _ in 0..len {
        idx += delta;
        if idx < 0 || idx >= len as i32 {
            return current;
        }
        if items[idx as usize].selectable() {
            return idx as usize;
        }
    }
    current
}

pub fn handle_menu_key(menu: &mut MenuState, code: KeyCode) -> MenuOutcome {
    if menu.submenu_open {
        let sub = menu.items[menu.selected].submenu.clone();
        match code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('q') => {
                menu.submenu_open = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                menu.submenu_selected = step_selection(&sub, menu.submenu_selected, -1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                menu.submenu_selected = step_selection(&sub, menu.submenu_selected, 1);
            }
            KeyCode::Enter => {
                if let Some(item) = sub.get(menu.submenu_selected)
                    && item.selectable() {
                        return MenuOutcome::Action(item.action.clone());
                    }
            }
            _ => {}
        }
        return MenuOutcome::Consumed;
    }

    match code {
        KeyCode::Esc | KeyCode::Char('q') => return MenuOutcome::Close,
        KeyCode::Up | KeyCode::Char('k') => {
            menu.selected = step_selection(&menu.items, menu.selected, -1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            menu.selected = step_selection(&menu.items, menu.selected, 1);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if !menu.items[menu.selected].submenu.is_empty() {
                open_submenu(menu);
            }
        }
        KeyCode::Enter => {
            let item = &menu.items[menu.selected];
            if !item.submenu.is_empty() {
                open_submenu(menu);
            } else if item.selectable() {
                return MenuOutcome::Action(item.action.clone());
            }
        }
        _ => {}
    }
    MenuOutcome::Consumed
}

fn open_submenu(menu: &mut MenuState) {
    let first = menu.items[menu.selected].submenu.iter()
        .position(|i| i.selectable())
        .unwrap_or(0);
    menu.submenu_open = true;
    menu.submenu_selected = first;
}

fn item_at(rect: Rect, items: &[MenuItem], tall: bool, col: u16, row: u16) -> Option<usize> {
    if !rect.contains(ratatui::layout::Position { x: col, y: row }) {
        return None;
    }
    item_rows(rect, items, tall).iter().position(|&(y, h)| row >= y && row < y + h && y + h <= rect.bottom())
}

/// Left-click routing while a menu is open.
pub fn handle_menu_click(menu: &mut MenuState, col: u16, row: u16) -> MenuOutcome {
    if menu.submenu_open {
        let sub = menu.items[menu.selected].submenu.clone();
        if let Some(idx) = item_at(menu.submenu_rect, &sub, menu.tall, col, row) {
            if sub[idx].selectable() {
                return MenuOutcome::Action(sub[idx].action.clone());
            }
            return MenuOutcome::Consumed;
        }
        // Click in the parent menu falls through to parent handling
        if !menu.rect.contains(ratatui::layout::Position { x: col, y: row }) {
            return MenuOutcome::Outside;
        }
        menu.submenu_open = false;
    }

    if let Some(idx) = item_at(menu.rect, &menu.items, menu.tall, col, row) {
        let item = &menu.items[idx];
        if item.separator || !item.enabled {
            return MenuOutcome::Consumed;
        }
        if !item.submenu.is_empty() {
            menu.selected = idx;
            open_submenu(menu);
            return MenuOutcome::Consumed;
        }
        return MenuOutcome::Action(item.action.clone());
    }

    if menu.rect.contains(ratatui::layout::Position { x: col, y: row }) {
        return MenuOutcome::Consumed; // border click
    }
    MenuOutcome::Outside
}

/// Mouse-move hover highlighting.
pub fn handle_menu_hover(menu: &mut MenuState, col: u16, row: u16) {
    if menu.submenu_open {
        let sub = &menu.items[menu.selected].submenu;
        if let Some(idx) = item_at(menu.submenu_rect, sub, menu.tall, col, row) {
            if sub[idx].selectable() {
                menu.submenu_selected = idx;
            }
            return;
        }
    }
    // Over the parent: like Qt, a submenu item opens its submenu and any
    // other item closes it
    let Some(idx) = item_at(menu.rect, &menu.items, menu.tall, col, row) else { return; };
    if !menu.items[idx].selectable() || (menu.submenu_open && idx == menu.selected) {
        return;
    }
    menu.selected = idx;
    if menu.items[idx].submenu.is_empty() {
        menu.submenu_open = false;
    } else {
        open_submenu(menu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tall_items_hit_test() {
        let items = vec![
            MenuItem::new("Open", MenuAction::None),
            MenuItem::sep(),
            MenuItem::new("Copy", MenuAction::None),
        ];
        let rect = Rect { x: 10, y: 5, width: 12, height: menu_size(&items, true).1 };
        assert_eq!(rect.height, 5);
        let hits: Vec<_> = (5..10).map(|r| item_at(rect, &items, true, 12, r)).collect();
        assert_eq!(hits, vec![Some(0), Some(0), Some(1), Some(2), Some(2)]);
        assert_eq!(item_at(rect, &items, false, 12, 5), None);
        assert_eq!(item_at(rect, &items, false, 12, 6), Some(0));
    }

    #[test]
    fn tall_width_fits_label() {
        let label = "Set as Wallpaper (swaybg)";
        let items = vec![MenuItem::new(label, MenuAction::None)];
        let (w, _) = menu_size(&items, true);
        let rect = Rect { x: 0, y: 0, width: w, height: 2 };
        let avail = rect.right() - 2 - (rect.x + 2);
        let (_, used) = kitty::centered_text(2, 0, 2, avail, label, (0, 0, 0), None);
        let (_, all) = kitty::centered_text(2, 0, 2, 200, label, (0, 0, 0), None);
        assert_eq!(used, all);
        assert_eq!(kitty::centered_width(label.chars().count(), 2), all);
        for n in 1..60 {
            let text: String = "x".repeat(n);
            assert_eq!(kitty::centered_width(n, 2), kitty::centered_text(0, 0, 2, 500, &text, (0, 0, 0), None).1, "{n}");
        }
    }
}
