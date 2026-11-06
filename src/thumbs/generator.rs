use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use image::DynamicImage;

/// Video frame via ffmpegthumbnailer, falling back to plain ffmpeg when it
/// isn't installed.
pub fn video_thumb(source: &Path, tmp: &Path) -> anyhow::Result<DynamicImage> {
    match ffmpegthumbnailer(source, tmp) {
        Err(e) if is_not_found(&e) => ffmpeg(source, tmp)?,
        r => r?,
    }
    let img = image::open(tmp);
    let _ = std::fs::remove_file(tmp);
    let mut img = img?.to_rgba8();
    filmstrip(&mut img);
    Ok(DynamicImage::ImageRgba8(img))
}

/// Dolphin's film-strip edges on a video frame: a dark band down each side
/// with sprocket holes. Drawn into the thumbnail, as Dolphin does, so the
/// shared cache looks the same in both.
fn filmstrip(img: &mut image::RgbaImage) {
    let (w, h) = img.dimensions();
    let band = (w.min(h) / 14).max(4);
    if w < band * 4 {
        return;
    }
    let (hole_w, hole_h) = ((band as f32 * 0.5).round().max(2.0) as u32, (band as f32 * 0.6).round().max(2.0) as u32);
    let pitch = (hole_h as f32 * 1.8).round() as u32;
    let film = image::Rgba([24, 24, 24, 255]);
    let hole = image::Rgba([226, 226, 222, 255]);
    let first = (h % pitch + pitch - hole_h) / 2;
    for x0 in [0, w - band] {
        for y in 0..h {
            for x in x0..x0 + band {
                img.put_pixel(x, y, film);
            }
        }
        let hx = x0 + (band - hole_w) / 2;
        let mut y = first;
        while y + hole_h <= h {
            for dy in 0..hole_h {
                for dx in 0..hole_w {
                    // Corners left off: slightly rounded holes
                    let corner = (dx == 0 || dx == hole_w - 1) && (dy == 0 || dy == hole_h - 1);
                    if !corner || hole_w < 3 {
                        img.put_pixel(hx + dx, y + dy, hole);
                    }
                }
            }
            y += pitch;
        }
    }
}

fn ffmpegthumbnailer(source: &Path, out: &Path) -> anyhow::Result<()> {
    let status = Command::new("ffmpegthumbnailer")
        .arg("-i").arg(source)
        .arg("-o").arg(out)
        .args(["-s", "256", "-q", "8", "-c", "png"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status()?;
    if !status.success() {
        anyhow::bail!("ffmpegthumbnailer failed for {:?}", source);
    }
    Ok(())
}

fn ffmpeg(source: &Path, out: &Path) -> anyhow::Result<()> {
    // Seek ~15% in to skip intros/black leaders and take the keyframe
    // there: decoding just one frame, on one thread, as several run at once
    let seek = media_duration(source).or_else(|| duration_secs(source)).map(|d| d * 0.15).unwrap_or(0.0);
    let run = |seek: f64| {
        Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-y", "-threads", "1", "-skip_frame", "nokey"])
            .args(["-ss", &format!("{seek:.2}"), "-i"])
            .arg(source)
            .args([
                "-an", "-sn", "-frames:v", "1",
                "-vf", "scale=256:256:force_original_aspect_ratio=decrease",
                "-f", "image2", "-c:v", "png",
            ])
            .arg(out)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status()
    };
    let mut ok = run(seek)?.success() && out.exists();
    // Seeking past the end of very short clips yields no frame; retry from 0
    if !ok && seek > 0.0 {
        ok = run(0.0)?.success() && out.exists();
    }
    if !ok {
        anyhow::bail!("ffmpeg failed for {:?}", source);
    }
    Ok(())
}

fn duration_secs(source: &Path) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(source)
        .stdin(Stdio::null()).stderr(Stdio::null())
        .output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Length in seconds read from the container itself (MP4/MOV, Matroska/
/// WebM), sparing an ffprobe start, which costs more than the reading.
pub fn media_duration(source: &Path) -> Option<f64> {
    let mut f = std::fs::File::open(source).ok()?;
    let mut magic = [0u8; 8];
    std::io::Read::read_exact(&mut f, &mut magic).ok()?;
    if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        matroska_duration(&mut f)
    } else if &magic[4..8] == b"ftyp" {
        mp4_duration(&mut f)
    } else {
        None
    }
}

/// From the `mvhd` box inside `moov` (which may sit at the end of the file).
fn mp4_duration(f: &mut std::fs::File) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    let len = f.metadata().ok()?.len();
    // Boxes: 4-byte size (1 = 64-bit size follows, 0 = to the end), 4-byte type
    let next_box = |f: &mut std::fs::File, pos: u64, end: u64| -> Option<(u64, u64, [u8; 4])> {
        if pos + 8 > end {
            return None;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h).ok()?;
        let kind = [h[4], h[5], h[6], h[7]];
        let (size, header) = match u32::from_be_bytes([h[0], h[1], h[2], h[3]]) {
            1 => {
                let mut b = [0u8; 8];
                f.read_exact(&mut b).ok()?;
                (u64::from_be_bytes(b), 16)
            }
            0 => (end - pos, 8),
            n => (n as u64, 8),
        };
        (size >= header).then_some((pos + header, pos + size, kind))
    };
    let mut pos = 0;
    let (moov, moov_end) = loop {
        let (body, end, kind) = next_box(f, pos, len)?;
        if &kind == b"moov" {
            break (body, end.min(len));
        }
        pos = end;
    };
    // The header's duration; fragmented files (DASH) leave it 0 and give
    // it in mvex/mehd instead, in the same timescale
    let mut scale = 0;
    let mut pos = moov;
    while let Some((body, end, kind)) = next_box(f, pos, moov_end) {
        let mut b = [0u8; 32];
        let u32_at = |b: &[u8; 32], i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as u64;
        if &kind == b"mvhd" {
            f.seek(SeekFrom::Start(body)).ok()?;
            f.read_exact(&mut b).ok()?;
            let dur;
            (scale, dur) = if b[0] == 1 {
                (u32_at(&b, 20), u32_at(&b, 24) << 32 | u32_at(&b, 28))
            } else {
                (u32_at(&b, 12), u32_at(&b, 16))
            };
            if scale > 0 && dur > 0 {
                return Some(dur as f64 / scale as f64);
            }
        } else if &kind == b"mvex" && scale > 0 {
            let mut p = body;
            while let Some((mbody, mend, mkind)) = next_box(f, p, end) {
                if &mkind == b"mehd" {
                    f.seek(SeekFrom::Start(mbody)).ok()?;
                    let mut b = [0u8; 32];
                    let n = f.read(&mut b).ok()?;
                    let dur = if b[0] == 1 && n >= 12 { u32_at(&b, 4) << 32 | u32_at(&b, 8) } else { u32_at(&b, 4) };
                    return (dur > 0).then(|| dur as f64 / scale as f64);
                }
                p = mend;
            }
        }
        pos = end;
    }
    None
}

/// From the Segment's Info element (Duration × TimecodeScale), which comes
/// before the clusters in the first part of the file.
fn matroska_duration(f: &mut std::fs::File) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(0)).ok()?;
    let mut buf = Vec::new();
    f.take(512 << 10).read_to_end(&mut buf).ok()?;
    // EBML variable-length integers: the leading zero bits give the length;
    // IDs keep the marker bit, sizes drop it (all ones = unknown size)
    let vint = |pos: usize, keep_marker: bool| -> Option<(u64, usize, bool)> {
        let first = *buf.get(pos)?;
        let n = first.leading_zeros() as usize + 1;
        if n > 8 {
            return None;
        }
        let mask = 0xFFu64 >> n;
        let mut v = if keep_marker { first as u64 } else { first as u64 & mask };
        let mut all_ones = v == mask;
        for i in 1..n {
            let b = *buf.get(pos + i)?;
            all_ones &= b == 0xFF;
            v = v << 8 | b as u64;
        }
        Some((v, n, !keep_marker && all_ones))
    };
    let element = |pos: usize| -> Option<(u64, usize, Option<usize>)> {
        let (id, idn, _) = vint(pos, true)?;
        let (size, sn, unknown) = vint(pos + idn, false)?;
        Some((id, pos + idn + sn, (!unknown).then_some(size as usize)))
    };
    const SEGMENT: u64 = 0x18538067;
    const INFO: u64 = 0x1549A966;
    const CLUSTER: u64 = 0x1F43B675;
    let mut pos = 0;
    // Top level: skip the EBML header, enter the Segment
    loop {
        let (id, body, size) = element(pos)?;
        if id == SEGMENT {
            pos = body;
            break;
        }
        pos = body + size?;
    }
    loop {
        let (id, body, size) = element(pos)?;
        if id == CLUSTER {
            return None;
        }
        let size = size?;
        if id == INFO {
            let (mut scale, mut duration) = (1_000_000u64, None);
            let mut p = body;
            while p < body + size {
                let (id, b, s) = element(p)?;
                let s = s?;
                let data = buf.get(b..b + s)?;
                match id {
                    0x2AD7B1 => scale = data.iter().fold(0, |v, &x| v << 8 | x as u64),
                    0x4489 if s == 4 => duration = Some(f32::from_be_bytes(data.try_into().ok()?) as f64),
                    0x4489 if s == 8 => duration = Some(f64::from_be_bytes(data.try_into().ok()?)),
                    _ => {}
                }
                p = b + s;
            }
            return duration.map(|d| d * scale as f64 / 1e9).filter(|d| *d > 0.0);
        }
        pos = body + size;
    }
}

fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == ErrorKind::NotFound)
}

/// Image thumbnail in-process with the image crate (no magick spawn).
pub fn image_thumb(source: &Path) -> anyhow::Result<DynamicImage> {
    Ok(image::open(source)?.thumbnail(256, 256))
}

/// EPUB cover, read straight from the archive: the OPF's declared cover
/// image (EPUB 3 `cover-image` property or EPUB 2 `<meta name="cover">`),
/// else an image named like a cover.
pub fn epub_thumb(source: &Path) -> anyhow::Result<DynamicImage> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(source)?)?;
    let read = |zip: &mut zip::ZipArchive<std::fs::File>, name: &str| -> anyhow::Result<Vec<u8>> {
        let mut buf = Vec::new();
        zip.by_name(name)?.take(64 << 20).read_to_end(&mut buf)?;
        Ok(buf)
    };

    let container = String::from_utf8_lossy(&read(&mut zip, "META-INF/container.xml")?).into_owned();
    let opf_path = tags(&container, "rootfile").into_iter()
        .find_map(|t| attr(&t, "full-path"))
        .ok_or_else(|| anyhow::anyhow!("no rootfile"))?;
    let opf = String::from_utf8_lossy(&read(&mut zip, &opf_path)?).into_owned();
    let base = opf_path.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));

    let items = tags(&opf, "item");
    let is_image = |t: &str| attr(t, "media-type").is_some_and(|m| m.starts_with("image/"));
    let cover_id = tags(&opf, "meta").into_iter()
        .find(|t| attr(t, "name").as_deref() == Some("cover"))
        .and_then(|t| attr(&t, "content"));
    let href = items.iter()
        .find(|t| attr(t, "properties").is_some_and(|p| p.split_whitespace().any(|p| p == "cover-image")))
        .or_else(|| items.iter().find(|t| cover_id.is_some() && attr(t, "id") == cover_id && is_image(t)))
        .or_else(|| items.iter().find(|t| is_image(t)
            && [attr(t, "id"), attr(t, "href")].iter().flatten().any(|v| v.to_lowercase().contains("cover"))))
        .and_then(|t| attr(t, "href"))
        .ok_or_else(|| anyhow::anyhow!("no cover"))?;

    let name = resolve_href(&base, &percent_decode(&href));
    Ok(image::load_from_memory(&read(&mut zip, &name)?)?.thumbnail(256, 256))
}

/// The opening tags named `name` (with or without a namespace prefix).
fn tags(xml: &str, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        let tag_name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        if tag_name == name || tag_name.rsplit(':').next() == Some(name) {
            out.push(tag.to_string());
        }
        rest = &rest[end..];
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let before_ok = i == 0 || rest[..i].ends_with(|c: char| c.is_whitespace());
        let after = rest[i + name.len()..].trim_start();
        if before_ok && let Some(v) = after.strip_prefix('=') {
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let v = &v[1..];
                return Some(v[..v.find(q)?].replace("&amp;", "&"));
            }
        }
        rest = &rest[i + name.len()..];
    }
    None
}

/// Join an OPF-relative href onto the OPF's directory, folding "..".
fn resolve_href(base: &str, href: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => { parts.pop(); }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Embedded album art via ffmpeg.
pub fn audio_thumb(source: &Path, tmp: &Path) -> anyhow::Result<DynamicImage> {
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-i"])
        .arg(source)
        .args([
            "-an", "-map", "0:v:0", "-frames:v", "1",
            "-vf", "scale=256:256:force_original_aspect_ratio=decrease",
            "-f", "image2", "-c:v", "png",
        ])
        .arg(tmp)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status()?;
    let img = (status.success() && tmp.exists()).then(|| image::open(tmp));
    let _ = std::fs::remove_file(tmp);
    match img {
        Some(img) => Ok(img?),
        None => anyhow::bail!("no album art in {:?}", source),
    }
}

/// The preview OpenDocument files carry at Thumbnails/thumbnail.png.
pub fn opendocument_thumb(source: &Path) -> anyhow::Result<DynamicImage> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(source)?)?;
    let mut buf = Vec::new();
    zip.by_name("Thumbnails/thumbnail.png")?.take(16 << 20).read_to_end(&mut buf)?;
    Ok(image::load_from_memory(&buf)?.thumbnail(256, 256))
}

/// First page (by name) of a CBZ comic.
pub fn comic_thumb(source: &Path) -> anyhow::Result<DynamicImage> {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(source)?)?;
    let mut pages: Vec<String> = zip.file_names()
        .filter_map(|n| n.ok().map(|n| n.into_owned()))
        .filter(|n| {
            let n = n.to_lowercase();
            [".jpg", ".jpeg", ".png", ".webp", ".gif"].iter().any(|e| n.ends_with(e)) && !n.starts_with("__macosx")
        })
        .collect();
    pages.sort();
    let first = pages.first().ok_or_else(|| anyhow::anyhow!("no pages"))?;
    let mut buf = Vec::new();
    zip.by_name(first)?.take(64 << 20).read_to_end(&mut buf)?;
    Ok(image::load_from_memory(&buf)?.thumbnail(256, 256))
}

/// First page of a PDF via poppler's pdftoppm.
pub fn pdf_thumb(source: &Path, tmp: &Path) -> anyhow::Result<DynamicImage> {
    // pdftoppm appends ".png" to the output prefix
    let prefix = tmp.with_extension("");
    let status = Command::new("pdftoppm")
        .args(["-png", "-f", "1", "-l", "1", "-singlefile", "-scale-to", "256"])
        .arg(source).arg(&prefix)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status()?;
    let out = PathBuf::from(format!("{}.png", prefix.display()));
    let img = status.success().then(|| image::open(&out));
    let _ = std::fs::remove_file(&out);
    match img {
        Some(img) => Ok(img?),
        None => anyhow::bail!("pdftoppm failed for {:?}", source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opf_parsing() {
        let opf = r#"<package><manifest>
            <opf:item id="c" href="images/My%20Cover.jpg" media-type="image/jpeg"/>
            <item id="x" href="text/a.xhtml" media-type="application/xhtml+xml" properties="nav"/>
            </manifest><metadata><meta name="cover" content="c"/></metadata></package>"#;
        let items = tags(opf, "item");
        assert_eq!(items.len(), 2);
        assert_eq!(attr(&items[0], "id").as_deref(), Some("c"));
        assert_eq!(attr(&items[1], "properties").as_deref(), Some("nav"));
        assert_eq!(attr(&tags(opf, "meta")[0], "content").as_deref(), Some("c"));
        assert_eq!(resolve_href("OEBPS/", &percent_decode("../images/My%20Cover.jpg")), "images/My Cover.jpg");
    }
}



