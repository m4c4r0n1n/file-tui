//! Dolphin's "Show in Groups" for Details: rows grouped by the sort column
//! (first letter, date period, size band or type), each under a header row.

use chrono::{DateTime, Datelike, Local, NaiveDate};

use crate::app::SortField;
use crate::fs::DirEntry;

/// A Details line: a group header or an entry index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DRow {
    Group(String),
    Entry(usize),
}

/// Sort key and header label of the group an entry falls in.
pub fn group_of(e: &DirEntry, field: SortField) -> ((i64, String), String) {
    match field {
        SortField::Name => {
            let c = e.name.trim_start_matches('.').chars().next().unwrap_or(' ');
            if c.is_ascii_digit() {
                ((0, String::new()), "0 \u{2013} 9".into())
            } else if c.is_alphabetic() {
                let up: String = c.to_uppercase().collect();
                ((1, up.clone()), up)
            } else {
                ((2, String::new()), "Others".into())
            }
        }
        SortField::Size => {
            const MIB: u64 = 1024 * 1024;
            let (k, label) = match (e.is_dir, e.size.unwrap_or(0)) {
                (true, _) => (0, "Folders"),
                (_, s) if s < MIB => (1, "Small (under 1 MiB)"),
                (_, s) if s < 100 * MIB => (2, "Medium (1 \u{2013} 100 MiB)"),
                _ => (3, "Large (over 100 MiB)"),
            };
            ((k, String::new()), label.into())
        }
        SortField::Modified | SortField::Created | SortField::Accessed => {
            let t = match field {
                SortField::Created => e.created,
                SortField::Accessed => e.accessed,
                _ => e.modified,
            };
            let Some(t) = t else { return ((i64::MIN, String::new()), "Unknown".into()); };
            let d = DateTime::<Local>::from(t).date_naive();
            let (start, label) = date_group(d, Local::now().date_naive());
            ((start.num_days_from_ce() as i64, String::new()), label)
        }
        SortField::Type => {
            if e.is_dir {
                return ((0, String::new()), "Folders".into());
            }
            let mime = e.mime_type.clone()
                .or_else(|| crate::mime::guess_by_name(&e.path))
                .unwrap_or_else(|| "file".into());
            ((1, mime.clone()), mime)
        }
    }
}

/// Dolphin-style date periods; the date is where the period starts, so
/// periods order like the dates in them.
fn date_group(d: NaiveDate, today: NaiveDate) -> (NaiveDate, String) {
    let days = (today - d).num_days();
    let week_start = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let month_start = today.with_day(1).unwrap_or(today);
    if days <= 0 {
        (today, "Today".into())
    } else if days == 1 {
        (today - chrono::Duration::days(1), "Yesterday".into())
    } else if d >= week_start {
        (week_start, "Earlier this Week".into())
    } else if d >= week_start - chrono::Duration::days(7) {
        (week_start - chrono::Duration::days(7), "Last Week".into())
    } else if d >= month_start {
        (month_start, "Earlier this Month".into())
    } else {
        (d.with_day(1).unwrap_or(d), d.format("%B %Y").to_string())
    }
}

/// Reorder so each group is contiguous (in the sort direction), keeping the
/// existing order inside groups (folders first) and tree children with
/// their parent.
pub fn arrange(entries: Vec<DirEntry>, field: SortField, ascending: bool) -> Vec<DirEntry> {
    let mut blocks: Vec<Vec<DirEntry>> = Vec::new();
    for e in entries {
        match blocks.last_mut() {
            Some(b) if e.depth > 0 => b.push(e),
            _ => blocks.push(vec![e]),
        }
    }
    let key = |b: &Vec<DirEntry>| group_of(&b[0], field).0;
    if ascending {
        blocks.sort_by_cached_key(key);
    } else {
        blocks.sort_by_cached_key(|b| std::cmp::Reverse(key(b)));
    }
    blocks.into_iter().flatten().collect()
}

/// Details lines with a header before each group.
pub fn rows(entries: &[DirEntry], field: SortField) -> Vec<DRow> {
    let mut out = Vec::with_capacity(entries.len() + 16);
    let mut last: Option<String> = None;
    for (i, e) in entries.iter().enumerate() {
        if e.depth == 0 {
            let (_, label) = group_of(e, field);
            if last.as_ref() != Some(&label) {
                out.push(DRow::Group(label.clone()));
                last = Some(label);
            }
        }
        out.push(DRow::Entry(i));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, dir: bool) -> DirEntry {
        DirEntry {
            path: name.into(), name: name.into(), is_dir: dir, size: Some(10), modified: None,
            created: None, accessed: None, mime_type: None, is_symlink: false, depth: 0,
        }
    }

    #[test]
    fn letters_with_folders_first() {
        let es = vec![entry("Zebra", true), entry("apple", true), entry("2024.txt", false), entry("Ant.txt", false)];
        let es = arrange(es, SortField::Name, true);
        let names: Vec<&str> = es.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["2024.txt", "apple", "Ant.txt", "Zebra"]);
        let labels: Vec<String> = rows(&es, SortField::Name).into_iter()
            .filter_map(|r| if let DRow::Group(l) = r { Some(l) } else { None }).collect();
        assert_eq!(labels, ["0 \u{2013} 9", "A", "Z"]);
    }

    #[test]
    fn date_periods() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(); // a Wednesday
        let g = |y, m, d| date_group(NaiveDate::from_ymd_opt(y, m, d).unwrap(), today).1;
        assert_eq!(g(2026, 11, 4), "Today");
        assert_eq!(g(2026, 11, 3), "Yesterday");
        assert_eq!(g(2026, 11, 2), "Earlier this Week");
        assert_eq!(g(2026, 10, 28), "Last Week");
        assert_eq!(g(2026, 10, 20), "October 2026");
    }
}
