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

/// Which set of luma weights turns this material's YUV back into RGB.
///
/// Taken from the file rather than assumed. The two common answers differ by
/// enough to matter — BT.601 read as BT.709 shifts saturated reds and greens by
/// ten code values or so — and the difference is a consistent tint rather than
/// an obvious fault, so nobody catches it until a delivery is rejected. SD
/// material is usually BT.601 and HD usually BT.709, but "usually" is not a
/// decoder.
///
/// The discriminants are what the fragment shader switches on; `yuv.wgsl`
/// spells the same numbers and a test asserts they still agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum YuvMatrix {
    Bt601 = 0,
    #[default]
    Bt709 = 1,
    Bt2020 = 2,
}

/// Whether luma occupies 16..235 or 0..255.
///
/// Limited is the default for everything a camera writes; full turns up in
/// screen recordings and in anything that has been through JPEG. Reading full
/// as limited crushes the ends of the ramp, reading limited as full leaves grey
/// blacks — the failure `docs/research/vaapi-jpeg-preview.md` records from the
/// encode side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum YuvRange {
    #[default]
    Limited = 0,
    Full = 1,
}

/// Something that has to stay alive for as long as the textures do.
///
/// The mapped-decode path imports a texture over memory the *decoder* owns: a
/// VA surface out of a fixed pool, which the decoder will happily recycle and
/// overwrite the moment nothing references the frame it came from. The
/// reference is an `AVFrame` held by `media::dmabuf::DmabufFrame`, which the
/// compositor has no business knowing about — so it travels as an opaque
/// keep-alive that is dropped with the last clone of the [`SourceFrame`].
///
/// Losing this is not a crash. It is the previous frame's picture appearing
/// intermittently under motion, which reads as a decoder bug.
pub type FrameGuard = Arc<dyn std::any::Any + Send + Sync>;

/// One material's appearance at one instant, as a texture the compositor can
/// bind.
///
/// Two shapes, and the compositor branches on which:
///
/// - **RGBA**, the ordinary case. One filterable `Rgba8Unorm` or
///   `Rgba8UnormSrgb` texture, alpha straight rather than premultiplied.
///   [`Self::from_texture`] builds this and nothing else changed for it.
/// - **Two-plane YUV**, what a hardware decoder produces. `texture` is the
///   `R8Unorm` luma plane, [`Self::chroma`] the half-size `Rg8Unorm` chroma
///   plane, and [`Self::matrix`]/[`Self::range`] say how to combine them. Both
///   are usually imported straight out of the decoder's memory, which is the
///   whole reason this case exists.
///
/// Everything is `Arc` because providers cache: the same decoded frame is
/// normally handed to several consecutive renders.
#[derive(Clone)]
pub struct SourceFrame {
    pub texture: Arc<wgpu::Texture>,
    pub view: Arc<wgpu::TextureView>,
    /// Pixel size of the picture *as displayed*, i.e. with any container
    /// rotation applied. This — not the material's declared dimensions, and not
    /// necessarily `texture`'s size — is what the compositor fits into the
    /// canvas.
    ///
    /// On the RGBA path the decoder has already rotated the pixels, so this is
    /// the texture's own size. On the mapped path nothing has been rotated and
    /// this is the texture's size with the axes exchanged; see [`Self::turns`].
    pub width: u32,
    pub height: u32,
    /// The interleaved CbCr plane, at half resolution in both axes. `Some`
    /// makes this a YUV frame.
    pub chroma: Option<Arc<wgpu::Texture>>,
    pub chroma_view: Option<Arc<wgpu::TextureView>>,
    /// How to convert, when `chroma` is `Some`. Meaningless otherwise.
    pub matrix: YuvMatrix,
    pub range: YuvRange,
    /// Clockwise quarter turns the compositor must apply, because the pixels
    /// have not been. Always 0 on the RGBA path — the decoder rotates there,
    /// since it is already copying every pixel and one more pass is cheap
    /// relative to being wrong everywhere downstream.
    pub turns: u32,
    /// Whatever must outlive the textures. See [`FrameGuard`].
    pub guard: Option<FrameGuard>,
}

impl SourceFrame {
    /// Wrap a texture, creating the default view.
    ///
    /// This is the RGBA case and it means what it always meant: one texture,
    /// already the right way up, sampled directly.
    pub fn from_texture(texture: Arc<wgpu::Texture>) -> Self {
        let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let (width, height) = (texture.width(), texture.height());
        Self {
            texture,
            view,
            width,
            height,
            chroma: None,
            chroma_view: None,
            matrix: YuvMatrix::default(),
            range: YuvRange::default(),
            turns: 0,
            guard: None,
        }
    }

    /// Wrap the two planes of an NV12 surface.
    ///
    /// `turns` is the display rotation in clockwise quarter turns, *not*
    /// applied to the pixels; the reported [`Self::size`] is post-rotation
    /// because that is what the compositor fits, while the textures stay as
    /// they were decoded.
    pub fn from_planes(
        luma: Arc<wgpu::Texture>,
        chroma: Arc<wgpu::Texture>,
        matrix: YuvMatrix,
        range: YuvRange,
        turns: u32,
        guard: Option<FrameGuard>,
    ) -> Self {
        let view = Arc::new(luma.create_view(&wgpu::TextureViewDescriptor::default()));
        let chroma_view = Arc::new(chroma.create_view(&wgpu::TextureViewDescriptor::default()));
        let (coded_w, coded_h) = (luma.width(), luma.height());
        let turns = turns % 4;
        let (width, height) = if turns % 2 == 1 {
            (coded_h, coded_w)
        } else {
            (coded_w, coded_h)
        };
        Self {
            texture: luma,
            view,
            width,
            height,
            chroma: Some(chroma),
            chroma_view: Some(chroma_view),
            matrix,
            range,
            turns,
            guard,
        }
    }

    /// Whether the compositor has to convert rather than sample directly.
    pub fn is_planar(&self) -> bool {
        self.chroma_view.is_some()
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
            .field("planar", &self.is_planar())
            .field("turns", &self.turns)
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

    /// Get ready for a render of `project` at `time`: decode whatever it will
    /// ask for, in parallel where the provider can. A hint, never required —
    /// the default does nothing and `frame` still answers every request.
    fn prefetch(
        &self,
        _ctx: &RenderContext,
        _project: &crate::modules::project::Project,
        _time: crate::modules::project::Micros,
        _size: (u32, u32),
    ) {
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader switches on raw numbers; Rust switches on names. If the two
    /// ever disagree, every hardware-decoded frame is converted with the wrong
    /// matrix and the only symptom is a slight tint.
    #[test]
    fn the_shader_and_rust_agree_on_the_colour_selectors() {
        let wgsl = include_str!("shaders/yuv.wgsl");
        for (name, value) in [
            ("MATRIX_BT601", YuvMatrix::Bt601 as u32),
            ("MATRIX_BT709", YuvMatrix::Bt709 as u32),
            ("MATRIX_BT2020", YuvMatrix::Bt2020 as u32),
            ("RANGE_LIMITED", YuvRange::Limited as u32),
            ("RANGE_FULL", YuvRange::Full as u32),
        ] {
            let declaration = format!("const {name}: u32 = {value}u;");
            assert!(
                wgsl.contains(&declaration),
                "yuv.wgsl does not declare `{declaration}`"
            );
        }
    }

    /// A sideways source reports the size it will be *shown* at, because that
    /// is what the compositor fits into the canvas. Reporting the coded size
    /// would letterbox a portrait clip as though it were landscape.
    #[test]
    fn a_quarter_turned_source_reports_its_display_size() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let plane = |width, height, format| {
            Arc::new(ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }))
        };
        let luma = plane(1920, 1080, wgpu::TextureFormat::R8Unorm);
        let chroma = plane(960, 540, wgpu::TextureFormat::Rg8Unorm);

        let upright = SourceFrame::from_planes(
            Arc::clone(&luma),
            Arc::clone(&chroma),
            YuvMatrix::Bt709,
            YuvRange::Limited,
            0,
            None,
        );
        assert_eq!(upright.size(), (1920, 1080));
        assert!(upright.is_planar());

        let sideways =
            SourceFrame::from_planes(luma, chroma, YuvMatrix::Bt709, YuvRange::Limited, 1, None);
        assert_eq!(sideways.size(), (1080, 1920));
        assert_eq!(sideways.texture.width(), 1920, "the pixels were not moved");
    }
}
