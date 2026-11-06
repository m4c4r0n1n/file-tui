use std::path::PathBuf;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Permanent-delete confirmation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ConfirmDeleteState {
    pub paths: Vec<PathBuf>,
    pub display_name: String,
}

// ---------------------------------------------------------------------------
// Clipboard (cut / copy / paste)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipOp {
    Copy,
    Cut,
}

#[derive(Debug, Clone)]
pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub op: ClipOp,
}

// ---------------------------------------------------------------------------
// Sidebar hit-testing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarTarget {
    Place(usize),
    Device(usize),
    /// A paired KDE Connect device (index into `kdeconnect_devices`).
    Phone(usize),
}

// ---------------------------------------------------------------------------
// Drag state machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub enum DragState {
    #[default]
    Idle,
    Armed {
        item_idx: usize,
        origin: (u16, u16),
    },
    Dragging {
        item_idx: usize,
        origin: (u16, u16),
        current_pos: (u16, u16),
        hover_target: Option<usize>,
    },
}

pub const DRAG_THRESHOLD: u16 = 2;

/// A press on a place in the sidebar: a click opens it on release; moving
/// the mouse first turns it into a reorder drag.
#[derive(Debug, Clone, Copy)]
pub struct PlaceDrag {
    pub from: usize,
    pub origin_row: u16,
    pub active: bool,
    /// Place the dragged one would take the position of.
    pub target: Option<usize>,
}

// ---------------------------------------------------------------------------
// Double-click tracker
// ---------------------------------------------------------------------------

const DOUBLE_CLICK_MS: u128 = 400;

#[derive(Debug, Default)]
pub struct ClickTracker {
    last_item: Option<usize>,
    last_time: Option<Instant>,
}

#[derive(Debug, PartialEq)]
pub enum ClickKind {
    Single,
    Double,
}

impl ClickTracker {
    pub fn record(&mut self, item: usize) -> ClickKind {
        let now = Instant::now();
        let is_double = self.last_item == Some(item)
            && self.last_time
                .map(|t| now.duration_since(t).as_millis() < DOUBLE_CLICK_MS)
                .unwrap_or(false);

        self.last_item = Some(item);
        self.last_time = Some(now);

        if is_double {
            // Reset so triple-click is two singles
            self.last_item = None;
            ClickKind::Double
        } else {
            ClickKind::Single
        }
    }

    /// Follow the last-clicked item to its new index after a listing reload.
    pub fn remap(&mut self, f: impl Fn(usize) -> Option<usize>) {
        self.last_item = self.last_item.and_then(f);
    }

    pub fn reset(&mut self) {
        self.last_item = None;
        self.last_time = None;
    }
}

// ---------------------------------------------------------------------------
// Inline rename state
// ---------------------------------------------------------------------------

/// What a name prompt creates once confirmed.
#[derive(Debug, Clone)]
pub enum PromptKind {
    NewFolder,
    NewFile,
    /// Clipboard text or an image, saved as a file.
    Paste(Vec<u8>),
}

/// Dolphin's Create Folder / Create File / Paste Clipboard Contents
/// dialogs: nothing is created until it's confirmed.
#[derive(Debug, Clone)]
pub struct NamePrompt {
    pub kind: PromptKind,
    pub dest: std::path::PathBuf,
    pub input: String,
    pub cursor_pos: usize,
    /// Last commit was rejected (bad name or file exists); cleared on edit.
    pub invalid: bool,
    /// Shown greyed while the field is empty, and used if it's left empty.
    pub placeholder: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictButton {
    /// Overwrite, or Write Into for folders.
    Replace,
    Skip,
    Rename,
    Cancel,
    AllToggle,
}

/// Dolphin's "File Already Exists" dialog for a clash the file
/// operation is waiting on.
pub struct ConflictPrompt {
    pub conflict: crate::fileops::Conflict,
    /// The name used by Rename: a free one to start with.
    pub input: String,
    pub cursor_pos: usize,
    pub invalid: bool,
    /// Apply the choice to the job's later clashes too.
    pub all: bool,
    /// Size and date of the incoming and the existing item.
    pub src_info: String,
    pub dst_info: String,
}

impl ConflictPrompt {
    pub fn new(conflict: crate::fileops::Conflict) -> Self {
        let input = crate::fileops::free_name(&conflict.dst).file_name()
            .map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let info = |p: &std::path::Path| {
            let Ok(m) = std::fs::symlink_metadata(p) else { return String::new() };
            let what = if m.is_dir() {
                let n = std::fs::read_dir(p).map_or(0, |d| d.count());
                format!("folder, {n} item{}", if n == 1 { "" } else { "s" })
            } else {
                crate::ui::properties::human_size(m.len())
            };
            match m.modified() {
                Ok(t) => format!("{what}, modified {}", chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M")),
                Err(_) => what,
            }
        };
        let (src_info, dst_info) = (info(&conflict.src), info(&conflict.dst));
        let cursor_pos = input.len();
        Self { conflict, input, cursor_pos, invalid: false, all: false, src_info, dst_info }
    }
}

pub struct RenameState {
    pub item_idx: usize,
    pub input: String,
    pub cursor_pos: usize,
    /// Last commit was rejected (bad name or target exists); cleared on edit.
    pub invalid: bool,
}

impl RenameState {
    pub fn new(item_idx: usize, current_name: &str) -> Self {
        let len = current_name.len();
        Self {
            item_idx,
            input: current_name.to_string(),
            cursor_pos: len,
            invalid: false,
        }
    }
}
