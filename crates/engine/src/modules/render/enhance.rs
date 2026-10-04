//! Remade frames on the GPU ("Remove object", "Enhance quality"): the
//! compositor's side of `modules/enhance`.
//!
//! For a clip with a chain on, [`EnhancedFrames::get`] finds the made frame
//! of the source frame being drawn in the cache, decodes the JPEG at the
//! smallest of full, half, quarter or eighth size that still covers the
//! render (a 4K enhanced frame in a 960 px preview decodes at a quarter, in
//! the DCT), uploads it as an ordinary `Rgba8UnormSrgb` source frame and
//! keeps the last few, so a paused preview does not read the file again.
//! The compositor draws it in place of the decoded frame, with every clip
//! feature (grade, masks, matte, effects) as usual.
//!
//! A frame not made yet answers `None` and the decoded frame is drawn; an
//! export makes what it needs before it renders (`enhance::jobs::ensure`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::source::SourceFrame;
use super::RenderContext;
use crate::modules::enhance::{cache, Chain};
use crate::modules::project::document::Micros;

/// How long a directory listing is trusted when a frame is not in it.
const RELIST_AFTER: Duration = Duration::from_millis(500);
/// Uploaded frames kept: a pause, a few frames of playback, both sides of a
/// transition.
const KEEP: usize = 6;

struct Listing {
    /// `None` when the media file cannot be read (offline media).
    key: Option<String>,
    dir: Option<PathBuf>,
    times: Vec<Micros>,
    listed: Instant,
}

/// A made frame as uploaded: its file and the size it was decoded at.
type Uploaded = ((PathBuf, Micros, (u32, u32)), SourceFrame);

/// Lookups and uploads, shared by every render of one compositor.
#[derive(Default)]
pub struct EnhancedFrames {
    /// By media path and chain signature.
    listings: Mutex<HashMap<(String, String), Listing>>,
    uploaded: Mutex<Vec<Uploaded>>,
}

impl EnhancedFrames {
    /// The made frame of `media`'s source frame at `source_time` (a file of
    /// frame period `period`) for `chain`, decoded to cover `want` (width,
    /// height; `(0, 0)` for whole), or `None` when it is not made.
    pub fn get(
        &self,
        ctx: &RenderContext,
        media: &str,
        chain: &Chain,
        source_time: Micros,
        period: Micros,
        want: (u32, u32),
    ) -> Option<SourceFrame> {
        let (dir, pts) = self.locate(media, chain, source_time, period)?;
        let key = (dir, pts, want);
        if let Some((_, hit)) = self.uploaded.lock().iter().find(|(k, _)| *k == key) {
            return Some(hit.clone());
        }
        let (width, height, rgba) = match cache::read(&key.0, pts, want) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(%error, "a remade frame could not be read; drawing the decoded one");
                return None;
            }
        };
        let frame = upload(ctx, width, height, &rgba);
        let mut uploaded = self.uploaded.lock();
        if uploaded.len() >= KEEP {
            uploaded.remove(0);
        }
        uploaded.push((key, frame.clone()));
        Some(frame)
    }

    fn locate(
        &self,
        media: &str,
        chain: &Chain,
        source_time: Micros,
        period: Micros,
    ) -> Option<(PathBuf, Micros)> {
        let lookup = crate::modules::matting::cache::lookup;
        let mut listings = self.listings.lock();
        let listing = listings
            .entry((media.to_string(), chain.signature()))
            .or_insert_with(|| {
                let key = cache::key_for(media.as_ref(), chain).ok();
                let best = key.as_deref().and_then(cache::best);
                Listing {
                    key,
                    dir: best.as_ref().map(|(dir, _)| dir.clone()),
                    times: best.map(|(_, times)| times).unwrap_or_default(),
                    listed: Instant::now(),
                }
            });
        if let Some(dir) = &listing.dir {
            if let Some(pts) = lookup(&listing.times, source_time, period) {
                return Some((dir.clone(), pts));
            }
        }
        if listing.listed.elapsed() < RELIST_AFTER {
            return None;
        }
        let best = listing.key.as_deref().and_then(cache::best);
        listing.dir = best.as_ref().map(|(dir, _)| dir.clone());
        listing.times = best.map(|(_, times)| times).unwrap_or_default();
        listing.listed = Instant::now();
        let dir = listing.dir.clone()?;
        lookup(&listing.times, source_time, period).map(|pts| (dir, pts))
    }
}

fn upload(ctx: &RenderContext, width: u32, height: u32, rgba: &[u8]) -> SourceFrame {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    // `Rgba8UnormSrgb`, like the decoder's RGBA frames, so the sampler
    // linearises it the same way and a made frame matches a decoded one.
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut remade frame"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        size,
    );
    SourceFrame::from_texture(Arc::new(texture))
}
