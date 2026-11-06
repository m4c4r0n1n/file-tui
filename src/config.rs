use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::app::{SortField, ViewMode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_places")]
    pub places: Vec<PlaceConfig>,
    #[serde(default)]
    pub default_view: ViewMode,
    #[serde(default)]
    pub sort: SortConfig,
    #[serde(default)]
    pub layout: LayoutConfig,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default = "default_true")]
    pub folders_first: bool,
    /// Details view: group rows under headers, like Dolphin's Show in Groups.
    #[serde(default = "default_true")]
    pub show_in_groups: bool,
    #[serde(default = "default_icon_size")]
    pub icon_size_idx: usize,
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: u16,
    /// Terminal emulator for "Open in terminal". Empty = $TERMINAL, fallback kitty.
    #[serde(default)]
    pub terminal_command: String,
    /// Custom folder icon names keyed by absolute directory path.
    #[serde(default)]
    pub folder_icons: HashMap<String, String>,
    #[serde(default)]
    pub context_menu: ContextMenuConfig,
    /// Show previews of a folder's images/videos on its icon, Dolphin-style.
    #[serde(default = "default_true")]
    pub folder_previews: bool,
    /// Which kinds of files show a thumbnail of their content (see
    /// thumbs::KINDS); unset: all but PDFs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnails: Option<Vec<String>>,
    /// Folders (and everything under them) whose thumbnails are made when
    /// shown but never written to disk; "~/" means the home folder.
    #[serde(default)]
    pub thumbnail_cache_exclude: Vec<String>,
    /// KDE Connect: phones under Devices and Send to Device.
    #[serde(default = "default_true")]
    pub kdeconnect: bool,
    /// Icon theme by name (e.g. "Papirus-Dark"); empty: the desktop's.
    #[serde(default)]
    pub icon_theme: String,
    /// Interface colors by role as "#rrggbb" (see ui/theme.rs); unset
    /// roles keep the Nord defaults. Kept as written, so it round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<toml::Value>,
    /// Keyboard shortcuts by action name, e.g. `rename = ["F2", "Alt+R"]`;
    /// replaces that action's defaults, `[]` unbinds it (see keys.rs).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub keys: HashMap<String, Vec<String>>,
    /// Set when the on-disk config failed to parse, so we never overwrite it.
    #[serde(skip)]
    read_only: bool,
}

fn default_icon_size() -> usize { crate::app::DEFAULT_ICON_LEVEL }
fn default_sidebar_w() -> u16 { 18 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextMenuConfig {
    #[serde(default)]
    pub custom: Vec<CustomMenuEntry>,
}

/// User-defined context menu entry.
/// command supports %f (quoted target path) and %d (quoted containing dir).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMenuEntry {
    pub label: String,
    pub command: String,
    /// Any of "file", "dir", "empty". Empty list = all.
    #[serde(default)]
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceConfig {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortConfig {
    #[serde(default)]
    pub field: SortField,
    #[serde(default = "default_true")]
    pub ascending: bool,
}

impl Default for SortConfig {
    fn default() -> Self {
        Self { field: SortField::default(), ascending: true }
    }
}

/// Spacing of the Icons and Compact views.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutConfig {
    /// Columns added to the icon's width for each Icons view cell.
    pub icon_spacing: u16,
    /// Lines of name under each icon.
    pub icon_label_lines: u16,
    /// Width of each Compact view column.
    pub compact_width: u16,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self { icon_spacing: 2, icon_label_lines: 2, compact_width: 22 }
    }
}

impl LayoutConfig {
    /// Within what the views can draw.
    pub fn clamped(self) -> Self {
        Self {
            icon_spacing: self.icon_spacing.min(20),
            icon_label_lines: self.icon_label_lines.clamp(1, 6),
            compact_width: self.compact_width.clamp(8, 80),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_places() -> Vec<PlaceConfig> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/root"));
    let home_str = home.to_string_lossy().to_string();

    let mut places = vec![
        PlaceConfig { name: "Home".into(),      path: home_str.clone(),                         icon: Some("user-home".into()) },
        PlaceConfig { name: "Desktop".into(),   path: format!("{}/Desktop", home_str),          icon: Some("user-desktop".into()) },
        PlaceConfig { name: "Documents".into(), path: format!("{}/Documents", home_str),        icon: Some("folder-documents".into()) },
        PlaceConfig { name: "Downloads".into(), path: format!("{}/Downloads", home_str),        icon: Some("folder-download".into()) },
        PlaceConfig { name: "Music".into(),     path: format!("{}/Music", home_str),            icon: Some("folder-music".into()) },
        PlaceConfig { name: "Pictures".into(),  path: format!("{}/Pictures", home_str),         icon: Some("folder-pictures".into()) },
        PlaceConfig { name: "Videos".into(),    path: format!("{}/Videos", home_str),           icon: Some("folder-videos".into()) },
    ];

    // Trash: freedesktop location
    if let Some(data) = dirs::data_dir() {
        places.push(PlaceConfig {
            name: "Trash".into(),
            path: data.join("Trash/files").to_string_lossy().to_string(),
            icon: Some("user-trash".into()),
        });
    }

    places
}

impl Default for Config {
    fn default() -> Self {
        Self {
            places: default_places(),
            default_view: ViewMode::Icons,
            sort: SortConfig::default(),
            layout: LayoutConfig::default(),
            show_hidden: false,
            folders_first: true,
            show_in_groups: true,
            icon_size_idx: default_icon_size(),
            sidebar_w: 18,
            terminal_command: String::new(),
            folder_icons: HashMap::new(),
            context_menu: ContextMenuConfig::default(),
            folder_previews: true,
            thumbnails: None,
            thumbnail_cache_exclude: Vec::new(),
            icon_theme: String::new(),
            kdeconnect: true,
            keys: HashMap::new(),
            theme: None,
            read_only: false,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
        } else {
            Ok(Config::default())
        }
    }

    /// Defaults that will not be written back (used when loading failed).
    pub fn unwritable() -> Self {
        Self { read_only: true, ..Self::default() }
    }

    /// Write back what the app changes itself (Places, sidebar width),
    /// editing those keys in place so the rest of the file, comments
    /// included, stays as the user wrote it. A first run writes the
    /// commented template.
    pub fn save(&self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        self.save_to(&config_path())
    }

    fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = std::fs::read_to_string(path).unwrap_or_else(|_| template());
        let mut doc: toml_edit::DocumentMut = text.parse()
            .with_context(|| format!("parsing {}", path.display()))?;
        self.apply_to(&mut doc)?;
        let out = doc.to_string();
        if out != text || !path.exists() {
            std::fs::write(path, out)?;
        }
        Ok(())
    }

    fn apply_to(&self, doc: &mut toml_edit::DocumentMut) -> Result<()> {
        let owned: toml_edit::DocumentMut = toml::to_string(&Owned { sidebar_w: self.sidebar_w, places: &self.places })?.parse()?;
        if doc.get("sidebar_w").and_then(|v| v.as_integer()) != Some(self.sidebar_w as i64) {
            doc["sidebar_w"] = owned["sidebar_w"].clone();
        }
        let same_places = doc.get("places").map(|p| p.to_string()) == owned.get("places").map(|p| p.to_string());
        if !same_places {
            doc["places"] = owned["places"].clone();
        }
        Ok(())
    }
}

/// The parts of the config the app itself changes.
#[derive(Serialize)]
struct Owned<'a> {
    sidebar_w: u16,
    places: &'a [PlaceConfig],
}

/// A new user's config: every option, commented out at its default.
fn template() -> String {
    use std::fmt::Write;
    let mut s = String::from(r#"# file-tui configuration. Uncomment a line to change it.
# View settings are remembered per folder; these are the defaults for
# folders you haven't changed.

# default_view = "icons"        # icons, compact or details
# show_hidden = false
# folders_first = true
# show_in_groups = true         # Details view: Dolphin's "Show in Groups"
# icon_size_idx = 7             # 0 (smallest) to 19
# folder_previews = true        # show a folder's pictures on its icon
# terminal_command = ""         # for "Open Terminal Here"; empty = $TERMINAL, then kitty
# kdeconnect = true            # phones under Devices, Send to Device (needs kdeconnect)
# icon_theme = ""               # e.g. "Papirus-Dark"; empty = the desktop's (KDE, GTK or GNOME setting)
sidebar_w = 18                  # sidebar width in columns (also set by dragging its edge)

# Which kinds of files show a thumbnail of their content: images, svg,
# videos, audio (album art), ebooks, comics, documents (OpenDocument), pdf.
# thumbnails = ["images", "svg", "videos", "audio", "ebooks", "comics", "documents"]
# Folders whose thumbnails are made when shown but never saved to disk
# (~/.cache/thumbnails), for privacy. Covers everything under them.
# thumbnail_cache_exclude = ["~/Private"]

# [sort]
# field = "name"                # name, size, modified, created, accessed or type
# ascending = true

# [layout]
# icon_spacing = 2              # Icons view: columns added to each icon's width
# icon_label_lines = 2          # Icons view: lines of name under each icon (1-6)
# compact_width = 22            # Compact view: width of each column

# Custom icons for particular folders, by icon theme name.
# [folder_icons]
# "~/Projects" = "folder-development"

# Extra right-click menu entries. %f is the item, %d its folder.
# [[context_menu.custom]]
# label = "Open in VS Code"
# command = "code %f"
# targets = ["file", "dir"]     # any of file, dir, empty; [] = everywhere

# Keyboard shortcuts. Listing an action replaces its default keys; [] unbinds it.
# Keys are written like "Ctrl+Shift+X", "Alt+F", "F2", "Backspace", "Ctrl+Plus".
# [keys]
"#);
    for (name, _, keys, what) in crate::keys::ACTIONS {
        let keys: Vec<String> = keys.iter().map(|k| format!("{k:?}")).collect();
        let _ = writeln!(s, "# {:<17} = [{}]{:>width$}# {what}", name, keys.join(", "), "", width = 24usize.saturating_sub(keys.join(", ").len() + 2));
    }
    s.push_str("\n# Colors, as \"#rrggbb\". Anything left out keeps the default (Nord).\n# [theme]\n");
    let t = crate::ui::theme::Theme::default();
    let roles: [(&str, Option<crate::ui::theme::Rgb>); 17] = [
        ("text", Some(t.text)), ("text_strong", t.text_strong), ("muted", Some(t.muted)), ("accent", Some(t.accent)),
        ("panel", Some(t.panel)), ("input", Some(t.input)), ("border", Some(t.border)), ("selection", Some(t.selection)),
        ("hover", Some(t.hover)), ("highlight", Some(t.highlight)), ("scrollbar", Some(t.scrollbar)), ("error", Some(t.error)),
        ("danger_bg", Some(t.danger_bg)), ("warning", Some(t.warning)), ("drop_target", Some(t.drop_target)),
        ("drop_target_text", t.drop_target_text), ("ghost", Some(t.ghost)),
    ];
    for (name, c) in roles {
        match c {
            Some((r, g, b)) => { let _ = writeln!(s, "# {name:<17} = \"#{r:02x}{g:02x}{b:02x}\""); }
            None => { let _ = writeln!(s, "# {name:<17} = \"#ffffff\"      # default: the terminal's own color"); }
        }
    }
    s.push_str("\n# Places in the sidebar (also edited from the app: drag to reorder,\n# right-click to add or remove).\n");
    #[derive(Serialize)]
    struct P<'a> { places: &'a [PlaceConfig] }
    s.push_str(&toml::to_string(&P { places: &default_places() }).unwrap_or_default());
    s
}

impl Config {
    pub fn path() -> PathBuf {
        config_path()
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("file-tui/config.toml")
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_and_in_place_save() {
        // The template is all comments: it reads as the defaults
        let t: Config = toml::from_str(&template()).unwrap();
        assert_eq!(t.sidebar_w, Config::default().sidebar_w);
        assert!(t.keys.is_empty() && t.theme.is_none());
        // Uncommenting the documented lines gives valid settings
        let on: String = template().lines()
            .filter(|l| l.starts_with("# ") && l.contains(" = ") && !l.contains("thumbnails = [") && !l.contains("\"~/Projects"))
            .map(|l| format!("{}\n", &l[2..]))
            .collect();
        let keys: String = on.lines().filter(|l| crate::keys::ACTIONS.iter().any(|a| l.starts_with(&format!("{} ", a.0)))).map(|l| format!("{l}\n")).collect();
        let theme: String = on.lines().filter(|l| l.contains("\"#")).map(|l| format!("{l}\n")).collect();
        let parsed: Config = toml::from_str(&format!("[keys]\n{keys}[theme]\n{theme}")).unwrap();
        assert_eq!(parsed.keys.len(), crate::keys::ACTIONS.len());
        assert!(crate::keys::Keymap::new(&parsed.keys).1.is_empty());
        let _: crate::ui::theme::Theme = parsed.theme.unwrap().try_into().unwrap();

        let dir = std::env::temp_dir().join(format!("fbt-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("config.toml");
        let mut c = Config::default();
        c.save_to(&path).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.starts_with("# file-tui configuration") && first.contains("[[places]]"));
        // A user's comment and setting survive the app saving its own changes
        std::fs::write(&path, first.replace("# show_hidden = false", "show_hidden = true  # mine")).unwrap();
        c.sidebar_w = 30;
        c.places.pop();
        c.save_to(&path).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("show_hidden = true  # mine") && after.contains("sidebar_w = 30"));
        let back: Config = toml::from_str(&after).unwrap();
        assert!(back.show_hidden && back.places.len() == c.places.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
