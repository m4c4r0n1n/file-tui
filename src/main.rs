mod actions;
mod app;
mod config;
mod dirstate;
mod dnd;
mod events;
mod fileops;
mod fs;
mod handler;
mod icons;
mod keys;
mod launch;
mod apps;
mod archive;
mod check;
mod clipboard;
mod lineedit;
mod mime;
mod places;
mod search;
mod thumbs;
mod ui;
mod user_actions;

use std::{io::Write, sync::mpsc, time::Duration};

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyEventKind,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal::{SetTitle, supports_keyboard_enhancement}};
use ratatui_image::picker::{Picker, ProtocolType};

use app::App;
use config::Config;

fn main() -> anyhow::Result<()> {
    let _ = rayon::ThreadPoolBuilder::new().num_threads(4).build_global();
    match std::env::args().nth(1).as_deref() {
        Some("--check") => std::process::exit(check::run()),
        Some("--kitty") => {
            // Become a kitty window running the app with its options
            use std::os::unix::process::CommandExt;
            let path = std::env::args_os().nth(2).map(std::path::PathBuf::from)
                .and_then(|p| std::fs::canonicalize(p).ok());
            let Some(mut cmd) = actions::kitty_command(path.as_deref()) else { std::process::exit(1) };
            let err = cmd.exec();
            eprintln!("file-tui: couldn't start kitty: {err}");
            std::process::exit(1);
        }
        Some("--version" | "-V") => {
            println!("file-tui {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--text") => {}
        Some("--help" | "-h") => {
            println!("{}", HELP);
            return Ok(());
        }
        Some(a) if a.starts_with('-') => {
            eprintln!("file-tui: unknown option {a} (see --help)");
            std::process::exit(2);
        }
        _ => {}
    }
    // Log to file (stdout is occupied by TUI)
    let log_file = std::fs::File::create("/tmp/file-tui.log")?;
    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("file_tui=debug")),
        )
        .init();

    let first_run = !Config::path().exists();
    let (config, config_err) = match Config::load() {
        Ok(c) => (c, None),
        Err(e) => {
            tracing::error!("config: {e:#}");
            (Config::unwritable(), Some(e))
        }
    };

    // Enter alternate screen FIRST (required before Picker::from_query_stdio)
    let mut terminal = ratatui::init();

    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;

    // Kitty keyboard protocol: distinguishes Shift+Backspace etc. and reports
    // event kinds (we filter to Press/Repeat so keys don't double-fire).
    let kbd_enhanced = matches!(supports_keyboard_enhancement(), Ok(true));
    if kbd_enhanced {
        let _ = execute!(
            std::io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        );
    }

    // Query terminal capabilities AFTER alternate screen, BEFORE any event::read()
    let picker = if std::env::var_os("TMUX").is_some() {
        Picker::halfblocks()
    } else {
        Picker::from_query_stdio().unwrap_or_else(|e| {
            tracing::warn!("Picker::from_query_stdio failed: {e}");
            Picker::halfblocks()
        })
    };

    tracing::info!(
        "Protocol: {:?}, font_size: {:?}",
        picker.protocol_type(),
        picker.font_size()
    );

    // Background event channels
    let (dir_tx, dir_rx) = mpsc::channel();
    let (mount_tx, mount_rx) = mpsc::channel();

    // Optional start path: a folder opens there, a file opens its folder
    // with it selected
    let text_flag = std::env::args().nth(1).as_deref() == Some("--text");
    let start = std::env::args_os().nth(if text_flag { 2 } else { 1 }).map(std::path::PathBuf::from)
        .and_then(|p| std::fs::canonicalize(p).ok());
    if let Some(p) = &start {
        let dir = if p.is_dir() { Some(p.as_path()) } else { p.parent() };
        if let Some(dir) = dir {
            let _ = std::env::set_current_dir(dir);
        }
    }

    let text_only = text_flag || config.text_only || picker.protocol_type() == ProtocolType::Halfblocks;
    let mut app = App::new(picker, config, dir_rx, mount_rx)?;
    if text_only {
        app.toggle_text_mode();
    }
    // Started in kitty some other way: say once how to get its options
    if first_run
        && app.picker.protocol_type() == ProtocolType::Kitty
        && std::env::var_os(actions::KITTY_ENV).is_none()
    {
        app.notify("Tip: launch with `file-tui --kitty` for Shift-click selection and no window padding".into());
    }
    if let Some(p) = start.filter(|p| !p.is_dir()) {
        app.reveal(&p);
    }

    // Start background watchers
    let dir_watcher = fs::watcher::DirWatcher::spawn(dir_tx);
    if let Some(w) = &dir_watcher {
        w.watch(&app.cwd);
    }
    fs::mounts::spawn_mount_watcher(mount_tx);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags, DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
        hook(info);
    }));

    let is_kitty = app.picker.protocol_type() == ProtocolType::Kitty;
    if is_kitty {
        dnd::enable();
    }
    let result = run(&mut app, &mut terminal, dir_watcher.as_ref());
    if is_kitty {
        dnd::disable();
    }

    if kbd_enhanced {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste)?;
    ratatui::restore();

    // Persist config and per-dir view state
    let _ = build_config(&app).save();
    app.dirstates.save();

    if let Some(e) = config_err {
        eprintln!("file-tui: config not loaded or saved: {e:#}");
    }
    if let Some(e) = &app.dirstates.load_error {
        eprintln!("file-tui: dirstate not loaded or saved: {e}");
    }

    result
}

const HELP: &str = "\
file-tui: a Dolphin-like file manager for kitty

Usage: file-tui [--kitty] [PATH]
  PATH        folder to open, or a file to open its folder with it selected

Options:
  --kitty     open in a new kitty window set up for the app (no padding,
              Shift/Ctrl+Shift clicks reach it); use this in launchers
  --text      use text icons instead of images (F9 toggles)
  --check     list the optional tools and features found on this system
  --version   print the version
  --help      print this help

Config: ~/.config/file-tui/config.toml (keys, theme, places, ...)
In the app, F1 lists the keyboard shortcuts.";

fn run(
    app: &mut App,
    terminal: &mut ratatui::DefaultTerminal,
    dir_watcher: Option<&fs::watcher::DirWatcher>,
) -> anyhow::Result<()> {
    // Bootstrap layout before first render so queue_visible_icons has correct areas
    let sz = terminal.size()?;
    app.main_area = ratatui::layout::Rect {
        x: app.sidebar_w,
        y: app::TOPBAR_H,
        width: sz.width.saturating_sub(app.sidebar_w),
        height: sz.height.saturating_sub(app::TOPBAR_H),
    };
    app.grid_cols = (app.main_area.width / app.cell_w).max(1);
    app.last_terminal_size = ratatui::layout::Size { width: sz.width, height: sz.height };
    app.queue_visible_icons();
    // A new window shows up complete: give the first frame's icons a
    // moment to load instead of drawing without them
    ui::sidebar::request_icons(app);
    let started = std::time::Instant::now();
    while !app.text_mode && started.elapsed() < Duration::from_millis(150)
        && (app.icon_cache.loading() || app.sidebar_icons.loading() || app.thumb_cache.loading())
    {
        std::thread::sleep(Duration::from_millis(2));
        app.drain_channels();
    }

    let is_kitty = app.picker.protocol_type() == ProtocolType::Kitty;
    let mut title_path = std::path::PathBuf::new();
    let mut last_input = std::time::Instant::now();
    let mut last_draw = std::time::Instant::now();

    'frames: loop {
        // Drain background results (icons, thumbs, dir events, mount events)
        app.drain_channels();

        // Render frame. Everything it writes (cells, centered labels, image
        // cleanup) goes in one synchronized update, so the terminal shows
        // whole frames only, never one half-drawn mid-scroll
        let busy = last_input.elapsed() < Duration::from_secs(1)
            || app.icon_cache.loading() || app.sidebar_icons.loading() || app.thumb_cache.loading();
        if busy || app.redraw || app.needs_live_redraw() || last_draw.elapsed() >= Duration::from_secs(1) {
            app.redraw = false;
            last_draw = std::time::Instant::now();
            app.tick_scroll();
            {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(b"\x1b[?2026h");
            }
            terminal.draw(|f| ui::render(f, app))?;
            // Window title follows the current folder, like Dolphin's
            if app.cwd != title_path {
                title_path = app.cwd.clone();
                let _ = execute!(std::io::stdout(), SetTitle(title_path.to_string_lossy()));
            }
            if !app.post_draw.is_empty() {
                let mut out = std::io::stdout().lock();
                for s in app.post_draw.drain(..) {
                    let _ = out.write_all(s.as_bytes());
                }
                let _ = out.flush();
            }

            // Clean up Kitty placements for images that vanished or changed
            // this frame (occluded by overlays, scrolled away, dir changed, ...)
            let (deletions, delete_all) = app.kitty_frame_diff();
            if is_kitty && (delete_all || !deletions.is_empty()) {
                let mut out = std::io::stdout().lock();
                if delete_all {
                    let _ = ui::kitty::delete_all(&mut out);
                } else {
                    let _ = ui::kitty::delete_at(&mut out, &deletions);
                }
                let _ = out.flush();
            }
            {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(b"\x1b[?2026l");
                let _ = out.flush();
            }

            // Queue icons for newly visible cells after render updated grid_cols/grid_area
            app.queue_all_visible_icons();
        }

        // Wait up to a frame for input, then take everything already
        // queued before drawing again: a burst of wheel events is one frame
        let frame = Duration::from_millis(16);
        let batch_start = std::time::Instant::now();
        let mut timeout = if busy { frame } else { Duration::from_millis(250) };
        while event::poll(timeout)? {
            timeout = Duration::ZERO;
            if batch_start.elapsed() > frame {
                break;
            }
            let ev = event::read()?;
            last_input = std::time::Instant::now();
            if let Event::Key(key) = &ev
                && dnd::swallow(app, key)
            {
                // The rest of the message is already buffered: take it
                // whole instead of redrawing per character
                while app.dnd.collecting() && event::poll(Duration::from_millis(50))? {
                    if let Event::Key(key) = event::read()? {
                        dnd::swallow(app, &key);
                    }
                }
                continue;
            }
            match ev {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if handler::handle_key(app, key) {
                        break 'frames;
                    }
                    maybe_restart_watcher(app, dir_watcher);
                }
                Event::Paste(text) => {
                    handler::handle_paste(app, &text);
                    maybe_restart_watcher(app, dir_watcher);
                }
                Event::Mouse(mouse) => {
                    handler::handle_mouse(app, mouse);
                    maybe_restart_watcher(app, dir_watcher);  // restart watcher if navigated
                }
                Event::Resize(w, h) => {
                    let new_size = ratatui::layout::Size { width: w, height: h };
                    if new_size != app.last_terminal_size {
                        app.last_terminal_size = new_size;
                        if is_kitty {
                            // Placeholder images follow their cells through the
                            // reflow; only direct placements (underlays) are
                            // wiped by the screen clear and must be re-placed
                            app.underlays.clear();
                            app.band_shown = None;
                        } else {
                            app.icon_cache.clear_protocols();
                            app.sidebar_icons.clear_protocols();
                            app.thumb_cache.clear_protocols();
                            app.kitty_delete_all = true;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Re-point the dir watcher when cwd has changed from what we're currently watching.
fn maybe_restart_watcher(app: &mut App, dir_watcher: Option<&fs::watcher::DirWatcher>) {
    if app.watched_path != app.cwd {
        app.watched_path = app.cwd.clone();
        if let Some(w) = dir_watcher {
            w.watch(&app.cwd);
        }
    }
}

fn build_config(app: &App) -> Config {
    let mut config = app.config.clone();
    config.places = app.places.iter().map(|p| config::PlaceConfig {
        name: p.name.clone(),
        path: p.path.clone(),
        icon: p.icon.clone(),
    }).collect();
    // View/sort settings are per folder (dirstate.toml); the config's
    // values are the defaults for folders without their own
    config.sidebar_w = app.sidebar_w;
    config
}
