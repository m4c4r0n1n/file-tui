pub mod mounts;
pub mod watcher;

use std::{
    path::PathBuf,
    time::SystemTime,
};

use crate::app::{SortField, SortState};

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub mime_type: Option<String>,
    pub is_symlink: bool,
    /// Nesting level in the Details tree (0 = the folder being viewed).
    pub depth: u16,
}

#[derive(Debug, Clone, Default)]
pub struct DirListing {
    pub entries: Vec<DirEntry>,
}

pub fn read_dir(path: &PathBuf, show_hidden: bool, folders_first: bool, sort: &SortState) -> anyhow::Result<DirListing> {
    let mut entries = Vec::new();

    for result in std::fs::read_dir(path)? {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        // Follow symlinks (a link to a folder is a folder), but remember them
        let is_symlink = entry.file_type().is_ok_and(|t| t.is_symlink());
        let meta = if is_symlink { std::fs::metadata(entry.path()).ok() } else { entry.metadata().ok() };
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = meta.as_ref().and_then(|m| if m.is_file() { Some(m.len()) } else { None });
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        let created = meta.as_ref().and_then(|m| m.created().ok());
        let accessed = meta.as_ref().and_then(|m| m.accessed().ok());

        entries.push(DirEntry {
            path: entry.path(),
            name,
            is_dir,
            size,
            modified,
            created,
            accessed,
            mime_type: None,
            is_symlink,
            depth: 0,
        });
    }

    sort_entries(&mut entries, folders_first, sort);
    Ok(DirListing { entries })
}

pub fn sort_entries(entries: &mut [DirEntry], folders_first: bool, sort: &SortState) {
    entries.sort_by(|a, b| {
        if folders_first {
            match (a.is_dir, b.is_dir) {
                (true, false) => return std::cmp::Ordering::Less,
                (false, true) => return std::cmp::Ordering::Greater,
                _ => {}
            }
        }
        let ord = match sort.field {
            SortField::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortField::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
            SortField::Modified => a.modified.cmp(&b.modified),
            SortField::Created => a.created.cmp(&b.created),
            SortField::Accessed => a.accessed.cmp(&b.accessed),
            SortField::Type => {
                let ext_a = a.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                let ext_b = b.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                ext_a.cmp(&ext_b)
            }
        };
        if sort.ascending { ord } else { ord.reverse() }
    });
}
