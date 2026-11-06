//! Configurable keyboard shortcuts: named actions bound to key combos,
//! with Dolphin's defaults. `[keys]` in the config replaces an action's
//! defaults (an empty list unbinds it).

use std::collections::HashMap;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Undo,
    Open,
    Cancel,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    First,
    Last,
    PageUp,
    PageDown,
    SelectToFirst,
    SelectToLast,
    SelectPageUp,
    SelectPageDown,
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    SplitView,
    OtherPane,
    CopyToOtherPane,
    MoveToOtherPane,
    Help,
    Search,
    SelectAll,
    EditPath,
    Back,
    Forward,
    Up,
    Reload,
    ToggleHidden,
    NewFolder,
    NewFile,
    Rename,
    Copy,
    Cut,
    Paste,
    CopyLocation,
    Trash,
    Delete,
    Properties,
    OpenTerminal,
    ZoomIn,
    ZoomOut,
    ViewIcons,
    ViewCompact,
    ViewDetails,
    SidebarNarrower,
    SidebarWider,
}

/// Config name, action, Dolphin's default keys, and the help text.
pub const ACTIONS: &[(&str, Action, &[&str], &str)] = &[
    ("quit", Action::Quit, &["Ctrl+Q"], "Quit"),
    ("open", Action::Open, &["Enter"], "Open"),
    ("cancel", Action::Cancel, &["Esc"], "Clear the selection / close search"),
    ("move_left", Action::MoveLeft, &["Left"], "Move left (Details: collapse)"),
    ("move_right", Action::MoveRight, &["Right"], "Move right (Details: expand)"),
    ("move_up", Action::MoveUp, &["Up"], "Move up"),
    ("move_down", Action::MoveDown, &["Down"], "Move down"),
    ("select_left", Action::SelectLeft, &["Shift+Left"], "Extend selection left"),
    ("select_right", Action::SelectRight, &["Shift+Right"], "Extend selection right"),
    ("select_up", Action::SelectUp, &["Shift+Up"], "Extend selection up"),
    ("select_down", Action::SelectDown, &["Shift+Down"], "Extend selection down"),
    ("first", Action::First, &["Home"], "First item"),
    ("last", Action::Last, &["End"], "Last item"),
    ("page_up", Action::PageUp, &["PageUp"], "Up a page"),
    ("page_down", Action::PageDown, &["PageDown"], "Down a page"),
    ("select_to_first", Action::SelectToFirst, &["Shift+Home"], "Extend selection to the first item"),
    ("select_to_last", Action::SelectToLast, &["Shift+End"], "Extend selection to the last item"),
    ("select_page_up", Action::SelectPageUp, &["Shift+PageUp"], "Extend selection up a page"),
    ("select_page_down", Action::SelectPageDown, &["Shift+PageDown"], "Extend selection down a page"),
    ("help", Action::Help, &["F1"], "Show this help"),
    ("search", Action::Search, &["Ctrl+F"], "Search"),
    ("select_all", Action::SelectAll, &["Ctrl+A"], "Select all"),
    ("edit_path", Action::EditPath, &["Ctrl+L", "F6", "Alt+D"], "Edit the location"),
    ("back", Action::Back, &["Backspace", "Alt+Left"], "Back"),
    ("forward", Action::Forward, &["Alt+Right"], "Forward"),
    ("up", Action::Up, &["Alt+Up"], "Up a folder"),
    ("reload", Action::Reload, &["F5"], "Reload"),
    ("toggle_hidden", Action::ToggleHidden, &["Ctrl+H", "Alt+."], "Show/hide hidden files"),
    ("new_folder", Action::NewFolder, &["F10"], "New folder"),
    ("new_file", Action::NewFile, &[], "New file"),
    ("rename", Action::Rename, &["F2"], "Rename"),
    ("copy", Action::Copy, &["Ctrl+C"], "Copy"),
    ("cut", Action::Cut, &["Ctrl+X"], "Cut"),
    ("paste", Action::Paste, &["Ctrl+V"], "Paste"),
    ("copy_location", Action::CopyLocation, &["Ctrl+Alt+C"], "Copy location"),
    ("undo", Action::Undo, &["Ctrl+Z"], "Undo"),
    ("trash", Action::Trash, &["Delete"], "Move to trash"),
    ("delete", Action::Delete, &["Shift+Delete"], "Delete permanently"),
    ("properties", Action::Properties, &["Alt+Enter"], "Properties"),
    ("open_terminal", Action::OpenTerminal, &["Shift+F4"], "Open terminal here"),
    ("zoom_in", Action::ZoomIn, &["Ctrl+Plus", "Ctrl+="], "Larger icons"),
    ("zoom_out", Action::ZoomOut, &["Ctrl+Minus"], "Smaller icons"),
    ("view_icons", Action::ViewIcons, &["Ctrl+1"], "Icons view"),
    ("view_compact", Action::ViewCompact, &["Ctrl+2"], "Compact view"),
    ("view_details", Action::ViewDetails, &["Ctrl+3"], "Details view"),
    ("sidebar_narrower", Action::SidebarNarrower, &["Alt+["], "Narrower sidebar"),
    ("sidebar_wider", Action::SidebarWider, &["Alt+]"], "Wider sidebar"),
    ("new_tab", Action::NewTab, &["Ctrl+T"], "New tab"),
    ("close_tab", Action::CloseTab, &["Ctrl+W"], "Close tab"),
    ("next_tab", Action::NextTab, &["Ctrl+Tab", "Ctrl+PageDown"], "Next tab"),
    ("prev_tab", Action::PrevTab, &["Ctrl+Shift+Tab", "Ctrl+PageUp"], "Previous tab"),
    ("split_view", Action::SplitView, &["F3"], "Split view on/off"),
    ("other_pane", Action::OtherPane, &["Ctrl+F3"], "Switch to the other side of a split"),
    ("copy_to_other_pane", Action::CopyToOtherPane, &[], "Copy to the other side of a split"),
    ("move_to_other_pane", Action::MoveToOtherPane, &[], "Move to the other side of a split"),
];

/// A key combo, normalized so a parsed spec and a key event compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Combo {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Combo {
    fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mods = mods & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER);
        match code {
            // Letters compare lowercase with Shift; for symbols, Shift is
            // part of which symbol it is ("?" not "Shift+/")
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                let mods = if c.is_ascii_uppercase() { mods | KeyModifiers::SHIFT } else { mods };
                Self { code: KeyCode::Char(c.to_ascii_lowercase()), mods }
            }
            KeyCode::Char(c) => Self { code: KeyCode::Char(c), mods: mods - KeyModifiers::SHIFT },
            KeyCode::BackTab => Self { code: KeyCode::Tab, mods: mods | KeyModifiers::SHIFT },
            code => Self { code, mods },
        }
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        Self::new(ev.code, ev.modifiers)
    }

    /// Parse "Ctrl+Shift+X", "Alt+F", "F10", "Backspace", ".", "Ctrl+Plus".
    pub fn parse(spec: &str) -> Option<Self> {
        let spec = spec.trim();
        // A trailing "+" is the plus key itself ("Ctrl++")
        let (mods_part, key) = match spec.strip_suffix("++") {
            Some(m) => (m, "+"),
            None => match spec.rsplit_once('+') {
                Some((m, k)) if !k.is_empty() => (m, k),
                _ => ("", spec),
            },
        };
        let mut mods = KeyModifiers::NONE;
        for m in mods_part.split('+').filter(|m| !m.is_empty()) {
            mods |= match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => KeyModifiers::CONTROL,
                "alt" | "meta" => KeyModifiers::ALT,
                "shift" => KeyModifiers::SHIFT,
                "super" | "win" => KeyModifiers::SUPER,
                _ => return None,
            };
        }
        let lower = key.to_ascii_lowercase();
        let code = match lower.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "insert" | "ins" => KeyCode::Insert,
            "tab" => KeyCode::Tab,
            "space" => KeyCode::Char(' '),
            "plus" => KeyCode::Char('+'),
            "minus" => KeyCode::Char('-'),
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() =>
                KeyCode::F(f[1..].parse().ok()?),
            _ => {
                let mut chars = key.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                // "Alt+R" names the R key; Shift has to be spelled out
                KeyCode::Char(c.to_ascii_lowercase())
            }
        };
        Some(Self::new(code, mods))
    }

    /// Display form, e.g. "Ctrl+Shift+X".
    pub fn label(&self) -> String {
        let mut s = String::new();
        for (m, name) in [(KeyModifiers::CONTROL, "Ctrl+"), (KeyModifiers::ALT, "Alt+"), (KeyModifiers::SHIFT, "Shift+"), (KeyModifiers::SUPER, "Super+")] {
            if self.mods.contains(m) {
                s.push_str(name);
            }
        }
        match self.code {
            KeyCode::Char('+') => s.push_str("Plus"),
            KeyCode::Char('-') => s.push_str("Minus"),
            KeyCode::Char(' ') => s.push_str("Space"),
            KeyCode::Char(c) => s.push(c.to_ascii_uppercase()),
            KeyCode::F(n) => s.push_str(&format!("F{n}")),
            KeyCode::Enter => s.push_str("Enter"),
            KeyCode::Backspace => s.push_str("Backspace"),
            KeyCode::Delete => s.push_str("Delete"),
            code => s.push_str(&format!("{code:?}")),
        }
        s
    }
}

pub struct Keymap {
    map: HashMap<Combo, Action>,
    /// Keys per action, for the help screen.
    pub bound: Vec<(Action, Vec<Combo>)>,
}

impl Keymap {
    /// Defaults overridden by the config's `[keys]`; returns the problems
    /// (unknown actions or keys) to report.
    pub fn new(overrides: &HashMap<String, Vec<String>>) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        for name in overrides.keys() {
            if !ACTIONS.iter().any(|(n, ..)| n == name) {
                problems.push(format!("unknown action \"{name}\""));
            }
        }
        let mut map = HashMap::new();
        let mut bound = Vec::new();
        for &(name, action, defaults, _) in ACTIONS {
            let specs: Vec<&str> = match overrides.get(name) {
                Some(v) => v.iter().map(String::as_str).collect(),
                None => defaults.to_vec(),
            };
            let mut combos = Vec::new();
            for spec in specs {
                match Combo::parse(spec) {
                    Some(c) => {
                        map.insert(c, action);
                        combos.push(c);
                    }
                    None => problems.push(format!("{name}: can't read key \"{spec}\"")),
                }
            }
            bound.push((action, combos));
        }
        (Self { map, bound }, problems)
    }

    pub fn action(&self, ev: &KeyEvent) -> Option<Action> {
        self.map.get(&Combo::from_event(ev)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn every_default_key_parses_and_is_its_actions_alone() {
        let (keymap, problems) = Keymap::new(&HashMap::new());
        assert!(problems.is_empty(), "{problems:?}");
        let mut seen = HashMap::new();
        for (name, action, keys, _) in ACTIONS {
            for spec in *keys {
                let combo = Combo::parse(spec).unwrap_or_else(|| panic!("{name}: {spec}"));
                if let Some(other) = seen.insert(combo, *name) {
                    panic!("{spec} is the default for both {other} and {name}");
                }
                assert_eq!(keymap.map.get(&combo), Some(action), "{name}: {spec}");
            }
        }
    }

    #[test]
    fn parse_and_match() {
        let (km, problems) = Keymap::new(&HashMap::from([
            ("rename".to_string(), vec!["F2".to_string(), "Alt+R".to_string()]),
            ("toggle_hidden".to_string(), vec![]),
            ("bogus".to_string(), vec!["Ctrl+Nope".to_string()]),
        ]));
        assert_eq!(problems, vec!["unknown action \"bogus\"".to_string()]);
        assert_eq!(km.action(&ev(KeyCode::Char('r'), KeyModifiers::ALT)), Some(Action::Rename));
        assert_eq!(km.action(&ev(KeyCode::Char('h'), KeyModifiers::CONTROL)), None);
        assert_eq!(km.action(&ev(KeyCode::Char('q'), KeyModifiers::CONTROL)), Some(Action::Quit));
        // Shifted letter arrives uppercase, with or without the flag
        let (km, _) = Keymap::new(&HashMap::from([("copy".to_string(), vec!["Ctrl+Shift+C".to_string()])]));
        assert_eq!(km.action(&ev(KeyCode::Char('C'), KeyModifiers::CONTROL)), Some(Action::Copy));
        assert_eq!(km.action(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Action::Copy));
        assert_eq!(km.action(&ev(KeyCode::Char('c'), KeyModifiers::CONTROL)), None);
        // Symbols ignore Shift; "Plus" and "Ctrl++" are the same key
        assert_eq!(Combo::parse("Ctrl++"), Combo::parse("Ctrl+Plus"));
        let (km, _) = Keymap::new(&HashMap::new());
        assert_eq!(km.action(&ev(KeyCode::Char('+'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)), Some(Action::ZoomIn));
        assert_eq!(km.action(&ev(KeyCode::Char('.'), KeyModifiers::NONE)), None);
        assert_eq!(Combo::parse("Shift+Delete").unwrap().label(), "Shift+Delete");
    }
}
