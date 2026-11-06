use std::{
    os::unix::io::AsRawFd,
    sync::mpsc,
    thread,
    fs::File,
    io::{BufRead, BufReader},
};

use nix::poll::{PollFd, PollFlags, poll};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountKind {
    /// The root filesystem: always there, never unmounted from here.
    Root,
    /// Removable media mounted by udisks under /run/media.
    Media,
    /// A user FUSE mount (sshfs, rclone, gocryptfs, ...).
    Fuse,
}

#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub name: String,
    pub path: String,
    /// Fraction of the filesystem in use (0..=1).
    pub usage: Option<f32>,
    pub kind: MountKind,
    /// What's mounted (e.g. /dev/sdb1), from mountinfo.
    pub source: String,
}

impl DeviceEntry {
    pub fn new(name: String, path: String) -> Self {
        Self::with_kind(name, path, MountKind::Root, String::new())
    }

    fn with_kind(name: String, path: String, kind: MountKind, source: String) -> Self {
        let usage = nix::sys::statvfs::statvfs(path.as_str()).ok().and_then(|st| {
            let total = st.blocks() as f64;
            (total > 0.0).then(|| (1.0 - st.blocks_available() as f64 / total) as f32)
        });
        Self { name, path, usage, kind, source }
    }

    pub fn can_unmount(&self) -> bool {
        self.kind != MountKind::Root
    }

    /// Theme icon for the sidebar.
    pub fn icon(&self) -> &'static str {
        match self.kind {
            MountKind::Root => "drive-harddisk-root",
            MountKind::Media => "drive-removable-media",
            MountKind::Fuse => "folder-remote",
        }
    }
}

/// Unmount `dev` (powering the drive off too with `power_off`, Dolphin's
/// Safely Remove). Blocks; returns the tool's complaint on failure.
pub fn unmount(dev: &DeviceEntry, power_off: bool) -> Result<(), String> {
    use std::process::{Command, Stdio};
    // Our own working directory inside it would keep it busy
    if std::env::current_dir().is_ok_and(|d| d.starts_with(&dev.path)) {
        let _ = std::env::set_current_dir(dirs::home_dir().unwrap_or_else(|| "/".into()));
    }
    let run = |cmd: &str, args: &[&str]| -> Result<(), String> {
        let out = Command::new(cmd).args(args).stdin(Stdio::null()).output()
            .map_err(|e| format!("{cmd}: {e}"))?;
        if out.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("{cmd} failed") } else { err })
    };
    match dev.kind {
        MountKind::Root => Err("the root filesystem can't be unmounted".into()),
        MountKind::Fuse => run("fusermount3", &["-u", &dev.path])
            .or_else(|_| run("fusermount", &["-u", &dev.path]))
            .or_else(|e| run("umount", &[&dev.path]).map_err(|_| e)),
        MountKind::Media if dev.source.starts_with("/dev/") => {
            run("udisksctl", &["unmount", "--no-user-interaction", "-b", &dev.source])
                .or_else(|e| run("umount", &[&dev.path]).map_err(|_| e))?;
            if power_off {
                run("udisksctl", &["power-off", "--no-user-interaction", "-b", &dev.source])?;
            }
            Ok(())
        }
        MountKind::Media => run("umount", &[&dev.path]),
    }
}

#[derive(Debug)]
pub enum MountEvent {
    Updated(Vec<DeviceEntry>),
}

pub fn spawn_mount_watcher(tx: mpsc::Sender<MountEvent>) {
    thread::spawn(move || {
        let file = match File::open("/proc/self/mountinfo") {
            Ok(f) => f,
            Err(e) => { tracing::error!("open mountinfo: {e}"); return; }
        };

        loop {
            let mut pfd = PollFd::new(
                unsafe { std::os::unix::io::BorrowedFd::borrow_raw(file.as_raw_fd()) },
                PollFlags::POLLPRI | PollFlags::POLLERR,
            );
            // 5 second fallback timeout
            let _ = poll(std::slice::from_mut(&mut pfd), 5000u16);

            if let Ok(devices) = parse_mountinfo() {
                let _ = tx.send(MountEvent::Updated(devices));
            }
        }
    });
}

/// FUSE pseudo-filesystems that are infrastructure, not user mounts.
const FUSE_BLOCKLIST: &[&str] = &["fuse.gvfsd-fuse", "fuse.portal"];

/// Filesystems we consider "real" for removable media under /run/media.
const MEDIA_FSTYPES: &[&str] = &[
    "ext4", "ext3", "ext2", "btrfs", "xfs", "f2fs",
    "vfat", "exfat", "ntfs", "ntfs3", "udf", "iso9660",
];

pub fn parse_mountinfo() -> anyhow::Result<Vec<DeviceEntry>> {
    let file = File::open("/proc/self/mountinfo")?;
    let reader = BufReader::new(file);
    let mut devices = Vec::new();

    // Always include root
    devices.push(DeviceEntry::new("Root".into(), "/".into()));

    for line in reader.lines() {
        let line = line?;
        let fields: Vec<&str> = line.split_whitespace().collect();
        // mountinfo format: id parent_id major:minor root mountpoint mountopts [optfields] - fstype source superopts
        if fields.len() < 5 {
            continue;
        }
        let mountpoint = unescape_mountpoint(fields[4]);

        // Find separator "-" to get fstype
        let dash_pos = fields.iter().position(|&f| f == "-");
        let fstype = dash_pos.and_then(|p| fields.get(p + 1)).copied().unwrap_or("");
        let source = dash_pos.and_then(|p| fields.get(p + 2)).map(|s| unescape_mountpoint(s)).unwrap_or_default();

        if mountpoint == "/" {
            continue;
        }

        // Real user FUSE mounts (sshfs, rclone, ...) — but not gvfs/portal
        // infrastructure (nothing under /run/user), and not fusectl.
        let is_fuse = (fstype == "fuse" || fstype == "fuseblk" || fstype.starts_with("fuse."))
            && !FUSE_BLOCKLIST.contains(&fstype)
            && !mountpoint.starts_with("/run/user/");

        // Removable media mounted by udisks
        let is_media = mountpoint.starts_with("/run/media/")
            && MEDIA_FSTYPES.contains(&fstype);

        if !is_fuse && !is_media {
            continue;
        }

        let name = mountpoint
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&mountpoint)
            .to_string();

        let kind = if is_media { MountKind::Media } else { MountKind::Fuse };
        devices.push(DeviceEntry::with_kind(name, mountpoint, kind, source));
    }

    Ok(devices)
}

/// mountinfo escapes space, tab, newline and backslash as octal (\040 etc).
fn unescape_mountpoint(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && let (Some(&a), Some(&b), Some(&c)) = (bytes.get(i + 1), bytes.get(i + 2), bytes.get(i + 3))
                && a.is_ascii_digit() && b.is_ascii_digit() && c.is_ascii_digit() {
                    let val = (a - b'0') * 64 + (b - b'0') * 8 + (c - b'0');
                    out.push(val as char);
                    i += 4;
                    continue;
                }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}
