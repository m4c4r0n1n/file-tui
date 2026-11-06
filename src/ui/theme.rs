use ratatui::style::Color;
use serde::Deserialize;

pub type Rgb = (u8, u8, u8);

/// Interface colors by role. Defaults are the Nord palette; `[theme]` in
/// the config overrides any of them with "#rrggbb".
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    /// Body text (nord4).
    #[serde(with = "hex")] pub text: Rgb,
    /// Text on selections and in active fields. Unset: the terminal's
    /// own bright white (palette color 15).
    #[serde(with = "hex_opt")] pub text_strong: Option<Rgb>,
    /// Secondary text: dimmer than the body text but readable on dark and
    /// translucent terminal backgrounds (nord3 is too dark for text).
    #[serde(with = "hex")] pub muted: Rgb,
    /// Section headers and column titles (nord8).
    #[serde(with = "hex")] pub accent: Rgb,
    /// Recessed fields, menus and popups (nord0).
    #[serde(with = "hex")] pub panel: Rgb,
    /// Text fields on the window background (location, search): light
    /// enough to stand out on a dark translucent terminal.
    #[serde(with = "hex")] pub input: Rgb,
    /// Borders and separators (nord3).
    #[serde(with = "hex")] pub border: Rgb,
    /// Selected items: 50% blend of nord9 with nord0.
    #[serde(with = "hex")] pub selection: Rgb,
    /// Hovered items: a faint lift, clearly weaker than the selection.
    #[serde(with = "hex")] pub hover: Rgb,
    /// Menu highlight, rubber band, active scrollbar (nord9).
    #[serde(with = "hex")] pub highlight: Rgb,
    #[serde(with = "hex")] pub scrollbar: Rgb,
    /// Invalid names, delete confirmation border (nord11).
    #[serde(with = "hex")] pub error: Rgb,
    /// Background of the permanent-delete confirmation.
    #[serde(with = "hex")] pub danger_bg: Rgb,
    /// Status notices (nord13).
    #[serde(with = "hex")] pub warning: Rgb,
    /// A folder a drag would drop into (nord14), and its text.
    #[serde(with = "hex")] pub drop_target: Rgb,
    /// Unset: the terminal's black (palette color 0).
    #[serde(with = "hex_opt")] pub drop_target_text: Option<Rgb>,
    /// The drag ghost away from any target (nord2).
    #[serde(with = "hex")] pub ghost: Rgb,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: (216, 222, 233),
            text_strong: None,
            muted: (150, 160, 180),
            accent: (136, 192, 208),
            panel: (46, 52, 64),
            input: (72, 82, 102),
            border: (76, 86, 106),
            selection: (88, 107, 128),
            hover: (60, 68, 84),
            highlight: (129, 161, 193),
            scrollbar: (110, 121, 143),
            error: (191, 97, 106),
            danger_bg: (59, 20, 20),
            warning: (235, 203, 139),
            drop_target: (163, 190, 140),
            drop_target_text: None,
            ghost: (67, 76, 94),
        }
    }
}

mod hex {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<super::Rgb, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_hex(&s).ok_or_else(|| serde::de::Error::custom(format!("\"{s}\" isn't a #rrggbb color")))
    }
}

pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

mod hex_opt {
    use serde::Deserializer;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<super::Rgb>, D::Error> {
        super::hex::deserialize(d).map(Some)
    }
}

static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();

/// Install the configured theme; call once at startup before drawing.
pub fn set(theme: Theme) {
    let _ = THEME.set(theme);
}

pub fn get() -> &'static Theme {
    THEME.get_or_init(Theme::default)
}

pub const fn rgb((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

pub fn text() -> Color { rgb(get().text) }
pub fn text_strong() -> Color { get().text_strong.map_or(Color::White, rgb) }
/// For drawing paths that need an exact color (kitty text sizing).
pub fn text_strong_rgb() -> Rgb { get().text_strong.unwrap_or((255, 255, 255)) }
pub fn muted() -> Color { rgb(get().muted) }
pub fn accent() -> Color { rgb(get().accent) }
pub fn panel() -> Color { rgb(get().panel) }
pub fn input() -> Color { rgb(get().input) }
pub fn border() -> Color { rgb(get().border) }
pub fn selection() -> Color { rgb(get().selection) }
pub fn hover() -> Color { rgb(get().hover) }
pub fn danger_bg() -> Color { rgb(get().danger_bg) }
pub fn drop_target_text() -> Color { get().drop_target_text.map_or(Color::Black, rgb) }

#[cfg(test)]
mod tests {
    #[test]
    fn overrides() {
        let t: super::Theme = toml::from_str("accent = \"#ff8000\"").unwrap();
        assert_eq!(t.accent, (255, 128, 0));
        assert_eq!(t.text, super::Theme::default().text);
        assert!(toml::from_str::<super::Theme>("accent = \"orange\"").is_err());
        assert!(toml::from_str::<super::Theme>("acent = \"#ffffff\"").is_err());
    }
}
