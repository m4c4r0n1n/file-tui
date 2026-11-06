pub mod folder_preview;
mod theme;
pub mod raster;
pub mod resolver;

use std::sync::Arc;
use std::{
    collections::HashSet,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::mpsc,
};

use lru::LruCache;
use ratatui_image::{picker::Picker, protocol::StatefulProtocol};

pub use theme::{set_theme, theme_name};

pub struct IconImageReady {
    pub path: PathBuf,
    pub image: Option<Arc<image::DynamicImage>>,
    pub mime_type: Option<String>,
    /// A folder preview, which a plain icon arriving later mustn't replace.
    pub preview: bool,
}

struct IconRequest {
    path: PathBuf,
    is_dir: bool,
    target_px: u32,
    /// Custom icon name (config folder_icons) tried before normal resolution.
    icon_override: Option<String>,
}

/// Overlay the theme's symlink emblem on the bottom-left corner, as Dolphin
/// marks links.
fn add_link_emblem(img: &mut image::DynamicImage) {
    let size = (img.width().min(img.height()) / 4).max(8);
    let Some((path, is_svg)) = theme::lookup("emblem-symbolic-link") else { return; };
    let Ok(emblem) = raster::load_icon(&path, is_svg, size) else { return; };
    let emblem = emblem.resize(size, size, image::imageops::FilterType::Triangle);
    let y = img.height().saturating_sub(emblem.height());
    image::imageops::overlay(img, &emblem, 0, y as i64);
}

/// How an image sits in its cell box.
#[derive(Clone, Copy)]
pub struct Fit {
    /// Largest image height as a fraction of the box (the rest is margin).
    pub max_h: f32,
    /// Cap on the image height in terminal rows (0 = none).
    pub max_rows: f32,
    pub valign: VAlign,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VAlign {
    Center,
    /// Resting on the bottom edge (right above a label).
    Bottom,
}

/// Grid items: a small margin above, resting on the label.
pub const GRID_FIT: Fit = Fit { max_h: 0.88, max_rows: 0.0, valign: VAlign::Bottom };

/// Fit `img` into a box of `cols`×`rows` terminal cells and pad it to exactly
/// that box, centered horizontally and resting on the bottom edge (right
/// above the label). Image protocols pin a fitted image to the top-left, so
/// without this, portrait thumbnails sit off-center and wide ones float.
/// With `bg`, the padding and any transparency are filled with that color so
/// a selection highlight covers the whole box (Kitty composites alpha over
/// the terminal background, not the cell background).
pub fn fit_to_cells(
    img: &image::DynamicImage,
    picker: &Picker,
    cols: u16,
    rows: u16,
    bg: Option<(u8, u8, u8)>,
    fit: Fit,
) -> image::DynamicImage {
    let font = picker.font_size();
    let (box_w, box_h) = (
        (cols as u32 * font.width as u32).max(1),
        (rows as u32 * font.height as u32).max(1),
    );
    let mut max_h = box_h as f32 * fit.max_h;
    if fit.max_rows > 0.0 {
        max_h = max_h.min(fit.max_rows * font.height as f32);
    }
    let max_h = (max_h as u32).max(1);
    // Icons come rasterized at this size (see App::icon_target_px), so
    // usually nothing is resized; shrinking otherwise averages areas,
    // much faster than a filter
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let scale = (box_w as f32 / iw as f32).min(max_h as f32 / ih as f32);
    let (w, h) = (((iw as f32 * scale).round() as u32).clamp(1, box_w), ((ih as f32 * scale).round() as u32).clamp(1, max_h));
    let fitted = if (w, h) == (iw, ih) {
        img.to_rgba8()
    } else if scale < 1.0 {
        image::imageops::thumbnail(&img.to_rgba8(), w, h)
    } else {
        img.resize(box_w, max_h, image::imageops::FilterType::Triangle).to_rgba8()
    };
    let fill = match bg {
        Some((r, g, b)) => image::Rgba([r, g, b, 255]),
        None => image::Rgba([0, 0, 0, 0]),
    };
    let mut canvas = image::RgbaImage::from_pixel(box_w, box_h, fill);
    let x = (box_w - fitted.width()) / 2;
    let y = match fit.valign {
        VAlign::Center => (box_h - fitted.height()) / 2,
        VAlign::Bottom => box_h - fitted.height(),
    };
    image::imageops::overlay(&mut canvas, &fitted, x as i64, y as i64);
    image::DynamicImage::ImageRgba8(canvas)
}

/// Fade an image the way Dolphin shows cut items: halfway to `bg` when
/// it's composited onto one, else half transparent.
pub fn dim_image(img: &mut image::DynamicImage, bg: Option<(u8, u8, u8)>) {
    if let image::DynamicImage::ImageRgba8(buf) = img {
        for p in buf.pixels_mut() {
            match bg {
                Some((r, g, b)) => {
                    for (c, t) in p.0.iter_mut().zip([r, g, b]) {
                        *c = ((*c as u16 + t as u16) / 2) as u8;
                    }
                }
                None => p.0[3] /= 2,
            }
        }
    }
}

/// Protocol cache key: (path, box cols, box rows, bg color variant, dimmed).
type ProtoKey = (PathBuf, u16, u16, Option<(u8, u8, u8)>, bool);

pub struct IconCache {
    /// Raw rasterized icons keyed by path; protocols are rebuilt from these.
    /// `None` = resolved but no icon found (don't re-request).
    images: LruCache<PathBuf, Option<Arc<image::DynamicImage>>>,
    /// Resolved mime types; independent of icon size, so survives `clear`.
    mimes: LruCache<PathBuf, Option<String>>,
    protocols: LruCache<ProtoKey, StatefulProtocol>,
    tx_work: mpsc::SyncSender<IconRequest>,
    /// Low priority: icons for folders not open yet (see `prefetch`).
    tx_prefetch: mpsc::SyncSender<IconRequest>,
    rx_image: mpsc::Receiver<IconImageReady>,
    /// Paths for which a request is already in-flight
    pending: HashSet<PathBuf>,
    /// Queued as prefetches: a real request still goes ahead of them.
    prefetch_pending: HashSet<PathBuf>,
    /// Paths whose cached image is a folder preview.
    previews: HashSet<PathBuf>,
    fit: Fit,
}

impl IconCache {
    /// `folder_previews`: follow each folder icon with a version showing
    /// previews of its media (see `folder_preview`).
    pub fn new(folder_previews: bool) -> Self {
        theme::prewarm();
        let (tx_work, rx_work) = mpsc::sync_channel::<IconRequest>(128);
        let (tx_prefetch, rx_prefetch) = mpsc::sync_channel::<IconRequest>(1024);
        let (tx_image, rx_image) = mpsc::channel::<IconImageReady>();

        // Visible icons first; prefetches only when none are waiting, and a
        // couple at a time so they never hold up what's on screen
        let prefetching = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        struct Done(Arc<std::sync::atomic::AtomicUsize>);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
            loop {
                let next_prefetch = || (prefetching.load(Ordering::Relaxed) < 2)
                    .then(|| rx_prefetch.try_recv().ok())
                    .flatten();
                let (req, prefetch) = match rx_work.try_recv() {
                    Ok(r) => (r, false),
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => match next_prefetch() {
                        Some(r) => (r, true),
                        None => match rx_work.recv_timeout(std::time::Duration::from_millis(10)) {
                            Ok(r) => (r, false),
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => break,
                        },
                    },
                };
                let done = prefetch.then(|| {
                    prefetching.fetch_add(1, Ordering::Relaxed);
                    Done(prefetching.clone())
                });
                let tx = tx_image.clone();
                rayon::spawn(move || {
                    let _done = done;
                    let (mut icon_names, mime) = resolver::resolve_names_and_mime(&req.path, req.is_dir);
                    if let Some(name) = req.icon_override {
                        icon_names.insert(0, name);
                    }
                    let resolved = resolver::resolve_icon_path(&icon_names, req.is_dir);
                    let is_link = req.path.symlink_metadata().is_ok_and(|m| m.is_symlink());
                    let mut image = resolved.and_then(|(icon_path, is_svg)| {
                        raster::load_icon_shared(&icon_path, is_svg, req.target_px)
                            .map_err(|e| { tracing::warn!("load_icon {:?}: {e}", icon_path); e })
                            .ok()
                    });
                    if is_link && let Some(img) = &mut image {
                        add_link_emblem(Arc::make_mut(img));
                    }
                    let wants_preview = folder_previews && req.is_dir && req.target_px >= folder_preview::MIN_PX;
                    let Some(base) = image.clone().filter(|_| wants_preview) else {
                        // Always send back (even if no icon) so mime_type reaches the listing
                        let _ = tx.send(IconImageReady { path: req.path, image, mime_type: mime, preview: false });
                        return;
                    };

                    // A preview that's quick (cached thumbnails) replaces the plain
                    // folder outright, so it doesn't visibly swap; a slow one follows it
                    let (ptx, prx) = std::sync::mpsc::channel();
                    let dir = req.path.clone();
                    // Own thread: blocking a pool worker on a pool task could deadlock
                    std::thread::spawn(move || {
                        let _ = ptx.send(folder_preview::build(&dir, &base).map(|mut p| {
                            if is_link {
                                add_link_emblem(&mut p);
                            }
                            Arc::new(p)
                        }));
                    });
                    let send = |image, preview| { let _ = tx.send(IconImageReady { path: req.path.clone(), image, mime_type: mime.clone(), preview }); };
                    match prx.recv_timeout(std::time::Duration::from_millis(150)) {
                        Ok(Some(preview)) => send(Some(preview), true),
                        Ok(None) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => send(image, false),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            send(image, false);
                            if let Ok(Some(preview)) = prx.recv() {
                                send(Some(preview), true);
                            }
                        }
                    }
                });
            }
        });

        Self {
            // Plain icons are shared rasters, so many entries cost little
            images: LruCache::new(NonZeroUsize::new(5000).unwrap()),
            protocols: LruCache::new(NonZeroUsize::new(500).unwrap()),
            mimes: LruCache::new(NonZeroUsize::new(20_000).unwrap()),
            tx_work,
            tx_prefetch,
            rx_image,
            pending: HashSet::new(),
            prefetch_pending: HashSet::new(),
            previews: HashSet::new(),
            fit: GRID_FIT,
        }
    }

    /// Use a different placement for this cache's images.
    pub fn with_fit(mut self, fit: Fit) -> Self {
        self.fit = fit;
        self
    }

    /// Queue an icon load if not already loaded or pending.
    pub fn request(&mut self, path: &Path, is_dir: bool, target_px: u32, icon_override: Option<String>) {
        if self.images.contains(path) || self.pending.contains(path) {
            return;
        }
        let req = IconRequest { path: path.to_path_buf(), is_dir, target_px, icon_override };
        // Only mark pending if queued; a full queue is retried next frame
        if self.tx_work.try_send(req).is_ok() {
            self.pending.insert(path.to_path_buf());
        }
    }

    /// Like `request`, but only worked on while no visible icon waits:
    /// warms the cache for folders that may be opened next.
    pub fn prefetch(&mut self, path: &Path, is_dir: bool, target_px: u32, icon_override: Option<String>) {
        if self.images.contains(path) || self.pending.contains(path) || self.prefetch_pending.contains(path) {
            return;
        }
        let req = IconRequest { path: path.to_path_buf(), is_dir, target_px, icon_override };
        if self.tx_prefetch.try_send(req).is_ok() {
            self.prefetch_pending.insert(path.to_path_buf());
        }
    }

    /// Whether requested icons are still loading (prefetches aside).
    pub fn loading(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Mime type from a previous resolution of this path, if any.
    pub fn cached_mime(&self, path: &Path) -> Option<&Option<String>> {
        self.mimes.peek(path)
    }

    /// Drain completed icon loads; returns mime type updates for each path.
    pub fn drain_ready(&mut self) -> Vec<(PathBuf, Option<String>)> {
        let mut mime_updates = Vec::new();
        while let Ok(ready) = self.rx_image.try_recv() {
            self.pending.remove(&ready.path);
            self.prefetch_pending.remove(&ready.path);
            // A prefetch and a request for one folder can both finish: keep
            // the preview over the plain icon that may arrive after it
            if !ready.preview && self.previews.contains(&ready.path) && self.images.contains(&ready.path) {
                continue;
            }
            if ready.preview {
                self.previews.insert(ready.path.clone());
            }
            // An upgrade (folder preview) replaces an icon already on screen
            if self.images.contains(&ready.path) {
                self.invalidate(&ready.path);
            }
            self.images.put(ready.path.clone(), ready.image);
            self.mimes.put(ready.path.clone(), ready.mime_type.clone());
            mime_updates.push((ready.path, ready.mime_type));
        }
        mime_updates
    }

    /// Get (building lazily from the raw image) a protocol for rendering.
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
            let img = self.images.get(path)?.as_ref()?;
            let mut img = fit_to_cells(img, picker, cols, rows, bg, self.fit);
            if dim {
                dim_image(&mut img, bg);
            }
            self.protocols.put(key.clone(), picker.new_resize_protocol(img));
        }
        self.protocols.get_mut(&key)
    }

    /// The loaded image for a path, if any.
    pub fn image(&self, path: &Path) -> Option<&image::DynamicImage> {
        self.images.peek(path)?.as_deref()
    }

    /// Drop all protocol variants for a path (raw image stays cached).
    /// Used when a Kitty placement was deleted and must be re-transmitted.
    pub fn invalidate(&mut self, path: &Path) {
        let keys: Vec<ProtoKey> = self.protocols.iter()
            .filter(|(k, _)| k.0 == path)
            .map(|(k, _)| k.clone())
            .collect();
        for k in keys {
            self.protocols.pop(&k);
        }
    }

    /// Clear all cached protocols and raw images (e.g. on icon size change).
    pub fn clear(&mut self) {
        self.images.clear();
        self.protocols.clear();
        self.pending.clear();
        self.prefetch_pending.clear();
        self.previews.clear();
    }

    /// Clear only encoded protocols, keeping raw images (e.g. terminal resize).
    pub fn clear_protocols(&mut self) {
        self.protocols.clear();
    }
}



