//! Mime detection from the freedesktop shared-mime-info database (what
//! Dolphin and other desktop file managers use): globs2 for names, the
//! subclass/alias tables for disambiguation and icon names.

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
};

struct Glob {
    weight: u32,
    mime: String,
    /// Pattern length, the spec's tie-breaker after weight.
    len: usize,
}

#[derive(Default)]
struct Db {
    /// "*.ext" patterns keyed by lowercase extension (may contain dots).
    by_ext: HashMap<String, Vec<Glob>>,
    /// Literal file names, lowercase.
    by_name: HashMap<String, Vec<Glob>>,
    /// canonical -> aliases (legacy names icon themes still ship)
    aliases_of: HashMap<String, Vec<String>>,
    /// alias -> canonical
    canonical: HashMap<String, String>,
    parents: HashMap<String, Vec<String>>,
    generic_icons: HashMap<String, String>,
}

fn db() -> &'static Db {
    static DB: OnceLock<Db> = OnceLock::new();
    DB.get_or_init(load)
}

fn mime_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    dirs.into_iter().map(|d| d.join("mime")).collect()
}

fn load() -> Db {
    let mut db = Db::default();
    let read = |dir: &Path, file: &str| std::fs::read_to_string(dir.join(file)).unwrap_or_default();

    for dir in mime_dirs() {
        for line in read(&dir, "globs2").lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut parts = line.splitn(4, ':');
            let (Some(w), Some(mime), Some(pat)) = (parts.next(), parts.next(), parts.next()) else { continue; };
            let glob = |pat: &str| Glob { weight: w.parse().unwrap_or(50), mime: mime.to_string(), len: pat.len() };
            if let Some(ext) = pat.strip_prefix("*.")
                && !ext.contains(['*', '?', '['])
            {
                db.by_ext.entry(ext.to_lowercase()).or_default().push(glob(pat));
            } else if !pat.contains(['*', '?', '[']) {
                db.by_name.entry(pat.to_lowercase()).or_default().push(glob(pat));
            }
        }
        for line in read(&dir, "aliases").lines() {
            if let Some((alias, canon)) = line.split_once(' ') {
                db.canonical.entry(alias.into()).or_insert_with(|| canon.into());
                db.aliases_of.entry(canon.into()).or_default().push(alias.into());
            }
        }
        for line in read(&dir, "subclasses").lines() {
            if let Some((child, parent)) = line.split_once(' ') {
                db.parents.entry(child.into()).or_default().push(parent.into());
            }
        }
        for line in read(&dir, "generic-icons").lines() {
            if let Some((mime, icon)) = line.split_once(':') {
                db.generic_icons.entry(mime.into()).or_insert_with(|| icon.into());
            }
        }
    }
    db
}

fn canonical(mime: &str) -> &str {
    db().canonical.get(mime).map(String::as_str).unwrap_or(mime)
}

/// Whether `mime` is, or descends from, `ancestor`.
fn is_a(mime: &str, ancestor: &str) -> bool {
    let mut stack = vec![canonical(mime).to_string()];
    let mut seen = 0;
    while let Some(m) = stack.pop() {
        if m == ancestor {
            return true;
        }
        seen += 1;
        if seen > 32 { break; }
        if let Some(ps) = db().parents.get(&m) {
            stack.extend(ps.iter().cloned());
        }
    }
    false
}

/// The type, its aliases and every ancestor (most specific first), e.g.
/// application/x-shellscript -> text/plain. Used to find capable apps.
pub fn lineage(mime: &str) -> Vec<String> {
    let db = db();
    let mut out: Vec<String> = Vec::new();
    let mut queue = vec![canonical(mime).to_string()];
    while let Some(m) = (!queue.is_empty()).then(|| queue.remove(0)) {
        if out.contains(&m) || out.len() > 32 {
            continue;
        }
        out.push(m.clone());
        out.extend(db.aliases_of.get(&m).into_iter().flatten().cloned());
        queue.extend(db.parents.get(&m).into_iter().flatten().cloned());
        // Every text type is also plain text
        if m.starts_with("text/") && m != "text/plain" {
            queue.push("text/plain".into());
        }
    }
    out
}

fn is_text(mime: &str) -> bool {
    mime.starts_with("text/") || is_a(mime, "text/plain")
}

fn is_xml(mime: &str) -> bool {
    mime.ends_with("+xml") || is_a(mime, "application/xml")
}

/// Every glob matching the file name.
fn candidates(path: &Path) -> Vec<&'static Glob> {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else { return Vec::new(); };
    let db = db();
    let mut out: Vec<&Glob> = db.by_name.get(&name).into_iter().flatten().collect();
    // Every dotted suffix: "a.tar.gz" tries "tar.gz" then "gz"
    for (i, _) in name.match_indices('.') {
        if let Some(globs) = db.by_ext.get(&name[i + 1..]) {
            out.extend(globs);
        }
    }
    out
}

fn best<'a>(globs: impl Iterator<Item = &'a &'static Glob>) -> Option<&'static Glob> {
    globs.copied().max_by_key(|g| (g.weight, g.len))
}

/// Mime from the file name alone (no I/O). Cheap enough for the UI thread.
pub fn guess_by_name(path: &Path) -> Option<String> {
    best(candidates(path).iter()).map(|g| canonical(&g.mime).to_string())
}

/// Mime from the file name, peeking at the content only when the name is
/// ambiguous (e.g. .ts: TypeScript, MPEG transport stream or Qt
/// translation). None when no glob matches.
pub fn detect(path: &Path) -> Option<String> {
    let cands = candidates(path);
    let top = best(cands.iter())?;
    let ambiguous = cands.iter()
        .any(|g| g.weight == top.weight && canonical(&g.mime) != canonical(&top.mime));
    if !ambiguous {
        return Some(canonical(&top.mime).to_string());
    }

    let mut head = [0u8; 1024];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let head = &head[..n];
    let textual = !head.contains(&0);
    let looks_xml = textual && head.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<');

    let fits = |g: &&&Glob| {
        let m = canonical(&g.mime);
        is_text(m) == textual && (!textual || is_xml(m) == looks_xml)
    };
    let chosen = best(cands.iter().filter(fits)).unwrap_or(top);
    Some(canonical(&chosen.mime).to_string())
}

/// Icon theme names for a mime type, most specific first: the type itself,
/// its legacy aliases, then its declared generic icon.
pub fn icon_names(mime: &str) -> Vec<String> {
    let db = db();
    let mime = canonical(mime);
    let mut names = vec![mime.replace('/', "-")];
    for alias in db.aliases_of.get(mime).into_iter().flatten() {
        names.push(alias.replace('/', "-"));
    }
    // Newer unprefixed types (text/rust) often ship as the old x- name
    if let Some((major, minor)) = mime.split_once('/')
        && !minor.starts_with("x-")
    {
        names.push(format!("{major}-x-{minor}"));
    }
    // Walk parents for a declared generic icon (e.g. scripts -> text-x-script)
    let mut m = mime.to_string();
    for _ in 0..8 {
        if let Some(icon) = db.generic_icons.get(&m) {
            names.push(icon.clone());
            break;
        }
        match db.parents.get(&m).and_then(|p| p.first()) {
            Some(p) => m = p.clone(),
            None => break,
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_types() {
        let g = |n: &str| guess_by_name(Path::new(n));
        assert_eq!(g("main.rs").as_deref(), Some("text/rust"));
        assert_eq!(g("x.py").as_deref(), Some("text/x-python"));
        assert_eq!(g("a.tar.gz").as_deref(), Some("application/x-compressed-tar"));
        assert_eq!(g("README.md").as_deref(), Some("text/markdown"));
        assert!(icon_names("text/rust").contains(&"text-x-rust".to_string()));
    }

    #[test]
    fn ambiguous_ts_sniffs_content() {
        let dir = std::env::temp_dir().join("fbt-mime");
        std::fs::create_dir_all(&dir).unwrap();
        let code = dir.join("app.ts");
        std::fs::write(&code, "export const x: number = 1;\n").unwrap();
        let video = dir.join("clip.ts");
        std::fs::write(&video, [0x47u8, 0x40, 0x00, 0x10, 0x00, 0x00, 0xb0]).unwrap();
        assert_eq!(detect(&code).as_deref(), Some("application/typescript"));
        assert_eq!(detect(&video).as_deref(), Some("video/mp2t"));
    }
}
