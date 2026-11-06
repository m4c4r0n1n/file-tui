pub mod generator;

use std::{
    collections::HashSet,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::mpsc,
};

use lru::LruCache;
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};

use crate::icons::{GRID_FIT, fit_to_cells};

pub struct ThumbReady {
    pub path: PathBuf,
    /// None when no thumbnail could be made (the file shows its icon).
    pub image: Option<image::DynamicImage>,
    /// Made only to have it on disk (a folder not open yet): not kept.
    warm_only: bool,
}

/// How soon a thumbnail is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// On screen.
    Visible,
    /// Just past the screen, for scrolling.
    Ahead,
    /// In a folder on screen, for opening it; made for the disk cache only.
    Depth,
}

struct ThumbRequest {
    path: PathBuf,
    mime_type: String,
    warm_only: bool,
}

/// Work waiting for the thumbnail workers, by priority.
#[derive(Default)]
struct Queue {
    jobs: [std::collections::VecDeque<ThumbRequest>; 3],
    running: HashSet<PathBuf>,
    /// Ahead/Depth jobs being worked on.
    running_bg: usize,
    quit: bool,
}

type Shared = std::sync::Arc<(std::sync::Mutex<Queue>, std::sync::Condvar)>;

type ProtoKey = (PathBuf, u16, u16, Option<(u8, u8, u8)>, bool);

pub struct ThumbCache {
    images: LruCache<PathBuf, image::DynamicImage>,
    protocols: LruCache<ProtoKey, StatefulProtocol>,
    shared: Shared,
    rx_image: mpsc::Receiver<ThumbReady>,
    /// Queued or being made.
    pending: HashSet<PathBuf>,
    /// The part of `pending` that's on screen.
    pending_visible: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    /// Made for the disk cache this session (Depth), so not asked again.
    warmed: HashSet<PathBuf>,
    /// Fingerprint of the last `plan`, to skip replanning an unchanged view.
    planned: u64,
}

impl ThumbCache {
    pub fn new() -> Self {
        let (tx_image, rx_image) = mpsc::channel::<ThumbReady>();
        let shared: Shared = Default::default();

        // Thumbnailers mostly wait on decoders and ffmpeg, so they get their
        // own threads rather than the icon pool. Two are kept for what's on
        // screen: background work never takes the last of them
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(3, 8);
        let bg_limit = workers - 2;
        for _ in 0..workers {
            let (shared, tx) = (shared.clone(), tx_image.clone());
            std::thread::spawn(move || {
                let (lock, cv) = &*shared;
                loop {
                    let (req, bg) = {
                        let mut q = lock.lock().unwrap();
                        let picked = loop {
                            if q.quit {
                                return;
                            }
                            if let Some(r) = q.jobs[0].pop_front() {
                                break (r, false);
                            }
                            if q.running_bg < bg_limit
                                && let Some(r) = q.jobs[1].pop_front().or_else(|| q.jobs[2].pop_front())
                            {
                                q.running_bg += 1;
                                break (r, true);
                            }
                            q = cv.wait(q).unwrap();
                        };
                        q.running.insert(picked.0.path.clone());
                        picked
                    };
                    let image = generate_or_load_thumb(&req.path, &req.mime_type)
                        .map_err(|e| tracing::debug!("thumb {:?}: {e}", req.path))
                        .ok();
                    let path = req.path.clone();
                    // Sent before leaving `running`, so a replan in between
                    // can't queue it again
                    let _ = tx.send(ThumbReady { path: req.path, image, warm_only: req.warm_only });
                    let mut q = lock.lock().unwrap();
                    q.running.remove(&path);
                    if bg {
                        q.running_bg -= 1;
                        cv.notify_one();
                    }
                }
            });
        }

        Self {
            // A few screens' worth: what's shown plus what's ahead
            images: LruCache::new(NonZeroUsize::new(600).unwrap()),
            protocols: LruCache::new(NonZeroUsize::new(200).unwrap()),
            shared,
            rx_image,
            pending: HashSet::new(),
            pending_visible: HashSet::new(),
            failed: HashSet::new(),
            warmed: HashSet::new(),
            planned: 0,
        }
    }

    /// Whether thumbnails on screen are still being made.
    pub fn loading(&self) -> bool {
        !self.pending_visible.is_empty()
    }

    /// Whether making a thumbnail for this path already failed.
    pub fn failed(&self, path: &Path) -> bool {
        self.failed.contains(path)
    }

    fn wanted(&self, path: &Path, prio: Priority) -> bool {
        !self.images.contains(path) && !self.failed.contains(path)
            && !(prio == Priority::Depth && (self.warmed.contains(path) || keep_off_disk(path)))
    }

    /// Replace the queued work with these, most wanted first; anything
    /// queued that isn't among them (scrolled away, folder left) is dropped.
    /// Cheap to call every frame: an unchanged plan is skipped.
    pub fn plan(&mut self, jobs: &[(&Path, &str, Priority)]) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (path, _, prio) in jobs {
            if self.wanted(path, *prio) {
                (path, *prio as u8).hash(&mut h);
            }
        }
        let fingerprint = h.finish();
        if fingerprint == self.planned {
            return;
        }
        self.planned = fingerprint;

        let (lock, cv) = &*self.shared;
        let mut q = lock.lock().unwrap();
        for queue in &mut q.jobs {
            queue.clear();
        }
        let mut pending: HashSet<PathBuf> = self.pending.iter().filter(|p| q.running.contains(*p)).cloned().collect();
        self.pending_visible.retain(|p| pending.contains(p));
        for &(path, mime, prio) in jobs {
            if prio == Priority::Visible && self.wanted(path, prio) {
                self.pending_visible.insert(path.to_path_buf());
            }
            if !self.wanted(path, prio) || pending.contains(path) {
                continue;
            }
            pending.insert(path.to_path_buf());
            q.jobs[prio as usize].push_back(ThumbRequest {
                path: path.to_path_buf(),
                mime_type: mime.to_string(),
                warm_only: prio == Priority::Depth,
            });
        }
        drop(q);
        cv.notify_all();
        self.pending = pending;
    }

    pub fn drain_ready(&mut self) {
        while let Ok(ready) = self.rx_image.try_recv() {
            self.pending.remove(&ready.path);
            self.pending_visible.remove(&ready.path);
            match ready.image {
                Some(_) if ready.warm_only => { self.warmed.insert(ready.path); }
                Some(img) => { self.images.put(ready.path, img); }
                None => { self.failed.insert(ready.path); }
            }
            // Something changed: the next plan isn't a repeat
            self.planned = 0;
        }
    }

    pub fn get_mut(
        &mut self,
        picker: &Picker,
        path: &Path,
        cols: u16,
        rows: u16,
        bg: Option<(u8, u8, u8)>,
        dim: bool,
    ) -> Option<&mut StatefulProtocol> {
        let key = (path.to_path_buf(), cols, rows, bg, dim);
        if !self.protocols.contains(&key) {
            let img = self.images.get(path)?;
            let mut img = fit_to_cells(img, picker, cols, rows, bg, GRID_FIT);
            if dim {
                crate::icons::dim_image(&mut img, bg);
            }
            self.protocols.put(key.clone(), picker.new_resize_protocol(img));
        }
        self.protocols.get_mut(&key)
    }

    pub fn invalidate(&mut self, path: &Path) {
        let keys: Vec<ProtoKey> = self.protocols.iter()
            .filter(|(k, _)| k.0 == path)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            self.protocols.pop(&k);
        }
    }

    pub fn clear_protocols(&mut self) {
        self.protocols.clear();
    }
}

impl Drop for ThumbCache {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut q) = lock.lock() {
            q.quit = true;
        }
        cv.notify_all();
    }
}

/// Freedesktop thumbnail dirs, in lookup order. New thumbnails go in `large`.
const SIZE_DIRS: [&str; 4] = ["large", "x-large", "xx-large", "normal"];

fn thumbnails_root() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| PathBuf::from("/tmp")).join("thumbnails")
}

/// file:// URI as the thumbnail spec (and Dolphin/GLib) hash it: path bytes
/// percent-encoded except unreserved and path-safe characters.
pub(crate) fn file_uri(abs_path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in abs_path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

fn mtime_secs(path: &Path) -> Option<u64> {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs())
}

/// A cached thumbnail is valid if its Thumb::MTime matches the source, or,
/// lacking that tag, if it is newer than the source.
fn is_thumb_valid(thumb_path: &Path, source_mtime: Option<u64>) -> bool {
    let Ok(file) = std::fs::File::open(thumb_path) else { return false; };
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let Ok(reader) = decoder.read_info() else { return false; };
    let tagged = reader.info().uncompressed_latin1_text.iter()
        .find(|t| t.keyword == "Thumb::MTime")
        .and_then(|t| t.text.parse::<u64>().ok());
    match (tagged, source_mtime) {
        (Some(tag), Some(src)) => tag == src,
        _ => mtime_secs(thumb_path) >= source_mtime,
    }
}

/// Save as PNG with the spec's Thumb::URI / Thumb::MTime tags so other
/// thumbnail consumers (Dolphin, Nautilus) accept and reuse it.
fn save_thumb(img: &image::DynamicImage, out: &Path, uri: &str, mtime: Option<u64>) -> anyhow::Result<()> {
    let rgba = img.to_rgba8();
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut enc = png::Encoder::new(file, rgba.width(), rgba.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.add_text_chunk("Thumb::URI".into(), uri.into())?;
    if let Some(m) = mtime {
        enc.add_text_chunk("Thumb::MTime".into(), m.to_string())?;
    }
    enc.add_text_chunk("Software".into(), "file-tui".into())?;
    enc.write_header()?.write_image_data(rgba.as_raw())?;
    Ok(())
}

/// Kinds of files shown as thumbnails of their content (`thumbnails` in
/// the config).
#[derive(Clone)]
pub struct Previews(HashSet<String>);

/// The kinds `thumbnails` accepts, and what each covers.
pub const KINDS: &[(&str, &str)] = &[
    ("images", "pictures (JPEG, PNG, WebP, GIF, ...)"),
    ("svg", "SVG drawings"),
    ("videos", "a frame of each video (ffmpeg)"),
    ("audio", "album art embedded in audio files (ffmpeg)"),
    ("ebooks", "EPUB covers"),
    ("comics", "comic book (CBZ) covers"),
    ("documents", "OpenDocument files (LibreOffice)"),
    ("pdf", "the first page of PDFs (pdftoppm)"),
];

/// Everything but PDFs, which are slow to render.
pub const DEFAULT_KINDS: &[&str] = &["images", "svg", "videos", "audio", "ebooks", "comics", "documents"];

static GLOBAL: std::sync::OnceLock<Previews> = std::sync::OnceLock::new();
/// `thumbnail_cache_exclude`, resolved.
static NO_DISK: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();

fn keep_off_disk(abs: &Path) -> bool {
    NO_DISK.get().is_some_and(|dirs| dirs.iter().any(|d| abs.starts_with(d)))
}

/// The startup preview settings, for background workers (folder previews).
pub fn previews() -> Option<&'static Previews> {
    GLOBAL.get()
}

impl Previews {
    /// From the config; also returns the names it didn't know.
    pub fn new(config: &crate::config::Config) -> (Self, Vec<String>) {
        let names: Vec<String> = config.thumbnails.clone()
            .unwrap_or_else(|| DEFAULT_KINDS.iter().map(|s| s.to_string()).collect());
        let unknown = names.iter().filter(|n| !KINDS.iter().any(|(k, _)| k == n)).cloned().collect();
        let p = Self(names.into_iter().collect());
        let _ = GLOBAL.set(p.clone());
        let home = dirs::home_dir().unwrap_or_default();
        let _ = NO_DISK.set(config.thumbnail_cache_exclude.iter().map(|d| {
            let path = match d.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None if d == "~" => home.clone(),
                None => PathBuf::from(d),
            };
            path.canonicalize().unwrap_or(path)
        }).collect());
        (p, unknown)
    }

    fn on(&self, kind: &str) -> bool {
        self.0.contains(kind)
    }

    /// Whether files of this type are shown as a thumbnail of their content.
    pub fn covers(&self, mime: &str) -> bool {
        match mime {
            "image/svg+xml" | "image/svg+xml-compressed" => self.on("svg"),
            "application/epub+zip" => self.on("ebooks"),
            "application/pdf" => self.on("pdf"),
            "application/vnd.comicbook+zip" => self.on("comics"),
            m if m.starts_with("image/") => self.on("images"),
            m if m.starts_with("video/") => self.on("videos"),
            m if m.starts_with("audio/") => self.on("audio"),
            m if m.starts_with("application/vnd.oasis.opendocument.") => self.on("documents"),
            _ => false,
        }
    }
}

pub(crate) fn generate_or_load_thumb(path: &Path, mime_type: &str) -> anyhow::Result<image::DynamicImage> {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let uri = file_uri(&abs);
    let name = format!("{:x}.png", md5::compute(uri.as_bytes()));
    let root = thumbnails_root();
    let src_mtime = mtime_secs(&abs);

    if keep_off_disk(&abs) {
        return generate_in_memory(&abs, mime_type);
    }

    // Reuse any valid thumbnail another app (or we) already made, any size
    for dir in SIZE_DIRS {
        let candidate = root.join(dir).join(&name);
        if is_thumb_valid(&candidate, src_mtime)
            && let Ok(img) = image::open(&candidate)
        {
            return Ok(img);
        }
    }
    // Failed before and unchanged since: don't run the thumbnailer again
    let fail_dir = root.join("fail/file-tui");
    let fail_path = fail_dir.join(&name);
    if src_mtime.is_some() && is_thumb_valid(&fail_path, src_mtime) {
        anyhow::bail!("thumbnailing failed before");
    }

    let large = root.join("large");
    std::fs::create_dir_all(&large)?;
    let thumb_path = large.join(&name);
    // Unique per request so concurrent workers never share a temp file
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = large.join(format!("{name}.{}-{seq}.tmp.png", std::process::id()));

    let generated = generate(&abs, mime_type, &tmp);
    let img = match generated {
        Ok(img) => img,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            // Remember it (as the spec's fail marker) unless the tool itself
            // is missing, so installing it later still works
            let tool_missing = e.chain().any(|c| c.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound));
            if !tool_missing && std::fs::create_dir_all(&fail_dir).is_ok() {
                let marker = fail_dir.join(format!("{name}.{}-{seq}.tmp.png", std::process::id()));
                let blank = image::DynamicImage::new_rgba8(1, 1);
                if save_thumb(&blank, &marker, &uri, src_mtime).is_ok() {
                    let _ = std::fs::rename(&marker, &fail_path);
                }
            }
            return Err(e);
        }
    };

    save_thumb(&img, &tmp, &uri, src_mtime)?;
    std::fs::rename(&tmp, &thumb_path)?;
    Ok(img)
}



/// Run the thumbnailer for this type (some write their result to `tmp`).
fn generate(abs: &Path, mime_type: &str, tmp: &Path) -> anyhow::Result<image::DynamicImage> {
    if mime_type.starts_with("video/") {
        generator::video_thumb(abs, tmp)
    } else if mime_type.starts_with("image/svg") {
        crate::icons::raster::load_icon(abs, true, 256)
    } else if mime_type.starts_with("image/") {
        generator::image_thumb(abs)
    } else if mime_type.starts_with("audio/") {
        generator::audio_thumb(abs, tmp)
    } else if mime_type == "application/epub+zip" {
        generator::epub_thumb(abs)
    } else if mime_type == "application/pdf" {
        generator::pdf_thumb(abs, tmp)
    } else if mime_type == "application/vnd.comicbook+zip" {
        generator::comic_thumb(abs)
    } else if mime_type.starts_with("application/vnd.oasis.opendocument.") {
        generator::opendocument_thumb(abs)
    } else {
        anyhow::bail!("no thumbnailer for {mime_type}")
    }
}

/// A thumbnail for an excluded folder: made fresh, with any scratch file
/// in the (RAM-backed) runtime dir and gone right after.
fn generate_in_memory(abs: &Path, mime_type: &str) -> anyhow::Result<image::DynamicImage> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
    let tmp = dir.join(format!("file-tui-thumb-{}-{seq}.png", std::process::id()));
    let result = if mime_type.starts_with("image/svg") {
        // Past the icon rasterizer, whose cache is on disk
        crate::icons::raster::rasterize_svg(abs, 256, &tmp).and_then(|()| Ok(image::open(&tmp)?))
    } else {
        generate(abs, mime_type, &tmp)
    };
    let _ = std::fs::remove_file(&tmp);
    result
}
