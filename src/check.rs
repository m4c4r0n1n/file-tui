//! `--check`: which optional tools and terminal features this system has,
//! and what each one is for.

use std::path::Path;

/// Whether `name` is an executable on $PATH.
pub fn has_command(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            let p = dir.join(name);
            p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

/// Optional tools: command, what it's for, and whether any of a group will
/// do (the group name).
const TOOLS: &[(&str, &str, Option<&str>)] = &[
    ("rsvg-convert", "drawing the icon theme's SVG icons (librsvg)", None),
    ("wl-paste", "clipboard on Wayland (with wl-copy)", Some("clipboard")),
    ("xclip", "clipboard on X11", Some("clipboard")),
    ("fd", "fast file name search", None),
    ("rg", "search inside files", None),
    ("ffmpeg", "video thumbnails and audio album art", None),
    ("pdftoppm", "PDF thumbnails (if \"pdf\" is in thumbnails)", None),
    ("xdg-open", "opening files with their default app", None),
    ("bsdtar", "Extract (zip, tar, 7z, rar, iso, ...; libarchive)", None),
    ("tar", "Compress to tar.gz", None),
    ("zip", "Compress to zip", None),
    ("udisksctl", "unmounting and safely removing drives", None),
    ("kdeconnect-cli", "Send to Device / browsing phones (KDE Connect)", None),
    ("gio", "file type icons for unusual files (fallback)", None),
];

pub fn run() -> i32 {
    let mut missing_required = false;
    if let Ok(c) = crate::config::Config::load() {
        crate::icons::set_theme(&c.icon_theme);
    }
    println!("file-tui {}\n", env!("CARGO_PKG_VERSION"));

    println!("Terminal");
    let kitty = std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM").is_ok_and(|t| t.contains("kitty"));
    line(kitty, "kitty", "images, smooth menus and drag and drop (other terminals get text-drawn icons)");
    line(has_command("kitty"), "--kitty", "launch in a kitty window with the app's options (use in launchers)");

    println!("\nTools");
    for &(cmd, what, group) in TOOLS {
        let ok = has_command(cmd);
        let alt = group.is_some_and(|g| TOOLS.iter().any(|t| t.2 == Some(g) && t.0 != cmd && has_command(t.0)));
        if ok || !alt {
            line(ok, cmd, what);
        }
    }
    if crate::clipboard::backend().is_none() {
        missing_required = true;
        println!("  ! no usable clipboard tool for this session (Wayland: wl-clipboard, X11: xclip)");
    }

    println!("\nIcons");
    println!("  theme: {} (set icon_theme in the config to change)", crate::icons::theme_name());

    println!("\nFiles");
    let config = crate::config::Config::path();
    line(config.exists(), &config.display().to_string(), "config (created on first run)");
    let trash = crate::fileops::home_trash();
    line(trash.exists() || trash.parent().is_some_and(Path::exists), &trash.display().to_string(), "trash");

    if missing_required { 1 } else { 0 }
}

fn line(ok: bool, name: &str, what: &str) {
    println!("  {} {name:<16} {what}", if ok { "\u{2713}" } else { "\u{2717}" });
}
