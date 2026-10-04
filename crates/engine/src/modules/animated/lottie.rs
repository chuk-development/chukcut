//! Lottie animations: velato's model, drawn by vello on the shared device.
//!
//! velato turns a Lottie file into a runtime model and, for any frame, walks
//! it into a `RenderSink`. Its own sink for `vello::Scene` is tied to the
//! vello release it was built against (0.10, on wgpu 29); ours, [`Sink`], is
//! the same eight lines against vello 0.11, which is on our wgpu 30. vello
//! then rasterises the scene with compute shaders on `gpu::render_context()`'s
//! device into a texture the compositor samples like a decoded frame.
//!
//! vello writes straight (unpremultiplied) alpha in sRGB-encoded bytes into
//! an `Rgba8Unorm` storage texture. The compositor samples RGBA sources as
//! `Rgba8UnormSrgb`, so the texture is created viewable as both and the frame
//! is handed out through the sRGB view: to the quad shader it is the same as
//! an uploaded PNG.

use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use vello::kurbo::{self, Affine};
use vello::peniko::{self, Fill};

use crate::modules::project::document::Micros;
use crate::modules::render::{RenderContext, SourceFrame};

/// The largest side a Lottie frame is rasterised at. A sticker is drawn at a
/// fraction of the canvas; 2048 covers a full-frame Lottie on a 4K export's
/// short side.
pub const MAX_SIDE: u32 = 2048;

pub struct LottieAnimation {
    composition: velato::Composition,
    pub width: u32,
    pub height: u32,
    /// First and last (exclusive) frame, in the file's frame numbers.
    pub first_frame: f64,
    pub end_frame: f64,
    pub frame_rate: f64,
}

impl LottieAnimation {
    pub fn open(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let composition = velato::Composition::from_slice(bytes)
            .map_err(|e| format!("not a Lottie animation: {e}"))?;
        let (first_frame, end_frame) = (composition.frames.start, composition.frames.end);
        let frame_rate = composition.frame_rate;
        if !(frame_rate.is_finite() && frame_rate > 0.0) {
            return Err("the Lottie file has no frame rate".into());
        }
        if !(first_frame.is_finite() && end_frame.is_finite() && end_frame > first_frame) {
            return Err("the Lottie file has no frames".into());
        }
        let (width, height) = (composition.width as u32, composition.height as u32);
        if width == 0 || height == 0 {
            return Err("the Lottie file has no size".into());
        }
        Ok(Self {
            composition,
            width,
            height,
            first_frame,
            end_frame,
            frame_rate,
        })
    }

    /// One pass of the animation.
    pub fn duration(&self) -> Micros {
        ((self.end_frame - self.first_frame) / self.frame_rate * 1_000_000.0).round() as Micros
    }

    /// The (fractional) frame number shown at `time` into the animation.
    pub fn frame_at(&self, time: Micros) -> f64 {
        let frame = self.first_frame + time.max(0) as f64 * self.frame_rate / 1_000_000.0;
        frame.min(self.end_frame - 1e-3).max(self.first_frame)
    }

    /// The animation at `time` as a vello scene filling `size` pixels.
    pub fn scene(&self, time: Micros, size: (u32, u32)) -> vello::Scene {
        let mut scene = vello::Scene::new();
        let transform = Affine::scale_non_uniform(
            size.0 as f64 / self.width as f64,
            size.1 as f64 / self.height as f64,
        );
        velato::Renderer::new().append(
            &self.composition,
            self.frame_at(time),
            transform,
            1.0,
            &mut Sink(&mut scene),
        );
        scene
    }

    /// The size to rasterise at so the animation fits `max` (the canvas or
    /// the preview), keeping its aspect, at most [`MAX_SIDE`].
    pub fn fitted(&self, max: (u32, u32)) -> (u32, u32) {
        let (w, h) = (self.width as f64, self.height as f64);
        let scale = (max.0.max(1) as f64 / w)
            .min(max.1.max(1) as f64 / h)
            .min(MAX_SIDE as f64 / w.max(h));
        (
            ((w * scale).round() as u32).max(1),
            ((h * scale).round() as u32).max(1),
        )
    }

    /// Rasterise the frame at `time` at `size` on the GPU.
    pub fn render(
        &self,
        ctx: &RenderContext,
        time: Micros,
        size: (u32, u32),
    ) -> Result<SourceFrame, String> {
        let texture = target(ctx, size);
        let storage = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        draw(ctx, &self.scene(time, size), &storage, size)?;
        // Sampling only: an sRGB format cannot be a storage view, and a view
        // inherits every usage of its texture unless told otherwise.
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
            ..Default::default()
        });
        let mut frame = SourceFrame::from_texture(Arc::new(texture));
        frame.view = Arc::new(view);
        Ok(frame)
    }

    /// The frame at `time` as straight-alpha RGBA bytes, for a preview file.
    pub fn render_rgba(
        &self,
        ctx: &RenderContext,
        time: Micros,
        size: (u32, u32),
    ) -> Result<Vec<u8>, String> {
        let texture = target(ctx, size);
        let storage = texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        });
        draw(ctx, &self.scene(time, size), &storage, size)?;
        read_rgba(ctx, &texture)
    }
}

fn target(ctx: &RenderContext, size: (u32, u32)) -> wgpu::Texture {
    ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut lottie frame"),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[wgpu::TextureFormat::Rgba8UnormSrgb],
    })
}

/// The one vello renderer, for the one device. Building it compiles vello's
/// compute shaders (about a second on lavapipe), so it is built on the first
/// Lottie frame and kept. Keyed by device in case a test process ever holds
/// another; a renderer must never be handed a device it was not built on.
fn renderer() -> &'static Mutex<Option<(wgpu::Device, vello::Renderer)>> {
    static RENDERER: OnceLock<Mutex<Option<(wgpu::Device, vello::Renderer)>>> = OnceLock::new();
    RENDERER.get_or_init(|| Mutex::new(None))
}

fn draw(
    ctx: &RenderContext,
    scene: &vello::Scene,
    view: &wgpu::TextureView,
    size: (u32, u32),
) -> Result<(), String> {
    let device = ctx.device();
    let mut slot = renderer().lock();
    if slot.as_ref().is_none_or(|(d, _)| d != device) {
        let renderer = vello::Renderer::new(
            device,
            vello::RendererOptions {
                use_cpu: false,
                antialiasing_support: vello::AaSupport::area_only(),
                num_init_threads: NonZeroUsize::new(1),
                pipeline_cache: None,
            },
        )
        .map_err(|e| format!("the Lottie renderer could not start: {e}"))?;
        *slot = Some((device.clone(), renderer));
    }
    let (_, renderer) = slot.as_mut().expect("just built");
    renderer
        .render_to_texture(
            device,
            ctx.queue(),
            scene,
            view,
            &vello::RenderParams {
                base_color: peniko::Color::TRANSPARENT,
                width: size.0.max(1),
                height: size.1.max(1),
                antialiasing_method: vello::AaConfig::Area,
            },
        )
        .map_err(|e| format!("a Lottie frame did not render: {e}"))
}

/// Copy an `Rgba8Unorm` texture back, rows unpadded.
fn read_rgba(ctx: &RenderContext, texture: &wgpu::Texture) -> Result<Vec<u8>, String> {
    let (width, height) = (texture.width(), texture.height());
    let unpadded = width * 4;
    let padded =
        unpadded.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let device = ctx.device();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chukcut lottie readback"),
        size: padded as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("chukcut lottie readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit(Some(encoder.finish()));
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|_| "the readback never finished".to_string())?
        .map_err(|e| e.to_string())?;
    let out = {
        let view = slice.get_mapped_range().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height as usize {
            let start = row * padded as usize;
            out.extend_from_slice(&view[start..start + unpadded as usize]);
        }
        out
    };
    buffer.unmap();
    Ok(out)
}

/// velato's drawing calls, onto a vello 0.11 scene.
struct Sink<'a>(&'a mut vello::Scene);

impl velato::RenderSink for Sink<'_> {
    fn push_layer(
        &mut self,
        blend: impl Into<peniko::BlendMode>,
        alpha: f32,
        transform: Affine,
        shape: &impl kurbo::Shape,
    ) {
        self.0
            .push_layer(Fill::NonZero, blend, alpha, transform, shape);
    }

    fn push_clip_layer(&mut self, transform: Affine, shape: &impl kurbo::Shape) {
        self.0.push_clip_layer(Fill::NonZero, transform, shape);
    }

    fn pop_layer(&mut self) {
        self.0.pop_layer();
    }

    fn draw(
        &mut self,
        stroke: Option<&velato::model::fixed::Stroke>,
        transform: Affine,
        brush: &velato::model::fixed::Brush,
        shape: &impl kurbo::Shape,
    ) {
        match stroke {
            Some(stroke) => self.0.stroke(stroke, transform, brush, None, shape),
            None => self.0.fill(Fill::NonZero, transform, brush, None, shape),
        }
    }

    /// Image layers are not drawn: their pixels live in files or base64 next
    /// to the animation and are not loaded here. Stickers do not use them.
    fn draw_image(&mut self, _: &velato::model::ImageAsset, _: Affine, _: f64) {}
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 100×50 Lottie, 2 s at 30 fps: a red rectangle at half opacity over
    /// the left half that slides to the right half over the animation.
    pub(crate) const SLIDE: &str = r#"{
      "v":"5.7.0","fr":30,"ip":0,"op":60,"w":100,"h":50,"nm":"slide","ddd":0,"assets":[],
      "layers":[{"ddd":0,"ind":1,"ty":4,"nm":"box","sr":1,
        "ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},
              "p":{"a":1,"k":[{"t":0,"s":[25,25,0],"i":{"x":[1],"y":[1]},"o":{"x":[0],"y":[0]}},{"t":60,"s":[75,25,0]}]},
              "a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]}},
        "ao":0,"ip":0,"op":60,"st":0,"bm":0,
        "shapes":[{"ty":"gr","nm":"g","it":[
          {"ty":"rc","nm":"r","d":1,"s":{"a":0,"k":[50,50]},"p":{"a":0,"k":[0,0]},"r":{"a":0,"k":0}},
          {"ty":"fl","nm":"f","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":50},"r":1},
          {"ty":"tr","p":{"a":0,"k":[0,0]},"a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100}}
        ]}]}]
    }"#;

    #[test]
    fn timing_follows_the_frame_rate_and_stays_inside_the_animation() {
        let animation = LottieAnimation::parse(SLIDE.as_bytes()).unwrap();
        assert_eq!(animation.duration(), 2_000_000);
        assert_eq!((animation.width, animation.height), (100, 50));
        assert!((animation.frame_at(1_000_000) - 30.0).abs() < 1e-9);
        assert!(animation.frame_at(5_000_000) < 60.0);
        assert_eq!(animation.frame_at(-5), 0.0);
        assert_eq!(animation.fitted((1080, 1920)), (1080, 540));
        assert_eq!(animation.fitted((10_000, 10_000)), (2048, 1024));
    }

    #[test]
    fn json_that_is_not_lottie_is_refused() {
        assert!(LottieAnimation::parse(b"{\"hello\": 1}").is_err());
        assert!(LottieAnimation::parse(b"not json").is_err());
    }

    /// Render the middle frame of the Lottie file named by
    /// `CHUKCUT_LOTTIE_SAMPLE` to a PNG beside it, for looking at a real file
    /// (`cargo test -p chukcut-engine --lib lottie_sample -- --ignored`).
    #[test]
    #[ignore = "needs a file named by CHUKCUT_LOTTIE_SAMPLE"]
    fn lottie_sample() {
        let Ok(path) = std::env::var("CHUKCUT_LOTTIE_SAMPLE") else {
            return;
        };
        let ctx = crate::modules::gpu::render_context().expect("a GPU");
        let animation = LottieAnimation::open(Path::new(&path)).expect("parse");
        let size = animation.fitted((512, 512));
        let started = std::time::Instant::now();
        let rgba = animation
            .render_rgba(&ctx, animation.duration() / 2, size)
            .expect("render");
        eprintln!(
            "{}x{} {} µs long, {:.1} fps, rendered in {:?}",
            animation.width,
            animation.height,
            animation.duration(),
            animation.frame_rate,
            started.elapsed()
        );
        image::RgbaImage::from_raw(size.0, size.1, rgba)
            .unwrap()
            .save(format!("{path}.png"))
            .unwrap();
    }

    /// The GPU half: the box is where the animation says, at half alpha,
    /// with straight colour.
    #[test]
    fn a_frame_rasterises_where_the_animation_puts_it() {
        let Some(ctx) = crate::modules::gpu::render_context() else {
            eprintln!("no GPU; skipping");
            return;
        };
        let animation = LottieAnimation::parse(SLIDE.as_bytes()).unwrap();
        let at = |rgba: &[u8], x: usize, y: usize| {
            let i = (y * 100 + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        let start = animation.render_rgba(&ctx, 0, (100, 50)).unwrap();
        let red = at(&start, 25, 25);
        assert!(red[0] > 245 && red[1] < 10 && red[2] < 10, "{red:?}");
        assert!((red[3] as i32 - 128).abs() <= 2, "half alpha: {red:?}");
        assert_eq!(at(&start, 75, 25)[3], 0, "right half empty at the start");

        let end = animation.render_rgba(&ctx, 1_999_999, (100, 50)).unwrap();
        assert_eq!(at(&end, 25, 25)[3], 0, "left half empty at the end");
        assert!(at(&end, 75, 25)[3] > 120);
    }
}
