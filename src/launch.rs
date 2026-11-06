//! Starting other programs so they outlive this window: each runs in a new
//! session with no controlling terminal, so the hangup sent when the
//! terminal closes never reaches it.

use std::{
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
};

/// Run `cmd` in a session of its own; stdio is whatever the caller set.
pub fn spawn_own_session(cmd: &mut Command) -> std::io::Result<Child> {
    // SAFETY: setsid is async-signal-safe, the only kind of call allowed
    // between fork and exec
    unsafe {
        cmd.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(std::io::Error::from));
    }
    cmd.spawn()
}

/// Start `cmd` detached (no stdio, own session) and reap it when it exits.
pub fn detach(cmd: &mut Command) {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match spawn_own_session(cmd) {
        Ok(mut child) => {
            std::thread::spawn(move || { let _ = child.wait(); });
        }
        Err(e) => tracing::warn!("launch {:?}: {e}", cmd.get_program()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn detached_programs_lead_their_own_session_without_a_terminal() {
        let out = std::env::temp_dir().join(format!("fbt-launch-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        super::detach(std::process::Command::new("sh").arg("-c")
            .arg("echo $$ $(ps -o sid= -p $$) $(ps -o tty= -p $$) > \"$0.tmp\" && mv \"$0.tmp\" \"$0\"").arg(&out));
        let mut text = String::new();
        for _ in 0..200 {
            if let Ok(t) = std::fs::read_to_string(&out) { text = t; break; }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(&out);
        let f: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(f.len(), 3, "{text:?}");
        assert_eq!(f[0], f[1], "session id should be its own pid");
        assert_eq!(f[2], "?", "no controlling terminal");
    }
}
