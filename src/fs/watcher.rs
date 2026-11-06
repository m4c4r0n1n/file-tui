use std::{
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify, WatchDescriptor};

#[derive(Debug)]
pub enum DirEvent {
    Changed,
}

/// Single long-lived inotify watcher; `watch` swaps the watched directory.
pub struct DirWatcher {
    inotify: Arc<Inotify>,
    current: Arc<Mutex<Option<WatchDescriptor>>>,
}

impl DirWatcher {
    pub fn spawn(tx: mpsc::Sender<DirEvent>) -> Option<Self> {
        let inotify = match Inotify::init(InitFlags::IN_CLOEXEC) {
            Ok(i) => Arc::new(i),
            Err(e) => { tracing::error!("inotify init: {e}"); return None; }
        };
        let current: Arc<Mutex<Option<WatchDescriptor>>> = Arc::new(Mutex::new(None));

        let (ino, cur) = (inotify.clone(), current.clone());
        thread::spawn(move || loop {
            match ino.read_events() {
                Ok(events) => {
                    // Ignore events still queued for a previously watched dir
                    let wd = *cur.lock().unwrap();
                    if events.iter().any(|e| Some(e.wd) == wd) && tx.send(DirEvent::Changed).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    tracing::error!("inotify read_events: {e}");
                    return;
                }
            }
        });

        Some(Self { inotify, current })
    }

    pub fn watch(&self, path: &Path) {
        let flags = AddWatchFlags::IN_CREATE
            | AddWatchFlags::IN_DELETE
            | AddWatchFlags::IN_MOVED_FROM
            | AddWatchFlags::IN_MOVED_TO
            | AddWatchFlags::IN_CLOSE_WRITE;

        let mut cur = self.current.lock().unwrap();
        if let Some(old) = cur.take() {
            let _ = self.inotify.rm_watch(old);
        }
        match self.inotify.add_watch(path, flags) {
            Ok(wd) => *cur = Some(wd),
            Err(e) => tracing::warn!("inotify add_watch {:?}: {e}", path),
        }
    }
}
