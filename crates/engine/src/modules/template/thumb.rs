//! A template's preview tile: one frame of it, drawn by the compositor, with
//! our sample landscapes in the slots instead of the numbered placeholders —
//! the tile should show what the template *does*, not that it is empty.
//!
//! Blocking and GPU-bound: the app calls it from a background thread.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::modules::fx::tiles::{cached, compositor, hash, sample_picture, upload};
use crate::modules::media::MediaSourceProvider;
use crate::modules::project::document::{Micros, Project};
use crate::modules::render::{RenderContext, SourceFrame, SourceProvider, SourceRequest};

use super::format::TemplateFile;
use super::slot;

/// Serves sample pictures for the slots and everything else from the files.
struct Sources {
    files: MediaSourceProvider,
    /// Slot material id → (width, height, night).
    slots: HashMap<String, (u32, u32, bool)>,
}

impl SourceProvider for Sources {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        if let Some(&(w, h, night)) = self.slots.get(request.material_id) {
            let bytes = sample_picture(w, h, night);
            return Ok(Some(upload(ctx, &bytes, w, h)));
        }
        self.files.frame(ctx, request)
    }
}

/// The instant a tile shows: the manifest's, else the middle of slot 1.
pub fn cover_time(file: &TemplateFile, project: &Project) -> Micros {
    file.cover_time.unwrap_or_else(|| {
        slot::slots(project)
            .first()
            .map(|s| s.start + s.duration / 2)
            .unwrap_or(0)
    })
}

/// The tile of `file` (whose project is `project`, paths resolved), with its
/// short edge `short` pixels, cached under the cache root by `key`.
pub fn render(
    file: &TemplateFile,
    project: &Project,
    key: &str,
    short: u32,
) -> Result<PathBuf, String> {
    let (cw, ch) = (project.canvas.width.max(1), project.canvas.height.max(1));
    let scale = short.max(16) as f64 / cw.min(ch) as f64;
    let size = (
        ((cw as f64 * scale).round() as u32).max(2) & !1,
        ((ch as f64 * scale).round() as u32).max(2) & !1,
    );
    let digest = hash(&[key, include_str!("thumb.rs"), &format!("{size:?}")]);
    let path = crate::modules::workspace::paths::cache_root()
        .join("template-tiles")
        .join(format!("{digest:016x}-{}x{}.png", size.0, size.1));
    let time = cover_time(file, project);
    cached(path, || {
        // The looks our templates use are files in the LUT library.
        crate::modules::library::commands::library_looks_install()?;
        let mut slots = HashMap::new();
        for (n, s) in slot::slots(project).iter().enumerate() {
            if let Some((_, segment)) = project.segment(&s.segment_id) {
                let (w, h) = sample_size(s.aspect);
                slots.insert(segment.material_id.clone(), (w, h, n % 2 == 1));
            }
        }
        let sources = Sources {
            files: MediaSourceProvider::from_project(project),
            slots,
        };
        let frame = compositor()?
            .render(
                project,
                time.clamp(0, (project.duration() - 1).max(0)),
                size,
                &sources,
            )
            .map_err(|e| e.to_string())?;
        image::RgbaImage::from_raw(frame.width, frame.height, frame.data)
            .ok_or_else(|| "the frame did not fit its own size".into())
    })
}

/// A sample picture of the slot's shape, small: it only has to fill a tile.
fn sample_size(aspect: [u32; 2]) -> (u32, u32) {
    let ratio = slot::ratio(aspect);
    if ratio >= 1.0 {
        ((360.0 * ratio).round() as u32, 360)
    } else {
        (360, (360.0 / ratio).round() as u32)
    }
}
