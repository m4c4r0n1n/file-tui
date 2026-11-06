//! Dolphin-style folder previews: up to four of the folder's previewable
//! files as thin-bordered prints over the folder icon (one or two large,
//! more in a 2x2 grid).

use std::path::{Path, PathBuf};

use image::{DynamicImage, Rgba, RgbaImage, imageops};

/// Below this the prints are too small to read.
pub const MIN_PX: u32 = 48;
const MAX_PRINTS: usize = 4;
/// Entries scanned per folder looking for media.
const SCAN_LIMIT: usize = 500;
const PAPER: Rgba<u8> = Rgba([250, 250, 247, 255]);

/// Area over the folder body, as fractions of the icon: left, top,
/// right, bottom, and the gap between tiles.
const GRID: (f32, f32, f32, f32) = (0.15, 0.31, 0.85, 0.86);
const GAP: f32 = 0.035;
/// Slight alternating tilt per quadrant (TL, TR, BL, BR), in degrees.
const TILT: [f32; 4] = [-2.5, 2.0, 1.5, -2.0];

/// Compose previews of `dir`'s media onto `folder`, or None if it has none.
pub fn build(dir: &Path, folder: &DynamicImage) -> Option<DynamicImage> {
    let photos: Vec<DynamicImage> = media_in(dir)
        .into_iter()
        .filter_map(|(path, mime)| crate::thumbs::generate_or_load_thumb(&path, &mime).ok())
        .take(MAX_PRINTS)
        .collect();
    if photos.is_empty() {
        return None;
    }

    let mut canvas = folder.to_rgba8();
    let (w, h) = (canvas.width() as f32, canvas.height() as f32);
    let (l, t, r, b) = GRID;
    let (gw, gh, gap) = ((r - l) * w, (b - t) * h, GAP * w);

    // One or two get larger slots, shaped like the item (a book cover stays
    // whole); three or four share the 2x2 grid, cropped to fill
    let n = photos.len();
    let slots: Vec<(f32, f32, f32, f32)> = match n {
        1 => vec![(l * w + gw / 2.0, t * h + gh / 2.0, gw, gh)],
        2 => {
            let sw = (gw - gap) / 2.0;
            (0..2).map(|i| (l * w + sw / 2.0 + i as f32 * (sw + gap), t * h + gh / 2.0, sw, gh)).collect()
        }
        _ => {
            let (sw, sh) = ((gw - gap) / 2.0, (gh - GAP * h) / 2.0);
            (0..4).map(|i| {
                let (col, row) = ((i % 2) as f32, (i / 2) as f32);
                (l * w + sw / 2.0 + col * (sw + gap), t * h + sh / 2.0 + row * (sh + GAP * h), sw, sh)
            }).collect()
        }
    };
    let tilt: &[f32] = match n {
        1 => &[-1.5],
        2 => &[-2.5, 2.0],
        _ => &TILT,
    };

    for (i, (photo, &(cx, cy, sw, sh))) in photos.iter().zip(&slots).enumerate() {
        let (tw, th) = if n <= 2 {
            let aspect = photo.width().max(1) as f32 / photo.height().max(1) as f32;
            if aspect > sw / sh { (sw, sw / aspect) } else { (sh * aspect, sh) }
        } else {
            (sw, sh)
        };
        let print = rotate(&make_print(photo, tw.max(4.0) as u32, th.max(4.0) as u32), tilt[i]);
        let shadow = shadow_of(&print, w.min(h));
        let x = cx as i64 - print.width() as i64 / 2;
        let y = cy as i64 - print.height() as i64 / 2;
        let off = (w.min(h) * 0.01).max(1.0) as i64;
        let pad = (shadow.width() as i64 - print.width() as i64) / 2;
        imageops::overlay(&mut canvas, &shadow, x - pad, y - pad + off);
        imageops::overlay(&mut canvas, &print, x, y);
    }
    Some(DynamicImage::ImageRgba8(canvas))
}

/// First few images/videos in `dir` by name (hidden files skipped).
fn media_in(dir: &Path) -> Vec<(PathBuf, String)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new(); };
    let mut media: Vec<(PathBuf, String)> = rd
        .take(SCAN_LIMIT)
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let mime = crate::mime::guess_by_name(&e.path())?;
            // Whatever gets a content preview of its own (book covers too)
            let usable = match crate::thumbs::previews() {
                Some(p) => p.covers(&mime),
                None => mime.starts_with("image/") || mime.starts_with("video/"),
            };
            usable.then(|| (e.path(), mime))
        })
        .collect();
    media.sort_by(|a, b| a.0.cmp(&b.0));
    // A few spares in case some thumbnails fail
    media.truncate(MAX_PRINTS * 2);
    media
}

/// The photo center-cropped to fill a `w`×`h` tile, inside a thin paper border.
fn make_print(photo: &DynamicImage, w: u32, h: u32) -> RgbaImage {
    let border = ((w.min(h) as f32) * 0.035).round().max(1.0) as u32;
    let (iw, ih) = (w.saturating_sub(border * 2).max(1), h.saturating_sub(border * 2).max(1));
    let photo = photo.resize_to_fill(iw, ih, imageops::FilterType::Triangle).to_rgba8();
    let mut print = RgbaImage::from_pixel(iw + border * 2, ih + border * 2, PAPER);
    imageops::overlay(&mut print, &photo, border as i64, border as i64);
    print
}

/// Soft drop shadow matching the print's (rotated) silhouette, padded so
/// the blur has room; centered on the print.
fn shadow_of(print: &RgbaImage, icon_size: f32) -> RgbaImage {
    let sigma = (icon_size * 0.012).max(0.8);
    let pad = (sigma * 2.0).ceil() as u32;
    let mut shadow = RgbaImage::new(print.width() + pad * 2, print.height() + pad * 2);
    for (x, y, p) in print.enumerate_pixels() {
        shadow.put_pixel(x + pad, y + pad, Rgba([0, 0, 0, (p[3] as f32 * 0.45) as u8]));
    }
    imageops::blur(&shadow, sigma)
}

/// Rotate by `deg` around the center into a canvas that fits the result,
/// with premultiplied bilinear sampling so edges antialias cleanly.
fn rotate(img: &RgbaImage, deg: f32) -> RgbaImage {
    let (sin, cos) = deg.to_radians().sin_cos();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let out_w = (w * cos.abs() + h * sin.abs()).ceil() as u32 + 2;
    let out_h = (w * sin.abs() + h * cos.abs()).ceil() as u32 + 2;
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ocx, ocy) = (out_w as f32 / 2.0, out_h as f32 / 2.0);

    let sample = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
            return [0.0; 4];
        }
        let p = img.get_pixel(x as u32, y as u32);
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, a]
    };

    RgbaImage::from_fn(out_w, out_h, |ox, oy| {
        // Inverse-map the output pixel center into the source
        let (dx, dy) = (ox as f32 + 0.5 - ocx, oy as f32 + 0.5 - ocy);
        let sx = dx * cos + dy * sin + cx - 0.5;
        let sy = -dx * sin + dy * cos + cy - 0.5;
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);

        let mut acc = [0.0f32; 4];
        for (px, py, wgt) in [
            (x0, y0, (1.0 - fx) * (1.0 - fy)),
            (x0 + 1, y0, fx * (1.0 - fy)),
            (x0, y0 + 1, (1.0 - fx) * fy),
            (x0 + 1, y0 + 1, fx * fy),
        ] {
            let s = sample(px, py);
            for i in 0..4 { acc[i] += s[i] * wgt; }
        }
        if acc[3] <= 0.0 {
            return Rgba([0, 0, 0, 0]);
        }
        let un = |c: f32| (c / acc[3]).round().clamp(0.0, 255.0) as u8;
        Rgba([un(acc[0]), un(acc[1]), un(acc[2]), (acc[3] * 255.0).round() as u8])
    })
}



