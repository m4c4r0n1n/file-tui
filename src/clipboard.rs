//! The system clipboard through the desktop's command-line tools:
//! wl-copy/wl-paste on Wayland, xclip on X11.

use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Wayland,
    X11,
}

pub fn backend() -> Option<Backend> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && crate::check::has_command("wl-paste") {
        Some(Backend::Wayland)
    } else if std::env::var_os("DISPLAY").is_some() && crate::check::has_command("xclip") {
        Some(Backend::X11)
    } else {
        None
    }
}

fn read_cmd(args: &[&str]) -> Option<Vec<u8>> {
    let (bin, base): (&str, &[&str]) = match backend()? {
        Backend::Wayland => ("wl-paste", &[]),
        Backend::X11 => ("xclip", &["-selection", "clipboard", "-o"]),
    };
    // A stuck clipboard owner mustn't hang the UI
    let out = Command::new("timeout")
        .arg("2").arg(bin).args(base).args(args)
        .stdin(Stdio::null()).stderr(Stdio::null())
        .output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// The clipboard's MIME types (empty when there's nothing, or no tool).
pub fn types() -> Vec<String> {
    let args: &[&str] = match backend() {
        Some(Backend::Wayland) => &["--list-types"],
        Some(Backend::X11) => &["-t", "TARGETS"],
        None => return Vec::new(),
    };
    read_cmd(args)
        .map(|out| String::from_utf8_lossy(&out).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The clipboard's content as `mime`, or as plain text with None.
pub fn read(mime: Option<&str>) -> Option<Vec<u8>> {
    match (backend()?, mime) {
        (Backend::Wayland, Some(m)) => read_cmd(&["-t", m]),
        (Backend::Wayland, None) => read_cmd(&["--no-newline"]),
        (Backend::X11, Some(m)) => read_cmd(&["-t", m]),
        (Backend::X11, None) => read_cmd(&["-t", "UTF8_STRING"]),
    }
}

/// Put `data` on the clipboard as `mime` (plain text with None).
pub fn write(mime: Option<&str>, data: &[u8]) {
    let mut cmd = match backend() {
        Some(Backend::Wayland) => {
            let mut c = Command::new("wl-copy");
            if let Some(m) = mime {
                c.args(["-t", m]);
            }
            c
        }
        Some(Backend::X11) => {
            let mut c = Command::new("xclip");
            c.args(["-selection", "clipboard", "-i"]);
            c.args(["-t", mime.unwrap_or("UTF8_STRING")]);
            c
        }
        None => return,
    };
    // wl-copy/xclip stay running to serve the clipboard; in a session of
    // their own, the copy survives this window closing
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Ok(mut child) = crate::launch::spawn_own_session(&mut cmd)
        && let Some(mut stdin) = child.stdin.take()
    {
        let _ = stdin.write_all(data);
        drop(stdin);
        std::thread::spawn(move || { let _ = child.wait(); });
    }
}
