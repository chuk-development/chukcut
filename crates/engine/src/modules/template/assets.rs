//! The files a template's project points at that are ours: slot
//! placeholders and the music beds.
//!
//! Both are drawn or synthesised here, so there is nothing to ship and no
//! licence to track (`CLAUDE.md`, "Legal boundary"). They live under
//! `<data>/templates/`, not the cache: a project made from a template still
//! shows its empty slots and plays its music after "clear cache". Each file
//! name says how to draw it again, so [`ensure_for`] can recreate any of them
//! that went missing.

use std::path::{Path, PathBuf};

use crate::modules::project::document::Project;
use crate::modules::workspace::paths;

use super::music::{self, Bed};

/// Bump to redraw every placeholder.
const PLACEHOLDER_VERSION: u32 = 1;

/// Where a template's own files live.
pub fn root() -> PathBuf {
    paths::data_root().join("templates")
}

pub fn placeholders_dir() -> PathBuf {
    root().join("placeholders")
}

pub fn music_dir() -> PathBuf {
    root().join("music")
}

/// Where user templates are kept, one directory each.
pub fn user_dir() -> PathBuf {
    root().join("user")
}

/// The pixel size of a placeholder of shape `aspect`: 720 on the short edge,
/// enough to look clean in the preview, small enough to write in a blink.
pub fn placeholder_size(aspect: [u32; 2]) -> (u32, u32) {
    let (a, b) = (aspect[0].max(1) as f64, aspect[1].max(1) as f64);
    let short = 720.0;
    let (w, h) = if a >= b {
        (short * a / b, short)
    } else {
        (short, short * b / a)
    };
    let cap = |v: f64| (v.round().clamp(2.0, 2560.0) as u32) & !1;
    (cap(w), cap(h))
}

/// The file of the placeholder for slot `index` of shape `aspect`.
pub fn placeholder_path(index: u32, aspect: [u32; 2]) -> PathBuf {
    placeholders_dir().join(format!(
        "slot-{index}-{}x{}-v{PLACEHOLDER_VERSION}.png",
        aspect[0], aspect[1]
    ))
}

/// `slot-3-9x16-v1.png` → `(3, [9, 16])`.
fn parse_placeholder(name: &str) -> Option<(u32, [u32; 2])> {
    let rest = name.strip_prefix("slot-")?;
    let rest = rest.strip_suffix(&format!("-v{PLACEHOLDER_VERSION}.png"))?;
    let (index, shape) = rest.split_once('-')?;
    let (w, h) = shape.split_once('x')?;
    Some((index.parse().ok()?, [w.parse().ok()?, h.parse().ok()?]))
}

/// The placeholder for slot `index`, drawn if it is not on disk.
pub fn ensure_placeholder(index: u32, aspect: [u32; 2]) -> Result<PathBuf, String> {
    let path = placeholder_path(index, aspect);
    if path.exists() {
        return Ok(path);
    }
    let (w, h) = placeholder_size(aspect);
    write_png(&path, &draw_placeholder(index, w, h))?;
    Ok(path)
}

/// Draw every placeholder and music bed `project` names that is missing.
/// Cheap when they are all there: one `stat` per file.
pub fn ensure_for(project: &Project) -> Result<(), String> {
    let pool = &project.materials;
    for image in &pool.images {
        let path = Path::new(&image.path);
        if path.exists() || path.parent() != Some(placeholders_dir().as_path()) {
            continue;
        }
        if let Some((index, aspect)) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(parse_placeholder)
        {
            ensure_placeholder(index, aspect)?;
        }
    }
    for audio in &pool.audios {
        let path = Path::new(&audio.path);
        if path.exists() || path.parent() != Some(music_dir().as_path()) {
            continue;
        }
        if let Some((bed, seconds)) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(music::parse_file_name)
        {
            ensure_music(bed, seconds)?;
        }
    }
    Ok(())
}

/// The music bed `bed`, `seconds` long, synthesised if it is not on disk.
pub fn ensure_music(bed: Bed, seconds: u32) -> Result<PathBuf, String> {
    let path = music_dir().join(music::file_name(bed, seconds));
    if path.exists() {
        return Ok(path);
    }
    let samples = music::render(bed, seconds);
    write_atomically(&path, &music::wav_bytes(&samples))?;
    Ok(path)
}

fn write_png(path: &Path, image: &image::RgbaImage) -> Result<(), String> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("encoding {} failed: {e}", path.display()))?;
    write_atomically(path, &bytes)
}

/// Written beside and renamed, so nobody reads half a file. The partial
/// file's name is unique per write: two threads drawing the same missing
/// placeholder at once (a preview tile and a new project) must not rename
/// each other's file away.
pub(super) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITES: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let partial = path.with_extension(format!(
        "{}.{}-{}.part",
        path.extension().and_then(|e| e.to_str()).unwrap_or("tmp"),
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&partial, bytes)
        .map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, path).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("cannot write {}: {e}", path.display())
    })
}

// ---------------------------------------------------------------------------
// Drawing a placeholder
// ---------------------------------------------------------------------------

/// 5×7 digits, one row per byte, high bit on the left.
const DIGITS: [[u8; 7]; 10] = [
    [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
    [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
    [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
    [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
    [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
    [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
    [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
    [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
    [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
    [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
];

/// The design language's graphite and aqua (`docs/design/language.md`), as
/// sRGB bytes: the placeholder is a picture, not chrome, but it should look
/// like it belongs to the app that made it.
const GROUND: [u8; 3] = [0x1a, 0x1b, 0x1f];
const STRIPE: [u8; 3] = [0x22, 0x24, 0x29];
const ACCENT: [u8; 3] = [0x2f, 0xd5, 0xc8];

/// A dark field with soft diagonal stripes, an aqua frame inset from the
/// edge, and the slot's number large in the middle: unmistakably "your clip
/// goes here", and the number matches the fill dialog's list.
pub fn draw_placeholder(index: u32, width: u32, height: u32) -> image::RgbaImage {
    let (w, h) = (width.max(2), height.max(2));
    let short = w.min(h) as f32;
    let stripe = (short / 14.0).max(4.0);
    let inset = (short * 0.06).round() as i64;
    let line = (short / 180.0).max(2.0).round() as i64;
    let mut image = image::RgbaImage::new(w, h);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let band = (((x + y) as f32 / stripe) as u32).is_multiple_of(2);
        let c = if band { GROUND } else { STRIPE };
        *pixel = image::Rgba([c[0], c[1], c[2], 255]);
    }
    // The frame: a rectangle of `line` thickness, with gaps at the middle of
    // each side, the way a viewfinder's corners read.
    let (x0, y0, x1, y1) = (inset, inset, w as i64 - inset, h as i64 - inset);
    let corner = (short as i64 / 6).max(8);
    let mut put = |x: i64, y: i64, color: [u8; 3]| {
        if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
            image.put_pixel(
                x as u32,
                y as u32,
                image::Rgba([color[0], color[1], color[2], 255]),
            );
        }
    };
    for t in 0..line {
        for x in x0..x1 {
            if x - x0 < corner || x1 - x <= corner {
                put(x, y0 + t, ACCENT);
                put(x, y1 - 1 - t, ACCENT);
            }
        }
        for y in y0..y1 {
            if y - y0 < corner || y1 - y <= corner {
                put(x0 + t, y, ACCENT);
                put(x1 - 1 - t, y, ACCENT);
            }
        }
    }
    // The number.
    let text = index.to_string();
    let cell = (short / 40.0).max(2.0).round() as i64;
    let glyph_w = 5 * cell;
    let gap = cell * 2;
    let total = text.len() as i64 * glyph_w + (text.len() as i64 - 1) * gap;
    let left = (w as i64 - total) / 2;
    let top = (h as i64 - 7 * cell) / 2;
    for (n, digit) in text.bytes().enumerate() {
        let rows = DIGITS[(digit - b'0') as usize];
        let ox = left + n as i64 * (glyph_w + gap);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (0x10 >> col) == 0 {
                    continue;
                }
                for dy in 0..cell {
                    for dx in 0..cell {
                        put(ox + col * cell + dx, top + row as i64 * cell + dy, ACCENT);
                    }
                }
            }
        }
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_names_round_trip() {
        let path = placeholder_path(12, [9, 16]);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(parse_placeholder(name), Some((12, [9, 16])));
        assert_eq!(parse_placeholder("slot-x-9x16-v1.png"), None);
    }

    #[test]
    fn a_placeholder_has_the_slot_shape_and_shows_its_number() {
        assert_eq!(placeholder_size([9, 16]), (720, 1280));
        assert_eq!(placeholder_size([16, 9]), (1280, 720));
        assert_eq!(placeholder_size([1, 1]), (720, 720));
        let image = draw_placeholder(7, 360, 640);
        let accent = image.pixels().filter(|p| p.0[..3] == ACCENT).count();
        // The frame's corners and the digit, not a blank field.
        assert!(accent > 1000, "{accent} accent pixels");
        // The middle row crosses the digit.
        let centre = (0..360)
            .filter(|x| image.get_pixel(*x, 320).0[..3] == ACCENT)
            .count();
        assert!(centre > 0);
    }
}
