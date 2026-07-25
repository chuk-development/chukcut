//! The bridge between decoding and compositing.
//!
//! The compositor asks "give me a texture for material X at source time T" and
//! knows nothing about files, codecs or FFmpeg. This module answers that
//! question: it resolves the material id to a path, keeps a decoder open per
//! material, decodes the requested frame, and uploads it to the GPU.
//!
//! ## Why it holds a snapshot rather than the live project
//!
//! A render walks the timeline while the user keeps editing. Borrowing the
//! live document for the duration would mean holding the project lock across
//! decode calls, which freezes the UI — exactly what the IPC contract
//! forbids. Instead the provider is built from a snapshot of the material
//! pool: a small map of id → path that is cheap to build and immutable
//! afterwards. An export started ten seconds ago renders the timeline as it
//! was ten seconds ago, which is also what the user expects.
//!
//! ## Caching
//!
//! Two layers, for two different costs:
//!
//! - **Decoders** are cached per material because opening a file and finding a
//!   keyframe is expensive, and because a decoder that stays open can answer a
//!   sequential request by decoding one frame forward instead of seeking.
//! - **Textures** are cached per (material, frame) because a paused preview,
//!   a resized window and a re-render after an unrelated edit all ask for the
//!   same frame repeatedly, and re-uploading 8 MB each time is pure waste.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use super::decoder::VideoDecoder;
use crate::modules::project::{Id, MaterialKind, Micros, Project};
use crate::modules::render::{RenderContext, SourceFrame, SourceProvider, SourceRequest};

/// Where a material's pixels come from.
#[derive(Debug, Clone)]
enum MaterialSource {
    Video { path: PathBuf },
    Image { path: PathBuf },
    /// Text is rasterized rather than decoded. Not yet implemented; the
    /// provider returns nothing for these so the rest of the frame still
    /// composites.
    Text,
}

/// How close two requested times must be to reuse a cached texture.
///
/// Rendering at 30 fps asks for times 33 333 µs apart, so anything below half
/// a frame is safely "the same frame". Using an absolute value rather than the
/// project frame rate keeps the provider independent of the timeline.
const FRAME_EPSILON: Micros = 8_000;

/// A cached upload, keyed by material.
struct CachedTexture {
    source_time: Micros,
    frame: SourceFrame,
}

pub struct MediaSourceProvider {
    /// Material id → where its pixels live. Built once from a project snapshot.
    sources: HashMap<Id, MaterialSource>,
    /// One decoder per material. `VideoDecoder` is `Send` but not `Sync`, so
    /// the whole map sits behind a mutex; two threads seeking one demuxer
    /// would fight over its read position anyway.
    decoders: Mutex<HashMap<Id, VideoDecoder>>,
    textures: Mutex<HashMap<Id, CachedTexture>>,
}

impl MediaSourceProvider {
    /// Snapshot the material pool of `project`.
    pub fn from_project(project: &Project) -> Self {
        let mut sources = HashMap::new();

        for video in &project.materials.videos {
            sources.insert(
                video.id.clone(),
                MaterialSource::Video {
                    path: PathBuf::from(&video.path),
                },
            );
        }
        for image in &project.materials.images {
            sources.insert(
                image.id.clone(),
                MaterialSource::Image {
                    path: PathBuf::from(&image.path),
                },
            );
        }
        for text in &project.materials.texts {
            sources.insert(text.id.clone(), MaterialSource::Text);
        }

        Self {
            sources,
            decoders: Mutex::new(HashMap::new()),
            textures: Mutex::new(HashMap::new()),
        }
    }

    /// Drop every cached decoder and texture. Called when a render session
    /// ends, so a finished export does not pin a gigabyte of GPU memory.
    pub fn clear(&self) {
        self.decoders.lock().clear();
        self.textures.lock().clear();
    }

    /// Number of materials this provider can serve, for diagnostics.
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    fn cached(&self, material_id: &str, source_time: Micros) -> Option<SourceFrame> {
        let textures = self.textures.lock();
        let cached = textures.get(material_id)?;
        ((cached.source_time - source_time).abs() <= FRAME_EPSILON).then(|| SourceFrame {
            texture: Arc::clone(&cached.frame.texture),
            view: Arc::clone(&cached.frame.view),
            width: cached.frame.width,
            height: cached.frame.height,
        })
    }

    fn store(&self, material_id: &str, source_time: Micros, frame: &SourceFrame) {
        self.textures.lock().insert(
            material_id.to_string(),
            CachedTexture {
                source_time,
                frame: SourceFrame {
                    texture: Arc::clone(&frame.texture),
                    view: Arc::clone(&frame.view),
                    width: frame.width,
                    height: frame.height,
                },
            },
        );
    }

    fn video_frame(
        &self,
        ctx: &RenderContext,
        material_id: &str,
        path: &PathBuf,
        source_time: Micros,
    ) -> anyhow::Result<SourceFrame> {
        let mut decoders = self.decoders.lock();
        let decoder = match decoders.get_mut(material_id) {
            Some(decoder) => decoder,
            None => {
                let decoder = VideoDecoder::open(path)?;
                decoders.entry(material_id.to_string()).or_insert(decoder)
            }
        };

        let decoded = decoder.seek_and_decode(source_time)?;
        let frame = upload_rgba(ctx, &decoded.data, decoded.width, decoded.height);
        // Drop the decoder lock before touching the texture cache; the two are
        // independent and holding both invites a lock-order bug later.
        drop(decoders);

        self.store(material_id, source_time, &frame);
        Ok(frame)
    }

    fn image_frame(
        &self,
        ctx: &RenderContext,
        material_id: &str,
        path: &PathBuf,
    ) -> anyhow::Result<SourceFrame> {
        // A still never changes, so any cached upload is valid regardless of
        // the requested time.
        if let Some(cached) = self.textures.lock().get(material_id) {
            return Ok(SourceFrame {
                texture: Arc::clone(&cached.frame.texture),
                view: Arc::clone(&cached.frame.view),
                width: cached.frame.width,
                height: cached.frame.height,
            });
        }

        let image = image::open(path)?.to_rgba8();
        let (width, height) = image.dimensions();
        let frame = upload_rgba(ctx, image.as_raw(), width, height);
        self.store(material_id, 0, &frame);
        Ok(frame)
    }
}

impl SourceProvider for MediaSourceProvider {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        let Some(source) = self.sources.get(request.material_id) else {
            // A segment pointing at a material that is not in the pool is a
            // document bug, caught by Project::validate. Skipping it here
            // keeps the rest of the frame renderable instead of failing the
            // whole composite.
            tracing::warn!(
                material_id = request.material_id,
                "no source for material; skipping"
            );
            return Ok(None);
        };

        // Audio contributes nothing to a video frame.
        if request.kind == MaterialKind::Audio {
            return Ok(None);
        }

        if let Some(cached) = self.cached(request.material_id, request.source_time) {
            return Ok(Some(cached));
        }

        match source {
            MaterialSource::Video { path } => self
                .video_frame(ctx, request.material_id, path, request.source_time)
                .map(Some),
            MaterialSource::Image { path } => self
                .image_frame(ctx, request.material_id, path)
                .map(Some),
            MaterialSource::Text => Ok(None),
        }
    }
}

/// Upload tightly packed RGBA8 to a fresh GPU texture.
///
/// `Rgba8UnormSrgb` rather than `Rgba8Unorm`: decoded video is sRGB-encoded, and
/// tagging it as such is what makes the sampler linearize before the compositor
/// blends. Blending sRGB values directly is the standard way to get muddy
/// cross-fades.
fn upload_rgba(ctx: &RenderContext, data: &[u8], width: u32, height: u32) -> SourceFrame {
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("media source"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
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
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    SourceFrame::from_texture(Arc::new(texture))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{CanvasConfig, ImageMaterial, VideoMaterial};

    fn project_with_materials() -> Project {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "v1".into(),
            path: "/nonexistent/clip.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 5_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        project.materials.images.push(ImageMaterial {
            id: "i1".into(),
            path: "/nonexistent/still.png".into(),
            width: 800,
            height: 600,
        });
        project
    }

    #[test]
    fn snapshot_covers_every_material_kind() {
        let provider = MediaSourceProvider::from_project(&project_with_materials());
        assert_eq!(provider.len(), 2);
        assert!(provider.sources.contains_key("v1"));
        assert!(provider.sources.contains_key("i1"));
    }

    #[test]
    fn an_empty_project_yields_an_empty_provider() {
        let project = Project::new("t", CanvasConfig::default(), 30.0);
        assert!(MediaSourceProvider::from_project(&project).is_empty());
    }

    #[test]
    fn frame_epsilon_treats_one_render_step_as_distinct() {
        // Two adjacent frames at 60 fps are 16 666 µs apart, which must not
        // collapse to the same cache entry.
        assert!(FRAME_EPSILON < 16_666);
        // Repeated requests for the same nominal frame must hit, even with
        // rounding noise in the microsecond conversion.
        assert!(FRAME_EPSILON > 2);
    }
}
