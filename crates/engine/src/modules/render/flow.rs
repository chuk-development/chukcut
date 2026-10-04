//! Baked "Optical flow (AI)" frames on the GPU: the compositor's side of
//! `speed::flow`.
//!
//! For a clip with optical flow on, at an instant between two source frames,
//! [`FlowFrames::get`] finds the frame RIFE made for that pair and phase in
//! the cache, decodes the JPEG, uploads it as an ordinary `Rgba8UnormSrgb`
//! source frame and keeps the last few, so a paused preview does not read
//! the file again. The compositor draws it in place of the decoded frame,
//! with every clip feature (grade, masks, matte, effects) as usual.
//!
//! A frame that is not baked yet answers `None` and the clip is drawn as a
//! plain frame blend; an export bakes what it needs before it renders
//! (`speed::flow::jobs::ensure`).

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::source::SourceFrame;
use super::RenderContext;
use crate::modules::speed::flow::{self, FlowSample};

/// How long a directory listing is trusted when a frame is not in it: a
/// bake writes several frames a second, and listing more often costs more
/// than it shows.
const RELIST_AFTER: Duration = Duration::from_millis(500);
/// Uploaded frames kept: a pause, and a few frames of playback.
const KEEP: usize = 6;

struct Listing {
    /// `None` when the media file cannot be read (offline media).
    key: Option<String>,
    dir: Option<PathBuf>,
    frames: BTreeSet<FlowSample>,
    listed: Instant,
}

/// Lookups and uploads, shared by every render of one compositor.
#[derive(Default)]
pub struct FlowFrames {
    /// By media path.
    listings: Mutex<HashMap<String, Listing>>,
    uploaded: Mutex<Vec<((PathBuf, FlowSample), SourceFrame)>>,
}

impl FlowFrames {
    /// The baked frame of `media` for `sample`, or one a step either side
    /// (a render at a time a few microseconds off the bake's grid rounds
    /// the other way), or `None` when none is baked.
    pub fn get(&self, ctx: &RenderContext, media: &str, sample: FlowSample) -> Option<SourceFrame> {
        let (dir, sample) = self.locate(media, sample)?;
        let key = (dir, sample);
        if let Some((_, hit)) = self.uploaded.lock().iter().find(|(k, _)| *k == key) {
            return Some(hit.clone());
        }
        let (width, height, rgba) = match flow::read(&key.0, sample) {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(%error, "a baked optical-flow frame could not be read; blending");
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

    fn locate(&self, media: &str, sample: FlowSample) -> Option<(PathBuf, FlowSample)> {
        let find = |listing: &Listing| -> Option<(PathBuf, FlowSample)> {
            let dir = listing.dir.clone()?;
            [0i64, -1, 1].into_iter().find_map(|d| {
                let step = sample.step as i64 + d;
                if step <= 0 || step >= flow::STEPS as i64 {
                    return None;
                }
                let near = FlowSample {
                    index: sample.index,
                    step: step as u32,
                };
                listing.frames.contains(&near).then(|| (dir.clone(), near))
            })
        };
        let mut listings = self.listings.lock();
        let listing = listings.entry(media.to_string()).or_insert_with(|| {
            let key = flow::key_for(media.as_ref()).ok();
            let best = key.as_deref().and_then(flow::best);
            Listing {
                key,
                dir: best.as_ref().map(|(dir, _)| dir.clone()),
                frames: best.map(|(_, frames)| frames).unwrap_or_default(),
                listed: Instant::now(),
            }
        });
        if let Some(hit) = find(listing) {
            return Some(hit);
        }
        if listing.listed.elapsed() < RELIST_AFTER {
            return None;
        }
        let best = listing.key.as_deref().and_then(flow::best);
        listing.dir = best.as_ref().map(|(dir, _)| dir.clone());
        listing.frames = best.map(|(_, frames)| frames).unwrap_or_default();
        listing.listed = Instant::now();
        find(listing)
    }
}

fn upload(ctx: &RenderContext, width: u32, height: u32, rgba: &[u8]) -> SourceFrame {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    // `Rgba8UnormSrgb`, like the decoder's RGBA frames, so the sampler
    // linearises it the same way and an in-between frame matches its
    // neighbours.
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut optical-flow frame"),
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
