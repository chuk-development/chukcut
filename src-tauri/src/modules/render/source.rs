//! Where pixels come from.
//!
//! The compositor decodes nothing. It knows a segment references a material and
//! that it needs that material's appearance at some source instant; turning
//! that into a GPU texture — demuxing, decoding, colour converting, uploading,
//! caching, rasterizing a text layer — is the `media` module's job.
//!
//! That split is what keeps this module testable without FFmpeg and keeps the
//! decoder free to be as clever as it likes about read-ahead and frame reuse.
//! The compositor asks for a frame and draws whatever it is handed.

use std::sync::Arc;

use super::context::RenderContext;
use crate::modules::project::document::{MaterialKind, Micros};

/// What the compositor wants and why.
///
/// The provider is free to ignore every hint here; they exist so a decoder can
/// pick a proxy level instead of always producing full resolution.
#[derive(Debug, Clone, Copy)]
pub struct SourceRequest<'a> {
    /// Id into `MaterialPool`.
    pub material_id: &'a str,
    pub kind: MaterialKind,
    /// Position inside the *source*, already mapped through the segment's
    /// speed by `Segment::source_time_at`.
    pub source_time: Micros,
    /// The segment asking. Providers that cache per placement (a rasterized
    /// text layer, say) key on this rather than on the material.
    pub segment_id: &'a str,
    /// Roughly the largest this material will be drawn on the canvas, in
    /// pixels. A hint for proxy selection, not a requirement — the compositor
    /// scales whatever it gets.
    pub max_size: (u32, u32),
}

/// One material's appearance at one instant, as a texture the compositor can
/// bind.
///
/// The texture must carry [`wgpu::TextureUsages::TEXTURE_BINDING`], be
/// `D2` with a single mip and sample count 1, and be filterable — in practice
/// an `Rgba8Unorm` or `Rgba8UnormSrgb` upload. Alpha is straight, not
/// premultiplied; the blend state matches.
///
/// Everything is `Arc` because providers cache: the same decoded frame is
/// normally handed to several consecutive renders.
#[derive(Clone)]
pub struct SourceFrame {
    pub texture: Arc<wgpu::Texture>,
    pub view: Arc<wgpu::TextureView>,
    /// Pixel size of `texture`. This — not the material's declared dimensions —
    /// is what the compositor fits into the canvas, so a provider that hands
    /// back an upright texture for a 90°-rotated file gets the right aspect
    /// without the compositor knowing about container rotation.
    pub width: u32,
    pub height: u32,
}

impl SourceFrame {
    /// Wrap a texture, creating the default view.
    pub fn from_texture(texture: Arc<wgpu::Texture>) -> Self {
        let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let (width, height) = (texture.width(), texture.height());
        Self {
            texture,
            view,
            width,
            height,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl std::fmt::Debug for SourceFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("format", &self.texture.format())
            .finish()
    }
}

/// The compositor's only way to obtain pixels.
///
/// `Ok(None)` means "nothing to draw here, and that is fine" — an audio-only
/// segment, a material this provider does not handle, a frame past the end of
/// a file. `Err` means something went wrong that a human might want to know
/// about; the compositor logs it and skips the segment unless configured to be
/// strict.
///
/// Implementations must be cheap to call repeatedly for the same request:
/// preview renders the same instant several times while the user drags a
/// handle.
pub trait SourceProvider: Send + Sync {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>>;
}

impl<T: SourceProvider + ?Sized> SourceProvider for &T {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        (**self).frame(ctx, request)
    }
}

impl<T: SourceProvider + ?Sized> SourceProvider for Arc<T> {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        (**self).frame(ctx, request)
    }
}

/// A provider that draws nothing. Renders the background and nothing else,
/// which is what the preview shows before `media` is wired up.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptySourceProvider;

impl SourceProvider for EmptySourceProvider {
    fn frame(
        &self,
        _ctx: &RenderContext,
        _request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Test provider
// ---------------------------------------------------------------------------

/// A material that is one flat colour, at a chosen resolution.
#[derive(Debug, Clone, Copy)]
pub struct SolidSource {
    /// Straight (non-premultiplied) RGBA, 0..1.
    pub color: [f32; 4],
    pub width: u32,
    pub height: u32,
}

impl SolidSource {
    pub fn new(color: [f32; 4], width: u32, height: u32) -> Self {
        Self {
            color,
            width,
            height,
        }
    }
}

/// A `SourceProvider` backed by solid colours, so the compositor can be
/// exercised without a decoder.
///
/// This is not a stub with a `todo!()` in it — it produces real textures on the
/// real device, which is what makes it useful: a test can render a project and
/// assert on the resulting pixels, and every transform, crop and blending bug
/// shows up as a wrong colour at a known coordinate.
pub struct SolidColorProvider {
    materials: parking_lot::RwLock<std::collections::HashMap<String, SolidSource>>,
    /// Textures are built once per material and reused, which also exercises
    /// the caching path a real decoder will take.
    cache: parking_lot::RwLock<std::collections::HashMap<String, SourceFrame>>,
    fallback: Option<SolidSource>,
    calls: std::sync::atomic::AtomicUsize,
}

impl Default for SolidColorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SolidColorProvider {
    pub fn new() -> Self {
        Self {
            materials: Default::default(),
            cache: Default::default(),
            fallback: None,
            calls: Default::default(),
        }
    }

    /// Answer for every material id that was not registered explicitly.
    pub fn with_fallback(mut self, source: SolidSource) -> Self {
        self.fallback = Some(source);
        self
    }

    pub fn insert(&self, material_id: impl Into<String>, source: SolidSource) {
        self.materials.write().insert(material_id.into(), source);
    }

    pub fn with(self, material_id: impl Into<String>, source: SolidSource) -> Self {
        self.insert(material_id, source);
        self
    }

    /// How many times the compositor asked for a frame. Lets a test assert
    /// that hidden tracks really were skipped rather than drawn transparently.
    pub fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn build(&self, ctx: &RenderContext, source: SolidSource) -> SourceFrame {
        let texel = [
            (source.color[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (source.color[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (source.color[2].clamp(0.0, 1.0) * 255.0).round() as u8,
            (source.color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
        let (width, height) = (source.width.max(1), source.height.max(1));
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..(width as usize * height as usize) {
            pixels.extend_from_slice(&texel);
        }

        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("chukcut solid test source"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
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
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        SourceFrame::from_texture(std::sync::Arc::new(texture))
    }
}

impl SourceProvider for SolidColorProvider {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if let Some(hit) = self.cache.read().get(request.material_id) {
            return Ok(Some(hit.clone()));
        }

        let Some(source) = self
            .materials
            .read()
            .get(request.material_id)
            .copied()
            .or(self.fallback)
        else {
            return Ok(None);
        };

        let frame = self.build(ctx, source);
        self.cache
            .write()
            .insert(request.material_id.to_string(), frame.clone());
        Ok(Some(frame))
    }
}
