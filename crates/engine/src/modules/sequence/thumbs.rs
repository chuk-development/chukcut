//! A compound clip's filmstrip: its sequence rendered, the way the preview
//! draws it — overlays, titles, grades and all — rather than the thumbnails
//! of whichever clip inside happens to be on top.
//!
//! A strip is `count` frames spread evenly over the whole sequence, exactly
//! like a video's strip spreads over the whole file, so the timeline picks a
//! tile for a compound clip the same way it does for a video. The key a strip
//! is cached under carries the sequence's digest (`super::digest`), so an
//! edit inside makes a new strip and the old one can be dropped.

use std::sync::{Arc, OnceLock};

use crate::modules::media::MediaSourceProvider;
use crate::modules::project::{Micros, Project};
use crate::modules::render::Compositor;

/// One rendered tile, in GPUI's byte order.
pub struct Tile {
    pub width: u32,
    pub height: u32,
    /// Tightly packed BGRA8.
    pub bgra: Vec<u8>,
}

/// What a strip of sequence `id` is cached under: its id and the digest of
/// everything its picture depends on. `None` for a sequence that is not
/// parked (only a parked sequence can be on a compound clip).
pub fn strip_key(project: &Project, id: &str) -> Option<String> {
    let canvas = (project.canvas.width, project.canvas.height);
    let digest = super::digest::digest(&project.materials, canvas, id)?;
    Some(format!("{id}#{digest:016x}"))
}

/// The sequence `key` (from [`strip_key`]) belongs to.
pub fn strip_sequence(key: &str) -> &str {
    key.split('#').next().unwrap_or(key)
}

/// The compositor every strip renders with: one per process, on the shared
/// device, built on first use.
fn compositor() -> Result<Arc<Compositor>, String> {
    static COMPOSITOR: OnceLock<Option<Arc<Compositor>>> = OnceLock::new();
    COMPOSITOR
        .get_or_init(|| {
            let ctx = crate::modules::gpu::render_context()?;
            Some(Arc::new(Compositor::new(ctx)))
        })
        .clone()
        .ok_or_else(|| "this machine has no GPU that can render frames".to_string())
}

/// `count` frames of sequence `id`, evenly spread over its length (tile `i`
/// shows the middle of the `i`-th of `count` equal stretches), `height`
/// pixels tall at the canvas's aspect. Blocking: it decodes and renders, so
/// run it off the UI thread.
pub fn sequence_strip(
    project: &Project,
    id: &str,
    count: usize,
    height: u32,
) -> Result<Vec<Tile>, String> {
    if count == 0 {
        return Err("a strip needs at least one frame".into());
    }
    let view = super::nested(project, id).ok_or("that compound clip's contents are gone")?;
    let duration = view.duration();
    if duration <= 0 {
        return Err("the compound clip is empty".into());
    }
    let height = height.max(2) & !1;
    let aspect = project.canvas.width.max(1) as f64 / project.canvas.height.max(1) as f64;
    let width = (((height as f64 * aspect).round() as u32).max(2) + 1) & !1;
    let compositor = compositor()?;
    let sources = MediaSourceProvider::from_project(&view);
    let mut tiles = Vec::with_capacity(count);
    for i in 0..count {
        let at = ((2 * i + 1) as f64 * duration as f64 / (2 * count) as f64) as Micros;
        let frame = compositor
            .render(&view, at, (width, height), &sources)
            .map_err(|e| e.to_string())?;
        let mut bgra = frame.data;
        for pixel in bgra.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        tiles.push(Tile {
            width,
            height,
            bgra,
        });
    }
    sources.clear();
    Ok(tiles)
}
