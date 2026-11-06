//! Dolphin-style search: file names via fd, file contents via ripgrep.
//! Results stream in from a background reader and replace the listing.

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use ratatui::layout::Rect;

use crate::fs::DirEntry;

/// Results beyond this are dropped (and flagged as truncated).
pub const MAX_RESULTS: usize = 2000;
/// Wait this long after the last keystroke before searching.
const DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Name,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// The folder search was opened in (recursive).
    Here,
    Home,
}

enum SearchMsg {
    Batch(u64, Vec<DirEntry>),
    Done(u64, bool),
}

pub struct SearchState {
    pub query: String,
    /// Cursor byte offset in `query`.
    pub cursor: usize,
    pub mode: SearchMode,
    pub scope: SearchScope,
    /// Keystrokes go to the query field (vs. the results).
    pub focused: bool,
    /// Folder search was opened from.
    pub origin: PathBuf,
    pub running: bool,
    pub truncated: bool,
    /// Query/options changed at this instant; search once it settles.
    dirty_since: Option<Instant>,
    generation: u64,
    child: Option<Child>,
    tx: mpsc::Sender<SearchMsg>,
    rx: mpsc::Receiver<SearchMsg>,

    // Rendered rects for hit-testing
    pub field_rect: Rect,
    pub mode_rects: [Rect; 2],
    pub scope_rects: [Rect; 2],
}

/// What the app should do with the listing after `poll`.
pub enum SearchUpdate {
    None,
    /// Start of a new result set: replace the listing with these.
    Replace(Vec<DirEntry>),
    Append(Vec<DirEntry>),
}

impl SearchState {
    pub fn new(origin: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            query: String::new(),
            cursor: 0,
            mode: SearchMode::Name,
            scope: SearchScope::Here,
            focused: true,
            origin,
            running: false,
            truncated: false,
            dirty_since: None,
            generation: 0,
            child: None,
            tx,
            rx,
            field_rect: Rect::default(),
            mode_rects: [Rect::default(); 2],
            scope_rects: [Rect::default(); 2],
        }
    }

    pub fn root(&self) -> PathBuf {
        match self.scope {
            SearchScope::Here => self.origin.clone(),
            SearchScope::Home => dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
        }
    }

    /// Whether results (rather than the folder) are on screen.
    pub fn has_query(&self) -> bool {
        !self.query.trim().is_empty()
    }

    /// Re-run the search once input settles.
    pub fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode { SearchMode::Name => SearchMode::Content, SearchMode::Content => SearchMode::Name };
        self.mark_dirty();
    }

    pub fn toggle_scope(&mut self) {
        self.scope = match self.scope { SearchScope::Here => SearchScope::Home, SearchScope::Home => SearchScope::Here };
        self.mark_dirty();
    }

    pub fn cancel(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.running = false;
    }

    /// Start a debounced search if due, and collect streamed results.
    pub fn poll(&mut self, show_hidden: bool) -> SearchUpdate {
        let mut update = SearchUpdate::None;

        if self.dirty_since.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            self.dirty_since = None;
            self.start(show_hidden);
            // Clear right away so stale results never mix with new ones
            update = SearchUpdate::Replace(Vec::new());
        }

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                SearchMsg::Batch(generation, entries) if generation == self.generation => {
                    update = match update {
                        SearchUpdate::Replace(mut v) => { v.extend(entries); SearchUpdate::Replace(v) }
                        SearchUpdate::Append(mut v) => { v.extend(entries); SearchUpdate::Append(v) }
                        SearchUpdate::None => SearchUpdate::Append(entries),
                    };
                }
                SearchMsg::Done(generation, truncated) if generation == self.generation => {
                    self.running = false;
                    self.truncated = truncated;
                    if let Some(mut child) = self.child.take() {
                        let _ = child.wait();
                    }
                }
                _ => {}
            }
        }
        update
    }

    fn start(&mut self, show_hidden: bool) {
        self.cancel();
        self.generation += 1;
        self.truncated = false;
        if !self.has_query() {
            return;
        }

        let root = self.root();
        let mut cmd = match self.mode {
            SearchMode::Name => {
                let mut c = Command::new("fd");
                c.args(["--fixed-strings", "--absolute-path", "--color", "never"]);
                if show_hidden { c.arg("--hidden"); }
                c.arg("--").arg(self.query.trim()).arg(&root);
                c
            }
            SearchMode::Content => {
                let mut c = Command::new("rg");
                c.args(["--files-with-matches", "--fixed-strings", "--smart-case", "--no-messages", "--color", "never"]);
                if show_hidden { c.arg("--hidden"); }
                c.arg("--").arg(self.query.trim()).arg(&root);
                c
            }
        };
        let mut child = match cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("search: {e}");
                return;
            }
        };
        let Some(stdout) = child.stdout.take() else { return; };
        self.child = Some(child);
        self.running = true;

        let (tx, generation) = (self.tx.clone(), self.generation);
        std::thread::spawn(move || read_results(stdout, tx, generation));
    }
}

impl Drop for SearchState {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn read_results(stdout: std::process::ChildStdout, tx: mpsc::Sender<SearchMsg>, generation: u64) {
    let mut batch = Vec::new();
    let mut last_send = Instant::now();
    let mut count = 0;
    let mut truncated = false;

    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break; };
        if count >= MAX_RESULTS {
            truncated = true;
            break;
        }
        let path = PathBuf::from(line.trim_end_matches('/'));
        if let Some(entry) = entry_for(&path) {
            batch.push(entry);
            count += 1;
        }
        if batch.len() >= 128 || last_send.elapsed() >= Duration::from_millis(50) {
            if tx.send(SearchMsg::Batch(generation, std::mem::take(&mut batch))).is_err() {
                return;
            }
            last_send = Instant::now();
        }
    }
    let _ = tx.send(SearchMsg::Batch(generation, batch));
    let _ = tx.send(SearchMsg::Done(generation, truncated));
}

fn entry_for(path: &Path) -> Option<DirEntry> {
    let meta = std::fs::metadata(path).ok();
    Some(DirEntry {
        path: path.to_path_buf(),
        name: path.file_name()?.to_string_lossy().into_owned(),
        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
        size: meta.as_ref().and_then(|m| m.is_file().then_some(m.len())),
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        created: meta.as_ref().and_then(|m| m.created().ok()),
        accessed: meta.as_ref().and_then(|m| m.accessed().ok()),
        mime_type: None,
        is_symlink: path.symlink_metadata().is_ok_and(|m| m.is_symlink()),
        depth: 0,
    })
}

