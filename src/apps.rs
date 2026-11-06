//! "Open With": applications that handle a mime type, from the installed
//! .desktop files and mimeapps.list (freedesktop specs, as Dolphin uses).

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct DesktopApp {
    /// Desktop file id, e.g. "org.kde.okular.desktop".
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub terminal: bool,
    pub path: PathBuf,
    mimes: Vec<String>,
}

fn data_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    dirs
}

/// All launchable applications by desktop id (earlier data dirs win).
/// Rescanned at most every 30s, so menus open quickly but new installs show.
type AppIndex = Arc<HashMap<String, DesktopApp>>;

fn installed() -> AppIndex {
    static CACHE: Mutex<Option<(Instant, AppIndex)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, apps)) = cache.as_ref()
        && at.elapsed() < Duration::from_secs(30)
    {
        return apps.clone();
    }
    let mut apps = HashMap::new();
    for dir in data_dirs() {
        scan(&dir.join("applications"), "", &mut apps);
    }
    let apps = Arc::new(apps);
    *cache = Some((Instant::now(), apps.clone()));
    apps
}

fn scan(dir: &Path, prefix: &str, apps: &mut HashMap<String, DesktopApp>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return; };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            // Subdirectories become dash-separated id prefixes
            scan(&path, &format!("{prefix}{name}-"), apps);
        } else if name.ends_with(".desktop") {
            let id = format!("{prefix}{name}");
            if apps.contains_key(&id) {
                continue;
            }
            if let Some(app) = parse(&path, id.clone()) {
                apps.insert(id, app);
            }
        }
    }
}

fn parse(path: &Path, id: String) -> Option<DesktopApp> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((k, v)) = line.split_once('=') {
            fields.entry(k.trim()).or_insert(v.trim());
        }
    }
    let truthy = |k: &str| fields.get(k).is_some_and(|v| *v == "true");
    if fields.get("Type") != Some(&"Application") || truthy("Hidden") {
        return None;
    }
    Some(DesktopApp {
        id,
        name: fields.get("Name")?.to_string(),
        exec: fields.get("Exec")?.to_string(),
        icon: fields.get("Icon").map(|s| s.to_string()).filter(|s| !s.is_empty()),
        terminal: truthy("Terminal"),
        path: path.to_path_buf(),
        mimes: fields.get("MimeType").map(|m| m.split(';').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default(),
    })
}

/// mimeapps.list files, most important first.
fn mimeapps_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default()
        .split(':').filter(|s| !s.is_empty()).map(str::to_lowercase).collect();
    let config_dirs: Vec<PathBuf> = dirs::config_dir().into_iter()
        .chain(std::env::var("XDG_CONFIG_DIRS").unwrap_or_else(|_| "/etc/xdg".into())
            .split(':').filter(|s| !s.is_empty()).map(PathBuf::from))
        .collect();
    let app_dirs: Vec<PathBuf> = data_dirs().into_iter().map(|d| d.join("applications")).collect();
    for dir in config_dirs.iter().chain(&app_dirs) {
        for d in &desktops {
            out.push(dir.join(format!("{d}-mimeapps.list")));
        }
        out.push(dir.join("mimeapps.list"));
    }
    out
}

#[derive(Default)]
struct Associations {
    defaults: Vec<String>,
    added: Vec<String>,
    removed: HashSet<String>,
}

fn associations(mimes: &[String]) -> Associations {
    let mut a = Associations::default();
    for file in mimeapps_files() {
        let Ok(text) = std::fs::read_to_string(&file) else { continue; };
        let mut section = "";
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line;
                continue;
            }
            let Some((mime, ids)) = line.split_once('=') else { continue; };
            if !mimes.iter().any(|m| m == mime.trim()) {
                continue;
            }
            let ids = ids.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from);
            match section {
                "[Default Applications]" => a.defaults.extend(ids),
                "[Added Associations]" => a.added.extend(ids),
                "[Removed Associations]" => a.removed.extend(ids),
                _ => {}
            }
        }
    }
    a
}

/// Applications for `mime`: (default, others sorted by name). A type's
/// parent types count too, so e.g. any text editor offers to open source code.
pub fn apps_for(mime: &str) -> (Option<DesktopApp>, Vec<DesktopApp>) {
    let lineage = crate::mime::lineage(mime);
    let installed = installed();
    let assoc = associations(&lineage);

    let usable = |id: &String| !assoc.removed.contains(id);
    let default = assoc.defaults.iter().chain(&assoc.added)
        .filter(|id| usable(id))
        .find_map(|id| installed.get(id).cloned());

    let mut seen: HashSet<String> = default.iter().map(|d| d.id.clone()).collect();
    let mut others: Vec<DesktopApp> = Vec::new();
    for id in assoc.added.iter().filter(|id| usable(id)) {
        if let Some(app) = installed.get(id)
            && seen.insert(id.clone())
        {
            others.push(app.clone());
        }
    }
    let mut by_type: Vec<&DesktopApp> = installed.values()
        .filter(|a| usable(&a.id) && !seen.contains(&a.id))
        .filter(|a| a.mimes.iter().any(|m| lineage.contains(m)))
        .collect();
    by_type.sort_by_key(|a| a.name.to_lowercase());
    others.extend(by_type.into_iter().cloned());

    // No explicit default: the first capable app, like xdg-open would pick
    match default {
        Some(d) => (Some(d), others),
        None if !others.is_empty() => {
            let first = others.remove(0);
            (Some(first), others)
        }
        None => (None, others),
    }
}

/// Split an Exec value into arguments (desktop entry quoting rules).
fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => { in_quotes = !in_quotes; has = true; }
            '\\' if in_quotes => { if let Some(n) = chars.next() { cur.push(n); } }
            ' ' | '\t' if !in_quotes => {
                if has || !cur.is_empty() { args.push(std::mem::take(&mut cur)); }
                has = false;
            }
            _ => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        args.push(cur);
    }
    args
}

/// Command lines for opening `paths` with `app`: one launch for %F/%U (or
/// no field code), one per file for %f/%u.
pub fn command_lines(app: &DesktopApp, paths: &[PathBuf]) -> Vec<Vec<String>> {
    let args = split_exec(&app.exec);
    let multi = args.iter().any(|a| a.contains("%F") || a.contains("%U"));
    let single = args.iter().any(|a| a.contains("%f") || a.contains("%u"));
    let as_str = |p: &PathBuf| p.to_string_lossy().into_owned();

    let expand = |files: &[PathBuf]| -> Vec<String> {
        let mut out = Vec::new();
        for arg in &args {
            match arg.as_str() {
                "%F" | "%U" => out.extend(files.iter().map(as_str)),
                "%f" | "%u" => out.extend(files.first().map(as_str)),
                "%i" => if let Some(icon) = &app.icon { out.push("--icon".into()); out.push(icon.clone()); },
                "%c" => out.push(app.name.clone()),
                "%k" => out.push(app.path.to_string_lossy().into_owned()),
                // Deprecated or unknown codes are dropped
                a if a.len() == 2 && a.starts_with('%') && a != "%%" => {}
                a => {
                    let file = files.first().map(as_str).unwrap_or_default();
                    out.push(a.replace("%f", &file).replace("%u", &file).replace("%c", &app.name).replace("%%", "%"));
                }
            }
        }
        if !multi && !single {
            out.extend(files.iter().map(as_str));
        }
        out
    };

    if single && !multi {
        paths.iter().map(|p| expand(std::slice::from_ref(p))).collect()
    } else {
        vec![expand(paths)]
    }
}

pub fn launch(app: &DesktopApp, paths: &[PathBuf], terminal: &str) {
    for argv in command_lines(app, paths) {
        let Some((prog, rest)) = argv.split_first() else { continue; };
        let mut cmd = if app.terminal {
            let mut c = Command::new(terminal);
            c.arg("-e").arg(prog).args(rest);
            c
        } else {
            let mut c = Command::new(prog);
            c.args(rest);
            c
        };
        if let Some(dir) = paths.first().and_then(|p| p.parent()) {
            cmd.current_dir(dir);
        }
        crate::launch::detach(&mut cmd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_expansion() {
        let app = DesktopApp {
            id: "x.desktop".into(), name: "X".into(), exec: r#"okular "--title=%c" %U --x %i"#.into(),
            icon: Some("okular".into()), terminal: false, path: "/a/x.desktop".into(), mimes: vec![],
        };
        let lines = command_lines(&app, &["/a b.pdf".into(), "/c.pdf".into()]);
        assert_eq!(lines, vec![vec!["okular", "--title=X", "/a b.pdf", "/c.pdf", "--x", "--icon", "okular"]]);
        let one = DesktopApp { exec: "gimp %f".into(), ..app };
        assert_eq!(command_lines(&one, &["/a".into(), "/b".into()]), vec![vec!["gimp", "/a"], vec!["gimp", "/b"]]);
    }
}

