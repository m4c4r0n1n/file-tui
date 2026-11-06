//! File operations (copy, move, trash, delete) on a background worker, with
//! progress for the status bar and a report of what failed. Done here
//! rather than by `gio`, so they work without GLib and can show progress,
//! errors and Dolphin's "File Already Exists" choices for name clashes.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Copy,
    Move,
    Trash,
    Delete,
    EmptyTrash,
}

impl Op {
    fn verb(self) -> (&'static str, &'static str) {
        match self {
            Op::Copy => ("Copying", "Copied"),
            Op::Move => ("Moving", "Moved"),
            Op::Trash => ("Moving to trash", "Moved to trash"),
            Op::Delete => ("Deleting", "Deleted"),
            Op::EmptyTrash => ("Emptying trash", "Emptied trash"),
        }
    }
}

struct Job {
    op: Op,
    sources: Vec<PathBuf>,
    dest: Option<PathBuf>,
}

/// A running job's progress, as last reported.
#[derive(Debug, Clone)]
pub struct Progress {
    pub op: Op,
    /// Still counting what there is to do.
    pub scanning: bool,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_items: usize,
    pub done_items: usize,
    /// Name of the file being worked on.
    pub current: String,
    pub started: Instant,
}

impl Progress {
    pub fn verb(&self) -> &'static str {
        self.op.verb().0
    }

    pub fn fraction(&self) -> f64 {
        if self.total_bytes > 0 {
            self.done_bytes as f64 / self.total_bytes as f64
        } else if self.total_items > 0 {
            self.done_items as f64 / self.total_items as f64
        } else {
            0.0
        }
    }

    /// Estimated time left, once there's enough to go on.
    pub fn remaining(&self) -> Option<Duration> {
        let f = self.fraction();
        let el = self.started.elapsed().as_secs_f64();
        (f > 0.02 && el > 1.0).then(|| Duration::from_secs_f64(el / f - el))
    }
}

/// A copy or move whose target name is taken, for the user to decide.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub op: Op,
    pub src: PathBuf,
    pub dst: PathBuf,
    /// Both folders: the choice is writing into the existing one.
    pub both_dirs: bool,
    /// Both files (or links): the existing one can be replaced.
    pub can_replace: bool,
    /// Other items may clash too, so "apply to all" means something.
    pub more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Replace the existing file, or write into the existing folder.
    Replace,
    Skip,
    /// Use this name instead (in the same folder).
    Rename(String),
    /// A free "name (2)" name.
    AutoRename,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub choice: Choice,
    /// Do the same for the rest of the job's clashes of this kind.
    pub all: bool,
}

/// Where a source goes, its clash (if any) settled.
enum Place {
    New(PathBuf),
    Replace(PathBuf),
    Merge(PathBuf),
    Skip,
}

/// One thing a job (or a rename, or New Folder) did, as undo needs it.
/// Overwrites, merges and permanent deletes aren't recorded: nothing
/// could bring back what they replaced.
#[derive(Debug, Clone, PartialEq)]
pub enum Undo {
    /// Made by a copy or New Folder/File: undoing puts it in the trash.
    Created(PathBuf),
    /// Moved or renamed from `from` to `to`.
    Moved { from: PathBuf, to: PathBuf },
    /// Put in the trash, as `trashed`.
    Trashed { from: PathBuf, trashed: PathBuf },
}

impl Undo {
    /// Reverse it. A move back across filesystems is left to the caller
    /// (`Err` of kind CrossesDevices), as it needs the copy machinery.
    pub fn revert(&self) -> io::Result<()> {
        match self {
            Undo::Created(p) => trash(p).map(|_| ()),
            Undo::Moved { from, to } => {
                if fs::symlink_metadata(from).is_ok() {
                    return Err(io::Error::other(format!("{} is in the way", from.display())));
                }
                if let Some(parent) = from.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(to, from)
            }
            Undo::Trashed { from, trashed } => {
                let dest = if fs::symlink_metadata(from).is_ok() { free_name(from) } else { from.clone() };
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::rename(trashed, &dest)?;
                if let (Some(files), Some(name)) = (trashed.parent(), trashed.file_name()) {
                    let info = files.with_file_name("info").join(format!("{}.trashinfo", name.to_string_lossy()));
                    let _ = fs::remove_file(info);
                }
                Ok(())
            }
        }
    }
}

/// How a finished job went, for the status bar.
#[derive(Debug, Clone)]
pub struct Report {
    pub op: Op,
    pub done: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
    pub cancelled: bool,
    /// What it did, in order, for undo.
    pub undo: Vec<Undo>,
}

impl Report {
    pub fn message(&self) -> String {
        let past = self.op.verb().1;
        let items = |n: usize| if n == 1 { "1 item".to_string() } else { format!("{n} items") };
        if self.cancelled {
            return format!("Cancelled ({} done)", items(self.done));
        }
        let done = match self.skipped {
            0 => items(self.done),
            n => format!("{}, skipped {n}", items(self.done)),
        };
        match self.errors.as_slice() {
            [] if self.op == Op::EmptyTrash => past.to_string(),
            [] => format!("{past} {done}"),
            [one] => format!("{past} {done}; failed: {one}"),
            [first, rest @ ..] => format!("{past} {done}; {} failed: {first} (+{} more)", rest.len() + 1, rest.len()),
        }
    }
}

enum Msg {
    Progress(Progress),
    Conflict(Conflict),
    Done(Report),
}

/// Settles a clash: the app's dialog, or a fixed answer in tests.
type Ask<'a> = &'a dyn Fn(&Conflict) -> Resolution;

pub struct FileOps {
    jobs: mpsc::Sender<Job>,
    rx: mpsc::Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    answers: mpsc::Sender<Resolution>,
    /// Queued or running jobs.
    pending: usize,
    pub progress: Option<Progress>,
    /// A clash the worker is waiting on (see `resolve`).
    pub conflict: Option<Conflict>,
}

impl FileOps {
    pub fn new() -> Self {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        let (tx, rx) = mpsc::channel();
        let (answers, answer_rx) = mpsc::channel::<Resolution>();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            let ask_tx = tx.clone();
            let ask = move |c: &Conflict| {
                let _ = ask_tx.send(Msg::Conflict(c.clone()));
                answer_rx.recv().unwrap_or(Resolution { choice: Choice::Cancel, all: false })
            };
            while let Ok(job) = job_rx.recv() {
                c.store(false, Ordering::Relaxed);
                let report = Worker::new(job.op, &tx, &c, &ask).run(&job);
                let _ = tx.send(Msg::Done(report));
            }
        });
        Self { jobs, rx, cancel, answers, pending: 0, progress: None, conflict: None }
    }

    /// Answer the clash the worker is waiting on.
    pub fn resolve(&mut self, answer: Resolution) {
        if self.conflict.take().is_some() {
            let _ = self.answers.send(answer);
        }
    }

    fn submit(&mut self, op: Op, sources: Vec<PathBuf>, dest: Option<PathBuf>) {
        if sources.is_empty() && op != Op::EmptyTrash {
            return;
        }
        self.pending += 1;
        let _ = self.jobs.send(Job { op, sources, dest });
    }

    pub fn copy(&mut self, sources: Vec<PathBuf>, dest: &Path) {
        self.submit(Op::Copy, sources, Some(dest.to_path_buf()));
    }

    pub fn move_to(&mut self, sources: Vec<PathBuf>, dest: &Path) {
        self.submit(Op::Move, sources, Some(dest.to_path_buf()));
    }

    pub fn trash(&mut self, paths: Vec<PathBuf>) {
        self.submit(Op::Trash, paths, None);
    }

    pub fn delete(&mut self, paths: Vec<PathBuf>) {
        self.submit(Op::Delete, paths, None);
    }

    pub fn empty_trash(&mut self) {
        self.submit(Op::EmptyTrash, Vec::new(), None);
    }

    /// Stop the running job after the current chunk.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.resolve(Resolution { choice: Choice::Cancel, all: false });
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// Take the worker's news: progress updates, and finished jobs' reports.
    pub fn poll(&mut self) -> Vec<Report> {
        let mut done = Vec::new();
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Progress(p) => self.progress = Some(p),
                Msg::Conflict(c) => self.conflict = Some(c),
                Msg::Done(r) => {
                    self.pending = self.pending.saturating_sub(1);
                    self.progress = None;
                    done.push(r);
                }
            }
        }
        done
    }
}

struct Worker<'a> {
    tx: &'a mpsc::Sender<Msg>,
    cancel: &'a AtomicBool,
    ask: Ask<'a>,
    /// "Apply to all" answers: for files, and for folders.
    policy: [Option<Choice>; 2],
    /// More than one item: later ones may clash too.
    multi: bool,
    p: Progress,
    last_sent: Instant,
    report: Report,
}

const CHUNK: usize = 1 << 20;

impl<'a> Worker<'a> {
    fn new(op: Op, tx: &'a mpsc::Sender<Msg>, cancel: &'a AtomicBool, ask: Ask<'a>) -> Self {
        let now = Instant::now();
        Self {
            tx,
            cancel,
            ask,
            policy: [None, None],
            multi: false,
            p: Progress { op, scanning: true, total_bytes: 0, done_bytes: 0, total_items: 0, done_items: 0, current: String::new(), started: now },
            last_sent: now - Duration::from_secs(1),
            report: Report { op, done: 0, skipped: 0, errors: Vec::new(), cancelled: false, undo: Vec::new() },
        }
    }

    fn cancelled(&mut self) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            self.report.cancelled = true;
        }
        self.report.cancelled
    }

    /// Send progress at most every 50 ms (or when forced).
    fn tick(&mut self, force: bool) {
        if force || self.last_sent.elapsed() >= Duration::from_millis(50) {
            self.last_sent = Instant::now();
            let _ = self.tx.send(Msg::Progress(self.p.clone()));
        }
    }

    fn fail(&mut self, path: &Path, e: impl std::fmt::Display) {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        tracing::warn!("fileops: {}: {e}", path.display());
        self.report.errors.push(format!("{name}: {e}"));
    }

    fn run(mut self, job: &Job) -> Report {
        let op = job.op;
        self.multi = job.sources.len() > 1;
        // Byte totals only matter where bytes get copied
        if matches!(op, Op::Copy | Op::Move) {
            for s in &job.sources {
                let (bytes, items) = measure(s);
                self.p.total_bytes += bytes;
                self.p.total_items += items;
                self.tick(false);
            }
        } else {
            self.p.total_items = job.sources.len();
        }
        self.p.scanning = false;
        self.tick(true);

        match op {
            Op::EmptyTrash => {
                for dir in ["files", "info"] {
                    let root = home_trash().join(dir);
                    for e in fs::read_dir(&root).into_iter().flatten().flatten() {
                        if self.cancelled() {
                            break;
                        }
                        if let Err(err) = remove_any(&e.path()) {
                            self.fail(&e.path(), err);
                        }
                    }
                }
            }
            _ => {
                for src in &job.sources {
                    if self.cancelled() {
                        break;
                    }
                    self.p.current = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.tick(false);
                    let result = match op {
                        Op::Copy => self.copy_top(src, job.dest.as_deref().unwrap_or(Path::new("."))),
                        Op::Move => self.move_top(src, job.dest.as_deref().unwrap_or(Path::new("."))),
                        Op::Trash => trash(src).map(|trashed| {
                            let from = std::path::absolute(src).unwrap_or_else(|_| src.clone());
                            self.report.undo.push(Undo::Trashed { from, trashed });
                            true
                        }),
                        Op::Delete => remove_any(src).map(|_| { forget_trashinfo(src); true }),
                        Op::EmptyTrash => Ok(true),
                    };
                    match result {
                        Ok(true) => self.report.done += 1,
                        Ok(false) => self.report.skipped += 1,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted && self.report.cancelled => {}
                        Err(e) => self.fail(src, e),
                    }
                    if !matches!(op, Op::Copy | Op::Move) {
                        self.p.done_items += 1;
                    }
                }
            }
        }
        self.tick(true);
        self.report
    }

    /// Settle where `src` goes when it's meant for `target`: asking the
    /// user (or following an "apply to all") if the name is taken.
    fn place(&mut self, src: &Path, target: PathBuf, more: bool) -> io::Result<Place> {
        let Ok(existing) = fs::symlink_metadata(&target) else { return Ok(Place::New(target)); };
        let src_meta = fs::symlink_metadata(src)?;
        // Pasting where it already is: a copy beside it, as nothing else makes sense
        if existing.dev() == src_meta.dev() && existing.ino() == src_meta.ino() {
            return Ok(Place::New(free_name(&target)));
        }
        let both_dirs = src_meta.is_dir() && existing.is_dir();
        let can_replace = !src_meta.is_dir() && !existing.is_dir();
        let kind = both_dirs as usize;
        let choice = match self.policy[kind].clone() {
            Some(c) => c,
            None => {
                let conflict = Conflict { op: self.p.op, src: src.to_path_buf(), dst: target.clone(), both_dirs, can_replace, more };
                let answer = (self.ask)(&conflict);
                if answer.all {
                    // A typed name can't apply to others: they get free names
                    self.policy[kind] = Some(match &answer.choice {
                        Choice::Rename(_) => Choice::AutoRename,
                        c => c.clone(),
                    });
                }
                answer.choice
            }
        };
        Ok(match choice {
            Choice::Replace if both_dirs => Place::Merge(target),
            Choice::Replace if can_replace => Place::Replace(target),
            // A file and a folder: neither can stand in for the other
            Choice::Replace | Choice::AutoRename => Place::New(free_name(&target)),
            Choice::Skip => Place::Skip,
            Choice::Rename(name) => return self.place(src, target.with_file_name(name), more),
            Choice::Cancel => {
                self.cancel.store(true, Ordering::Relaxed);
                self.report.cancelled = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
        })
    }

    /// Count a skipped item as done, so the progress still reaches the end.
    fn skip(&mut self, src: &Path) {
        let (bytes, items) = measure(src);
        self.p.done_bytes += bytes;
        self.p.done_items += items;
        self.tick(false);
    }

    fn copy_top(&mut self, src: &Path, dest_dir: &Path) -> io::Result<bool> {
        let name = src.file_name().ok_or_else(|| io::Error::other("no file name"))?;
        if dest_dir.starts_with(src) {
            return Err(io::Error::other("can't copy a folder into itself"));
        }
        let place = self.place(src, dest_dir.join(name), self.multi)?;
        let created = match &place { Place::New(t) => Some(t.clone()), _ => None };
        let copied = self.copy_placed(src, place)?;
        if copied && let Some(t) = created {
            self.report.undo.push(Undo::Created(t));
        }
        Ok(copied)
    }

    /// Copy to where `place` says; false if skipped.
    fn copy_placed(&mut self, src: &Path, place: Place) -> io::Result<bool> {
        match place {
            Place::New(t) => self.copy_tree(src, &t, false)?,
            Place::Merge(t) => self.copy_tree(src, &t, true)?,
            Place::Replace(t) => {
                // Copied beside it first: the old file stays if this fails
                let tmp = partial_name(&t);
                let _ = fs::remove_file(&tmp);
                self.copy_tree(src, &tmp, false)?;
                if let Err(e) = fs::rename(&tmp, &t) {
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
            }
            Place::Skip => {
                self.skip(src);
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn move_top(&mut self, src: &Path, dest_dir: &Path) -> io::Result<bool> {
        let name = src.file_name().ok_or_else(|| io::Error::other("no file name"))?;
        if src.parent() == Some(dest_dir) {
            // Already there
            let (bytes, items) = measure(src);
            self.p.done_bytes += bytes;
            self.p.done_items += items;
            return Ok(true);
        }
        if dest_dir.starts_with(src) {
            return Err(io::Error::other("can't move a folder into itself"));
        }
        let place = self.place(src, dest_dir.join(name), self.multi)?;
        let to = match &place { Place::New(t) | Place::Replace(t) => Some(t.clone()), _ => None };
        let moved = self.move_placed(src, place)?;
        if moved && let Some(to) = to {
            self.report.undo.push(Undo::Moved { from: src.to_path_buf(), to });
        }
        Ok(moved)
    }

    /// Move to where `place` says; false if skipped.
    fn move_placed(&mut self, src: &Path, place: Place) -> io::Result<bool> {
        let target = match place {
            Place::Skip => {
                self.skip(src);
                return Ok(false);
            }
            Place::Merge(t) => {
                // Each item into the existing folder, clashes asked in turn;
                // the source folder goes once it's empty (skips keep it)
                for e in fs::read_dir(src)? {
                    if self.cancelled() {
                        return Err(io::ErrorKind::Interrupted.into());
                    }
                    let e = e?;
                    let result = self.place(&e.path(), t.join(e.file_name()), true)
                        .and_then(|place| self.move_placed(&e.path(), place));
                    if let Err(err) = result {
                        if err.kind() == io::ErrorKind::Interrupted {
                            return Err(err);
                        }
                        self.fail(&e.path(), err);
                    }
                }
                let _ = fs::remove_dir(src);
                return Ok(true);
            }
            Place::New(t) | Place::Replace(t) => t,
        };
        // A rename replaces a file atomically
        match fs::rename(src, &target) {
            Ok(()) => {
                let (bytes, items) = measure(&target);
                self.p.done_bytes += bytes;
                self.p.done_items += items;
                Ok(true)
            }
            // Another filesystem: copy, then remove the original
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
                let errors = self.report.errors.len();
                let replace = fs::symlink_metadata(&target).is_ok();
                self.copy_placed(src, if replace { Place::Replace(target) } else { Place::New(target) })?;
                // Anything inside that didn't copy keeps the original
                if self.report.errors.len() > errors {
                    return Err(io::Error::other("not everything copied; original kept"));
                }
                remove_any(src)?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    /// Copy `src` to `dst`; with `merge`, `dst` is an existing folder to
    /// write into, its clashing items asked about one by one.
    fn copy_tree(&mut self, src: &Path, dst: &Path, merge: bool) -> io::Result<()> {
        let meta = fs::symlink_metadata(src)?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
        } else if ft.is_dir() {
            if !merge {
                fs::create_dir(dst)?;
            }
            for e in fs::read_dir(src)? {
                if self.cancelled() {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let e = e?;
                let target = dst.join(e.file_name());
                let result = if merge {
                    self.place(&e.path(), target, true).and_then(|place| self.copy_placed(&e.path(), place).map(|_| ()))
                } else {
                    self.copy_tree(&e.path(), &target, false)
                };
                if let Err(err) = result {
                    if err.kind() == io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                    self.fail(&e.path(), err);
                }
            }
            let _ = fs::set_permissions(dst, fs::Permissions::from_mode(meta.mode()));
        } else {
            self.p.current = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Err(e) = self.copy_file(src, dst, &meta) {
                let _ = fs::remove_file(dst);
                return Err(e);
            }
        }
        self.p.done_items += 1;
        self.tick(false);
        Ok(())
    }

    fn copy_file(&mut self, src: &Path, dst: &Path, meta: &fs::Metadata) -> io::Result<()> {
        let mut from = fs::File::open(src)?;
        let mut to = fs::OpenOptions::new().write(true).create_new(true).open(dst)?;
        let mut buf = vec![0u8; CHUNK];
        loop {
            if self.cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let n = from.read(&mut buf)?;
            if n == 0 {
                break;
            }
            to.write_all(&buf[..n])?;
            self.p.done_bytes += n as u64;
            self.tick(false);
        }
        let _ = to.set_permissions(fs::Permissions::from_mode(meta.mode()));
        if let Ok(m) = meta.modified() {
            let _ = to.set_modified(m);
        }
        Ok(())
    }
}

/// Bytes and items under `path` (not following symlinks).
fn measure(path: &Path) -> (u64, usize) {
    let Ok(meta) = fs::symlink_metadata(path) else { return (0, 0) };
    if !meta.is_dir() {
        return (if meta.is_file() { meta.len() } else { 0 }, 1);
    }
    let (mut bytes, mut items) = (0, 1);
    for e in fs::read_dir(path).into_iter().flatten().flatten() {
        let (b, i) = measure(&e.path());
        bytes += b;
        items += i;
    }
    (bytes, items)
}

fn remove_any(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// A hidden sibling to copy into before replacing `path`.
fn partial_name(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.part"))
}

/// `path`, or the first free "name (2).ext", "name (3).ext", ...
pub fn free_name(path: &Path) -> PathBuf {
    if fs::symlink_metadata(path).is_err() {
        return path.to_path_buf();
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Extension stays at the end ("photo (2).jpg"); folders and dotfiles keep it whole
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !path.is_dir() => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| fs::symlink_metadata(p).is_err())
        .unwrap_or_else(|| path.to_path_buf())
}

// ---------------------------------------------------------------------------
// Trash (freedesktop.org Trash spec)
// ---------------------------------------------------------------------------

pub fn in_trash(path: &Path) -> bool {
    path.starts_with(home_trash().join("files"))
}

fn forget_trashinfo(src: &Path) {
    if src.parent() == Some(home_trash().join("files").as_path()) && let Some(name) = src.file_name() {
        let mut info = name.to_os_string();
        info.push(".trashinfo");
        let _ = fs::remove_file(home_trash().join("info").join(info));
    }
}

pub fn home_trash() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("Trash")
}

/// Move `path` to the trash on its own filesystem: the home trash, or a
/// `.Trash-$uid` at the top of other mounts. Returns where it went.
pub fn trash(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let meta = fs::symlink_metadata(&path)?;
    let home = home_trash();
    let home_dev = fs::create_dir_all(&home).and_then(|_| fs::metadata(&home)).map(|m| m.dev());
    let (trash_dir, info_path) = if home_dev.is_ok_and(|d| d == meta.dev()) {
        (home, path.clone())
    } else {
        let top = mount_top(&path, meta.dev());
        let uid = nix::unistd::getuid().as_raw();
        let dir = top.join(format!(".Trash-{uid}"));
        // Path is relative to the mount's top in a topdir trash
        let rel = path.strip_prefix(&top).map(Path::to_path_buf).unwrap_or_else(|_| path.clone());
        (dir, rel)
    };
    trash_into(&trash_dir, &path, &info_path)
}

fn trash_into(trash_dir: &Path, path: &Path, info_path: &Path) -> io::Result<PathBuf> {
    let (files, info) = (trash_dir.join("files"), trash_dir.join("info"));
    for d in [trash_dir, &files, &info] {
        if !d.exists() {
            fs::create_dir_all(d)?;
            let _ = fs::set_permissions(d, fs::Permissions::from_mode(0o700));
        }
    }
    let name = path.file_name().ok_or_else(|| io::Error::other("no file name"))?.to_string_lossy().into_owned();
    // Reserve a name by creating its .trashinfo first (atomic, per spec)
    let encoded = crate::thumbs::file_uri(info_path);
    let encoded = encoded.strip_prefix("file://").unwrap_or(&encoded);
    let encoded = if info_path.is_relative() { encoded.trim_start_matches('/') } else { encoded };
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    for n in 1.. {
        let candidate = if n == 1 { name.clone() } else { format!("{name}.{n}") };
        let info_file = info.join(format!("{candidate}.trashinfo"));
        match fs::OpenOptions::new().write(true).create_new(true).open(&info_file) {
            Ok(mut f) => {
                f.write_all(format!("[Trash Info]\nPath={encoded}\nDeletionDate={date}\n").as_bytes())?;
                let target = files.join(&candidate);
                if let Err(e) = fs::rename(path, &target) {
                    let _ = fs::remove_file(&info_file);
                    return Err(e);
                }
                return Ok(target);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// The top directory of the mount holding `path` (device `dev`).
fn mount_top(path: &Path, dev: u64) -> PathBuf {
    let mut top = path.to_path_buf();
    while let Some(parent) = top.parent() {
        if fs::metadata(parent).map(|m| m.dev()).ok() != Some(dev) {
            break;
        }
        top = parent.to_path_buf();
    }
    top
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run until done, answering each clash with `answer` (and noting it).
    fn answering(ops: &mut FileOps, seen: &mut Vec<Conflict>, mut answer: impl FnMut(&Conflict) -> Resolution) -> Report {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let done = ops.poll().pop();
            if let Some(c) = ops.conflict.clone() {
                let a = answer(&c);
                seen.push(c);
                ops.resolve(a);
            }
            if let Some(r) = done {
                return r;
            }
            assert!(Instant::now() < deadline, "job didn't finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn once(choice: Choice) -> Resolution {
        Resolution { choice, all: false }
    }

    #[test]
    fn copy_move_conflicts() {
        let root = std::env::temp_dir().join(format!("fbt-fileops-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (src, dst) = (root.join("src"), root.join("dst"));
        fs::create_dir_all(src.join("tree/inner")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("tree/inner/a.txt"), vec![7u8; 3 * CHUNK + 5]).unwrap();
        fs::write(src.join("note.txt"), "hi").unwrap();
        std::os::unix::fs::symlink("inner/a.txt", src.join("tree/link")).unwrap();
        fs::write(dst.join("note.txt"), "old").unwrap();
        let mut seen = Vec::new();

        // A clash asks; a free name keeps both
        let mut ops = FileOps::new();
        ops.copy(vec![src.join("tree"), src.join("note.txt")], &dst);
        let r = answering(&mut ops, &mut seen, |_| once(Choice::AutoRename));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.done, 2);
        assert_eq!(seen.len(), 1);
        assert!(seen[0].can_replace && !seen[0].both_dirs && seen[0].more);
        assert_eq!(fs::read(dst.join("tree/inner/a.txt")).unwrap().len(), 3 * CHUNK + 5);
        assert_eq!(fs::read_link(dst.join("tree/link")).unwrap(), PathBuf::from("inner/a.txt"));
        assert_eq!(fs::read_to_string(dst.join("note.txt")).unwrap(), "old");
        assert_eq!(fs::read_to_string(dst.join("note (2).txt")).unwrap(), "hi");

        // Copy into itself is refused; pasting where it is makes a copy beside it
        ops.copy(vec![src.clone()], &src.join("tree"));
        assert_eq!(answering(&mut ops, &mut seen, |_| once(Choice::Cancel)).errors.len(), 1);
        ops.copy(vec![dst.join("note.txt")], &dst);
        answering(&mut ops, &mut seen, |_| panic!("asked about copying onto itself"));
        assert!(dst.join("note (3).txt").exists());

        // Writing into a folder: new items go in, clashes inside are asked
        fs::write(src.join("tree/inner/a.txt"), "new a").unwrap();
        fs::write(src.join("tree/extra.txt"), "extra").unwrap();
        seen.clear();
        ops.copy(vec![src.join("tree")], &dst);
        let r = answering(&mut ops, &mut seen, |c| once(if c.both_dirs || c.src.ends_with("a.txt") { Choice::Replace } else { Choice::Skip }));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(fs::read_to_string(dst.join("tree/inner/a.txt")).unwrap(), "new a");
        assert_eq!(fs::read_to_string(dst.join("tree/extra.txt")).unwrap(), "extra");
        assert!(seen[0].both_dirs && seen.iter().any(|c| c.src.ends_with("link")));
        assert!(!dst.join("tree/inner/.a.txt.part").exists());

        // A typed name; then skipping, which leaves the source where it is
        ops.copy(vec![src.join("note.txt")], &dst);
        answering(&mut ops, &mut seen, |_| once(Choice::Rename("custom.txt".into())));
        assert_eq!(fs::read_to_string(dst.join("custom.txt")).unwrap(), "hi");
        ops.move_to(vec![src.join("note.txt")], &dst);
        let r = answering(&mut ops, &mut seen, |_| once(Choice::Skip));
        assert_eq!((r.done, r.skipped), (0, 1));
        assert!(src.join("note.txt").exists());
        assert_eq!(r.message(), "Moved 0 items, skipped 1");

        // Moving into a folder with "apply to all": asked once for folders
        // and once for files, and the source is emptied
        seen.clear();
        ops.move_to(vec![src.join("tree")], &dst);
        let r = answering(&mut ops, &mut seen, |_| Resolution { choice: Choice::Replace, all: true });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(seen.iter().map(|c| c.both_dirs).collect::<Vec<_>>(), [true, false]);
        assert!(!src.join("tree").exists());
        assert_eq!(fs::read_to_string(dst.join("tree/inner/a.txt")).unwrap(), "new a");

        // Cancel stops the job
        ops.copy(vec![dst.join("note.txt")], &src);
        fs::write(src.join("note.txt"), "x").unwrap();
        ops.copy(vec![dst.join("note.txt")], &src);
        let first = answering(&mut ops, &mut seen, |_| once(Choice::Cancel));
        let second = if first.cancelled { first } else { answering(&mut ops, &mut seen, |_| once(Choice::Cancel)) };
        assert!(second.cancelled);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn cancel_cleans_up() {
        let root = std::env::temp_dir().join(format!("fbt-cancel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("dst")).unwrap();
        fs::write(root.join("big.bin"), vec![1u8; 4 * CHUNK]).unwrap();
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(true);
        let job = Job { op: Op::Copy, sources: vec![root.join("big.bin")], dest: Some(root.join("dst")) };
        let r = Worker::new(Op::Copy, &tx, &cancel, &|_| once(Choice::Cancel)).run(&job);
        assert!(r.cancelled && r.done == 0 && r.errors.is_empty(), "{r:?}");
        assert!(!root.join("dst/big.bin").exists());
        assert_eq!(r.message(), "Cancelled (0 items done)");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn trash_spec() {
        let root = std::env::temp_dir().join(format!("fbt-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let trash = root.join("Trash");
        for _ in 0..2 {
            let f = root.join("my file.txt");
            fs::write(&f, "x").unwrap();
            trash_into(&trash, &f, &f).unwrap();
        }
        assert!(trash.join("files/my file.txt").exists() && trash.join("files/my file.txt.2").exists());
        let info = fs::read_to_string(trash.join("info/my file.txt.2.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/") && info.contains("my%20file.txt\nDeletionDate=20"), "{info}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn undo_records_and_reverts() {
        let root = std::env::temp_dir().join(format!("fbt-undo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (src, dst) = (root.join("src"), root.join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("a.txt"), "a").unwrap();
        fs::write(src.join("b.txt"), "b").unwrap();
        fs::write(dst.join("b.txt"), "old b").unwrap();
        let mut seen = Vec::new();
        let mut ops = FileOps::new();

        // A copy records what it made, not what it overwrote
        ops.copy(vec![src.join("a.txt"), src.join("b.txt")], &dst);
        let r = answering(&mut ops, &mut seen, |_| once(Choice::Replace));
        assert_eq!(r.undo, vec![Undo::Created(dst.join("a.txt"))]);

        // A move goes back where it was
        fs::create_dir(root.join("moved")).unwrap();
        ops.move_to(vec![src.join("a.txt")], &root.join("moved"));
        let r = answering(&mut ops, &mut seen, |_| panic!("no clash expected"));
        assert_eq!(r.undo, vec![Undo::Moved { from: src.join("a.txt"), to: root.join("moved/a.txt") }]);
        r.undo[0].revert().unwrap();
        assert_eq!(fs::read_to_string(src.join("a.txt")).unwrap(), "a");
        assert!(!root.join("moved/a.txt").exists());

        // Something new in the way: refused, nothing lost
        let step = Undo::Moved { from: src.join("b.txt"), to: dst.join("b.txt") };
        assert!(step.revert().is_err());
        assert_eq!(fs::read_to_string(dst.join("b.txt")).unwrap(), "b");

        // Out of the trash, its .trashinfo gone with it
        let trash_dir = root.join("Trash");
        let f = src.join("a.txt");
        let trashed = trash_into(&trash_dir, &f, &f).unwrap();
        assert!(!f.exists());
        Undo::Trashed { from: f.clone(), trashed }.revert().unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "a");
        assert_eq!(fs::read_dir(trash_dir.join("info")).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&root);
    }
}
