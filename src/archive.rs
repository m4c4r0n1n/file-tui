//! Unpacking archives, like Dolphin's "Extract Here, Autodetect Subfolder":
//! into a hidden folder first, then a single top-level item is moved out
//! beside the archive, or the folder takes the archive's name.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Name endings of archives we can unpack, longest first.
const SUFFIXES: &[&str] = &[
    ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz4", ".tar.lzma", ".tar.Z",
    ".tgz", ".tbz", ".tbz2", ".txz", ".tzst", ".tar",
    ".zip", ".7z", ".rar", ".iso", ".cpio", ".xar", ".jar", ".cbz", ".cbr", ".cb7",
    ".deb", ".rpm", ".apk",
];

fn suffix_of(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    SUFFIXES.iter().copied().find(|s| lower.ends_with(&s.to_lowercase()) && lower.len() > s.len())
}

pub fn is_archive(path: &Path) -> bool {
    path.is_file() && path.file_name().is_some_and(|n| suffix_of(&n.to_string_lossy()).is_some())
}

/// The archive's name without its archive ending ("photos.tar.gz" -> "photos").
pub fn stem(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match suffix_of(&name) {
        Some(s) => name[..name.len() - s.len()].to_string(),
        None => name,
    }
}

/// `dir/base`, or `dir/base (2)` and so on if that's taken.
pub fn free_name(dir: &Path, base: &str) -> PathBuf {
    let candidate = dir.join(base);
    if !candidate.exists() {
        return candidate;
    }
    (2..).map(|n| dir.join(format!("{base} ({n})"))).find(|c| !c.exists()).unwrap_or(candidate)
}

/// Unpack `archive` into `dest` (which must exist), with bsdtar, else the
/// format's own tool.
fn unpack(archive: &Path, dest: &Path) -> Result<(), String> {
    let lower = archive.to_string_lossy().to_lowercase();
    let mut tries: Vec<Command> = Vec::new();
    let mut bsdtar = Command::new("bsdtar");
    bsdtar.arg("-xf").arg(archive).arg("-C").arg(dest);
    tries.push(bsdtar);
    if lower.ends_with(".7z") || lower.ends_with(".cb7") || lower.ends_with(".rar") || lower.ends_with(".cbr") {
        let mut c = Command::new("7z");
        c.arg("x").arg("-y").arg(format!("-o{}", dest.display())).arg(archive);
        tries.push(c);
    }
    if lower.ends_with(".rar") || lower.ends_with(".cbr") {
        let mut c = Command::new("unrar");
        c.arg("x").arg("-o-").arg(archive).arg(format!("{}/", dest.display()));
        tries.push(c);
    }
    if lower.ends_with(".zip") || lower.ends_with(".jar") || lower.ends_with(".cbz") {
        let mut c = Command::new("unzip");
        c.arg("-q").arg(archive).arg("-d").arg(dest);
        tries.push(c);
    }
    if lower.contains(".tar") || lower.ends_with(".tgz") || lower.ends_with(".txz") || lower.ends_with(".tbz2") {
        let mut c = Command::new("tar");
        c.arg("-xf").arg(archive).arg("-C").arg(dest);
        tries.push(c);
    }
    let mut complaint = String::from("no tool to unpack it (install libarchive)");
    for mut cmd in tries {
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let Ok(child) = crate::launch::spawn_own_session(&mut cmd) else { continue; };
        let Ok(out) = child.wait_with_output() else { continue; };
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        complaint = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("failed").to_string();
        // Start the next tool from a clean folder
        let _ = std::fs::remove_dir_all(dest);
        let _ = std::fs::create_dir(dest);
    }
    Err(complaint)
}

/// Extract next to the archive; returns what was made. With `into`, into a
/// new folder of that name instead of autodetecting.
pub fn extract(archive: &Path, into: Option<&str>) -> Result<PathBuf, String> {
    let parent = archive.parent().ok_or("no parent folder")?;
    let stem = stem(archive);
    let staging = free_name(parent, &format!(".{stem}.extracting"));
    std::fs::create_dir(&staging).map_err(|e| e.to_string())?;
    if let Err(e) = unpack(archive, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let finish = |result: std::io::Result<PathBuf>| {
        result.map_err(|e| { let _ = std::fs::remove_dir_all(&staging); e.to_string() })
    };
    if let Some(name) = into {
        let dest = free_name(parent, name);
        return finish(std::fs::rename(&staging, &dest).map(|_| dest));
    }
    let items: Vec<PathBuf> = std::fs::read_dir(&staging).map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    if let [only] = items.as_slice() {
        let name = only.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let dest = free_name(parent, &name);
        let moved = std::fs::rename(only, &dest).map(|_| dest);
        if moved.is_ok() {
            let _ = std::fs::remove_dir(&staging);
        }
        return finish(moved);
    }
    let dest = free_name(parent, &stem);
    finish(std::fs::rename(&staging, &dest).map(|_| dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fbt-archive-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stems_and_detection() {
        assert_eq!(stem(Path::new("/x/photos.tar.gz")), "photos");
        assert_eq!(stem(Path::new("/x/A.ZIP")), "A");
        assert_eq!(suffix_of(".zip"), None);
        assert_eq!(suffix_of("notes.txt"), None);
    }

    #[test]
    fn single_folder_moves_up_and_loose_files_get_a_folder() {
        let d = scratch("x");
        std::fs::create_dir_all(d.join("src/proj")).unwrap();
        std::fs::write(d.join("src/proj/a.txt"), "a").unwrap();
        std::fs::write(d.join("src/b.txt"), "b").unwrap();
        let run = |args: &[&str]| assert!(Command::new("tar").current_dir(d.join("src")).args(args).status().unwrap().success());
        run(&["-czf", "../one.tar.gz", "proj"]);
        run(&["-czf", "../loose.tar.gz", "proj", "b.txt"]);

        let made = extract(&d.join("one.tar.gz"), None).unwrap();
        assert_eq!(made, d.join("proj"));
        assert_eq!(std::fs::read_to_string(d.join("proj/a.txt")).unwrap(), "a");

        let made = extract(&d.join("loose.tar.gz"), None).unwrap();
        assert_eq!(made, d.join("loose"));
        assert!(d.join("loose/b.txt").exists() && d.join("loose/proj/a.txt").exists());

        // Taken name: a numbered one beside it
        let made = extract(&d.join("one.tar.gz"), None).unwrap();
        assert_eq!(made, d.join("proj (2)"));

        let made = extract(&d.join("one.tar.gz"), Some("chosen")).unwrap();
        assert!(d.join("chosen/proj/a.txt").exists(), "{made:?}");

        std::fs::write(d.join("broken.zip"), "not a zip").unwrap();
        assert!(extract(&d.join("broken.zip"), None).is_err());
        let leftovers: Vec<_> = std::fs::read_dir(&d).unwrap().filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with('.')).collect();
        assert!(leftovers.is_empty(), "staging folders left behind");
        let _ = std::fs::remove_dir_all(&d);
    }
}
