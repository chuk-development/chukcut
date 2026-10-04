//! Baked "Remove background" mattes on the GPU: the compositor's side of
//! `modules/matting`.
//!
//! For a clip with the setting on, [`MatteFrames::get`] finds the matte of
//! the source frame being drawn in the cache, uploads it as an `R8Unorm`
//! texture and keeps the last few, so a paused preview that redraws the same
//! instant does not read the PNG again. `quad.wgsl` multiplies it into the
//! clip's alpha (`M_BACKGROUND`), sampled in display orientation over the
//! whole source frame, so a crop, a transform or a rotated file all line up.
//!
//! A frame with no matte yet (a bake still running) answers `None` and the
//! clip draws as it is. The export cannot meet that case: it bakes every
//! missing matte before it renders (`matting::commands::matting_ensure`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::RenderContext;
use crate::modules::matting::cache;
use crate::modules::project::compositing::BackgroundRemoval;
use crate::modules::project::document::Micros;

/// One matte, uploaded.
pub struct GpuMatte {
    pub view: wgpu::TextureView,
}

/// How long a listing of a matte directory is trusted when a frame is not
/// in it. A bake writes several frames a second; re-reading the directory
/// more often than this would cost more than it shows.
const RELIST_AFTER: Duration = Duration::from_millis(500);
/// Uploaded mattes kept: a few frames of playback, and both sides of a
/// transition between two matted clips.
const KEEP: usize = 8;

struct Listing {
    /// `None` when the media file cannot be read (offline media).
    dir: Option<PathBuf>,
    times: Vec<Micros>,
    listed: Instant,
}

/// A matte file: its directory and its presentation time.
type MatteKey = (PathBuf, Micros);

/// Matte lookups and uploads, shared by every render of one compositor.
#[derive(Default)]
pub struct MatteFrames {
    listings: Mutex<HashMap<(String, String, String), Listing>>,
    uploaded: Mutex<Vec<(MatteKey, Arc<GpuMatte>)>>,
}

impl MatteFrames {
    /// The matte of `media`'s frame at `source_time` (a file of frame period
    /// `period`) made with `setting`, or `None` when it is not baked.
    pub fn get(
        &self,
        ctx: &RenderContext,
        media: &str,
        setting: &BackgroundRemoval,
        source_time: Micros,
        period: Micros,
    ) -> Option<Arc<GpuMatte>> {
        let (dir, pts) = self.locate(media, setting, source_time, period)?;
        let key = (dir, pts);
        if let Some((_, hit)) = self.uploaded.lock().iter().find(|(k, _)| *k == key) {
            return Some(Arc::clone(hit));
        }
        let (width, height, alpha) = match cache::read(&key.0, pts) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(%error, "a baked matte could not be read; drawing the clip whole");
                return None;
            }
        };
        let gpu = Arc::new(upload(ctx, width, height, &alpha));
        let mut uploaded = self.uploaded.lock();
        if uploaded.len() >= KEEP {
            uploaded.remove(0);
        }
        uploaded.push((key, Arc::clone(&gpu)));
        Some(gpu)
    }

    /// The directory and the presentation time of the matte to draw.
    fn locate(
        &self,
        media: &str,
        setting: &BackgroundRemoval,
        source_time: Micros,
        period: Micros,
    ) -> Option<(PathBuf, Micros)> {
        let key = (
            media.to_string(),
            setting.model.clone(),
            setting.version.clone(),
        );
        let mut listings = self.listings.lock();
        let listing = listings.entry(key).or_insert_with(|| {
            let dir = cache::dir_for(media.as_ref(), &setting.model, &setting.version).ok();
            let times = dir.as_deref().map(cache::list).unwrap_or_default();
            Listing {
                dir,
                times,
                listed: Instant::now(),
            }
        });
        let dir = listing.dir.clone()?;
        if let Some(pts) = cache::lookup(&listing.times, source_time, period) {
            return Some((dir, pts));
        }
        if listing.listed.elapsed() < RELIST_AFTER {
            return None;
        }
        listing.times = cache::list(&dir);
        listing.listed = Instant::now();
        cache::lookup(&listing.times, source_time, period).map(|pts| (dir, pts))
    }
}

fn upload(ctx: &RenderContext, width: u32, height: u32, alpha: &[u8]) -> GpuMatte {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut background matte"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // `write_texture` has no row alignment rule (only buffer copies do), so
    // the PNG's tightly packed rows go up as they are.
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        alpha,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        size,
    );
    GpuMatte {
        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
    }
}
