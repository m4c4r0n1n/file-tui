use std::{path::{Path, PathBuf}, process::{Command, Stdio}};

use image::DynamicImage;

/// Load an icon image, rasterizing SVG via rsvg-convert if needed.
/// Rasterized SVGs are cached persistently so each icon converts once per size.
/// Rasterized theme icons by (file, size): files of one type share an icon,
/// so a folder full of PDFs rasterizes the PDF icon once.
type SharedIcons = lru::LruCache<(std::path::PathBuf, u32), std::sync::Arc<DynamicImage>>;
static SHARED: std::sync::Mutex<Option<SharedIcons>> = std::sync::Mutex::new(None);

pub fn load_icon_shared(path: &Path, is_svg: bool, target_px: u32) -> anyhow::Result<std::sync::Arc<DynamicImage>> {
    let key = (path.to_path_buf(), target_px);
    if let Some(img) = SHARED.lock().ok().and_then(|mut c| c.as_mut()?.get(&key).cloned()) {
        return Ok(img);
    }
    let img = std::sync::Arc::new(load_icon(path, is_svg, target_px)?);
    if let Ok(mut c) = SHARED.lock() {
        c.get_or_insert_with(|| lru::LruCache::new(std::num::NonZeroUsize::new(400).unwrap()))
            .put(key, img.clone());
    }
    Ok(img)
}

pub fn load_icon(path: &Path, is_svg: bool, target_px: u32) -> anyhow::Result<DynamicImage> {
    if !is_svg {
        let img = image::open(path)?;
        return Ok(img.resize(target_px, target_px, image::imageops::FilterType::Lanczos3));
    }

    let cached = raster_cache_path(path, target_px);
    if cached.exists() {
        return Ok(image::open(&cached)?);
    }
    if let Some(parent) = cached.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = cached.with_extension(format!("{:?}.tmp.png", std::thread::current().id()));
    rasterize_svg(path, target_px, &tmp)?;
    std::fs::rename(&tmp, &cached)?;
    Ok(image::open(&cached)?)
}

/// Render an SVG to a `px`-square PNG at `out` with rsvg-convert.
pub fn rasterize_svg(path: &Path, px: u32, out: &Path) -> anyhow::Result<()> {
    let status = Command::new("rsvg-convert")
        .args(["-w", &px.to_string(), "-h", &px.to_string(), "-o"])
        .arg(out)
        .arg(path)
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        let _ = std::fs::remove_file(out);
        anyhow::bail!("rsvg-convert failed for {:?}", path);
    }
    Ok(())
}

fn raster_cache_path(svg_path: &Path, target_px: u32) -> PathBuf {
    let stem = svg_path.file_stem().and_then(|s| s.to_str()).unwrap_or("icon");
    let hash = hash_path(svg_path);
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("file-tui/icons")
        .join(format!("{stem}-{target_px}-{hash:016x}.png"))
}

fn hash_path(path: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}
