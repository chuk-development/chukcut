//! What compositing costs, isolated from decoding.
//!
//! The export and preview breakdowns in `docs/STATUS.md` both report a
//! `composite` figure, but both are measured with a decoder in the loop, and
//! `docs/STATUS.md` also records the decode stage swinging between 12.8 and
//! 76 ms per frame depending on what else the machine is doing. A composite
//! number measured next to that cannot be trusted to a factor of two.
//!
//! So this group decodes nothing. Every layer samples the same texture, uploaded
//! once before the clock starts, and what is left is exactly the thing being
//! asked about: per-segment uniform writes, draw calls, blending, and the
//! readback.
//!
//! Two axes, because they are the two questions a timeline raises:
//!
//! - **Layer count** (1, 3, 10). Whether cost is per-frame or per-layer decides
//!   whether a ten-clip timeline is playable.
//! - **Plain against transformed.** Transform, crop and keyframes are all
//!   resolved on the CPU in `render::layout` before anything is drawn, so the
//!   difference between the two rows is the cost of that resolution — including
//!   the keyframe sampling that happens per segment per frame.
//!
//! The transformed variant is deliberately built to cover *the same area of the
//! canvas* as the plain one. The first version of it scaled every layer to 55%
//! and rotated it, and measured **faster** than plain — because a compositor at
//! 1080p is fill-rate bound and a shrunken quad touches a third of the pixels.
//! A benchmark that reports "adding transforms made it faster" is worse than no
//! benchmark, so scale stays at 1, rotation at 0, and what differs is the alpha
//! blend, the crop, the flip, and five keyframes on three properties sampled per
//! segment per frame. If a future change makes the CPU-side layout expensive,
//! this pair is where it will show.
//!
//! The `GPU only` row exists because `render_frame` bundles two things that
//! move independently: the compositing itself, and the map-and-copy that brings
//! the result back to system memory. `docs/STATUS.md` measures the latter at
//! 4.4–5.7 ms for a 1080p frame, which on a ten-layer composite is most of the
//! total, and a single number would hide that. The row waits on the device
//! explicitly — `render_to_texture` only submits, so timing it without a poll
//! would measure command encoding and report a GPU-bound composite as free.

use std::sync::{Arc, Mutex};

use chukcut_engine::modules::project::{
    new_id, AnimatableProperty, CanvasConfig, ColorAdjustMaterial, Crop, Easing, Keyframe,
    KeyframeTrack, LutRef, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_engine::modules::render::{
    Compositor, RenderContext, SourceFrame, SourceProvider, SourceRequest,
};

use crate::harness::{rounds, Measurement};

pub const GROUP: &str = "composite";

/// A provider that hands every segment the same texture, uploaded once.
///
/// This is the whole point of the group: with a real provider the number would
/// be dominated by decoding, which the decode group already measures and which
/// this machine measures differently every time.
struct StillProvider {
    width: u32,
    height: u32,
    frame: Mutex<Option<SourceFrame>>,
}

impl StillProvider {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            frame: Mutex::new(None),
        }
    }

    /// Upload before timing starts. Doing it lazily inside `frame` would put a
    /// 8 MB upload in the first sample of every configuration.
    fn warm(&self, ctx: &RenderContext) {
        let mut slot = self.frame.lock().expect("still provider poisoned");
        if slot.is_some() {
            return;
        }
        // Detail rather than a flat fill: a flat texture is the same work for
        // the sampler, but a benchmark whose output is one colour cannot be
        // eyeballed for correctness at all.
        let mut data = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                let noise = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
                data.extend_from_slice(&[
                    ((x * 255 / self.width.max(1)) as u8).wrapping_add(noise / 4),
                    ((y * 255 / self.height.max(1)) as u8).wrapping_add(noise / 8),
                    noise,
                    255,
                ]);
            }
        }
        *slot = Some(upload(ctx, &data, self.width, self.height));
    }
}

impl SourceProvider for StillProvider {
    fn frame(
        &self,
        ctx: &RenderContext,
        _request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        self.warm(ctx);
        Ok(self.frame.lock().expect("still provider poisoned").clone())
    }
}

/// `Rgba8UnormSrgb`, matching `media::provider::upload_rgba`. Tagging the
/// texture sRGB is what makes the sampler linearize before blending, so a
/// benchmark that uploaded `Rgba8Unorm` would measure a subtly different
/// pipeline from the one that ships.
fn upload(ctx: &RenderContext, data: &[u8], width: u32, height: u32) -> SourceFrame {
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("bench still"),
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

/// `layers` clips, all visible at the same instant, each on its own track.
///
/// One track per layer rather than one track with many clips, because segments
/// on a track may not overlap — and the question is what N *simultaneously
/// visible* layers cost, which is what a title over a picture-in-picture over a
/// background is.
fn timeline(layers: usize, canvas: (u32, u32), fancy: bool) -> Project {
    let mut project = Project::new(
        "composite bench",
        CanvasConfig {
            width: canvas.0,
            height: canvas.1,
            background: [0.05, 0.05, 0.08, 1.0],
        },
        30.0,
    );
    const SPAN: Micros = 10_000_000;
    let material_id = new_id();
    project.materials.videos.push(VideoMaterial {
        id: material_id.clone(),
        path: "bench://still".into(),
        width: canvas.0,
        height: canvas.1,
        duration: SPAN,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });

    for layer in 0..layers {
        let mut track = Track::new(TrackKind::Video, format!("Layer {layer}"));
        let spread = layer as f32 / layers.max(1) as f32;
        track.segments.push(Segment {
            id: new_id(),
            material_id: material_id.clone(),
            target_range: TimeRange::new(0, SPAN),
            source_range: TimeRange::new(0, SPAN),
            render_index: layer as i32,
            speed: 1.0,
            volume: 1.0,
            transform: if fancy {
                Transform {
                    // Everything here is chosen to leave the covered area
                    // alone. `opacity` below 1 is the one that costs: it forces
                    // a real source-over blend where an opaque layer can be
                    // written straight through.
                    position: [(spread - 0.5) * 0.02, (0.5 - spread) * 0.02],
                    scale: [1.0, 1.0],
                    rotation: 0.0,
                    opacity: 0.9,
                    flip_h: layer % 2 == 1,
                    flip_v: false,
                }
            } else {
                Transform::default()
            },
            // A 2% margin: enough that the UV rectangle is not the identity and
            // `crop_uv` has real work, small enough that the quad still fills
            // the canvas.
            crop: fancy.then_some(Crop {
                left: 0.02,
                top: 0.02,
                right: 0.98,
                bottom: 0.98,
            }),
            extras: Vec::new(),
            keyframes: if fancy {
                vec![
                    keyframes(AnimatableProperty::PositionX, SPAN, -0.01, 0.01),
                    keyframes(AnimatableProperty::ScaleX, SPAN, 1.0, 1.02),
                    keyframes(AnimatableProperty::Opacity, SPAN, 0.85, 1.0),
                ]
            } else {
                Vec::new()
            },
        });
        project.tracks.push(track);
    }
    project
}

/// Five keyframes rather than two, so sampling has to search rather than hit
/// the first-or-last shortcut in `KeyframeTrack::sample`.
fn keyframes(property: AnimatableProperty, span: Micros, from: f32, to: f32) -> KeyframeTrack {
    const POINTS: usize = 5;
    KeyframeTrack {
        property,
        keyframes: (0..POINTS)
            .map(|n| {
                let t = n as f32 / (POINTS - 1) as f32;
                Keyframe {
                    time: (span as f64 * t as f64) as Micros,
                    value: from + (to - from) * t,
                    easing: Easing::EaseInOut,
                }
            })
            .collect(),
    }
}

/// A realistic 33-point warm LUT, written to the temp dir once per run.
///
/// 33 is the commonest size in shipped looks; the values are a gentle warm so
/// nothing about the table is degenerate (an identity LUT tempts a future
/// cache into special-casing it and benchmarking the special case).
fn warm_lut_file() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("chukcut-bench-warm-{}.cube", std::process::id()));
    let n = 33u32;
    let last = (n - 1) as f32;
    let mut text = format!("LUT_3D_SIZE {n}\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let (rf, gf, bf) = (r as f32 / last, g as f32 / last, b as f32 / last);
                text.push_str(&format!(
                    "{} {} {}\n",
                    (rf * 1.05).min(1.0),
                    gf.powf(0.98),
                    bf * 0.95
                ));
            }
        }
    }
    std::fs::write(&path, text).expect("write bench LUT");
    path
}

/// Attach a non-identity grade plus the LUT to every layer, so the row
/// measures the whole colour pass: encode, four scalar ops, eight
/// `textureLoad`s and the lerp, decode — per covered pixel per layer.
fn grade_and_lut(project: &mut Project, lut: &std::path::Path) {
    project.materials.color_adjusts.push(ColorAdjustMaterial {
        id: "bench-grade".into(),
        brightness: 0.05,
        contrast: 1.1,
        saturation: 0.9,
        temperature: 0.2,
        lut: Some(LutRef {
            path: lut.to_string_lossy().into_owned(),
            intensity: 0.8,
        }),
        grade: Default::default(),
    });
    for track in &mut project.tracks {
        for segment in &mut track.segments {
            segment.extras.push("bench-grade".into());
        }
    }
}

pub struct Budget {
    pub frames: usize,
    pub rounds: usize,
}

pub fn run(ctx: &Arc<RenderContext>, budget: &Budget) -> Vec<Measurement> {
    let mut out = Vec::new();
    let compositor = Compositor::new(Arc::clone(ctx));
    let canvas = (1920u32, 1080u32);
    let provider = StillProvider::new(canvas.0, canvas.1);
    provider.warm(ctx);

    for fancy in [false, true] {
        let shape = if fancy {
            "transform+crop+keyframes"
        } else {
            "plain"
        };
        for layers in [1usize, 3, 10] {
            let project = timeline(layers, canvas, fancy);
            let name = format!("{layers} layer(s), {shape}");

            // A whole frame, readback included: what the preview and the export
            // both pay per frame for the composite stage.
            let samples = rounds::<String>(budget.rounds, |_| {
                Ok(sweep(
                    &compositor,
                    &project,
                    &provider,
                    canvas,
                    budget.frames,
                    true,
                ))
            })
            .expect("compositing does not fail once the first frame has");
            compositor.reset_stats();
            let with_readback = Measurement::ms(GROUP, name.clone(), samples);

            // The same work stopping at the GPU texture, but waited on. The gap
            // between the two rows is the map-and-copy `docs/STATUS.md` puts at
            // ~5 ms for a 1080p frame.
            let samples = rounds::<String>(budget.rounds, |_| {
                Ok(sweep(
                    &compositor,
                    &project,
                    &provider,
                    canvas,
                    budget.frames,
                    false,
                ))
            })
            .expect("compositing does not fail once the first frame has");
            let stats = compositor.stats();
            compositor.reset_stats();

            let without =
                Measurement::ms(GROUP, format!("{name}, GPU only"), samples).with_note(format!(
                    "sources {:.2} ms, composite {:.2} ms",
                    stats.per_frame(stats.sources_ns) / 1e6,
                    stats.per_frame(stats.composite_ns) / 1e6
                ));
            out.push(with_readback);
            out.push(without);
        }
    }

    // The colour pass, priced against the `plain` rows above: the same
    // timeline with a non-identity grade *and* a 33-point LUT on every layer.
    // The delta against `plain` at the same layer count is what grading a
    // clip costs per frame — measured rather than asserted, because the LUT
    // is eight `textureLoad`s per covered pixel and "surely that is free" is
    // exactly the sentence that precedes a fill-rate regression.
    let lut = warm_lut_file();
    for layers in [1usize, 3, 10] {
        let mut project = timeline(layers, canvas, false);
        grade_and_lut(&mut project, &lut);
        let name = format!("{layers} layer(s), grade+lut");

        let samples = rounds::<String>(budget.rounds, |_| {
            Ok(sweep(
                &compositor,
                &project,
                &provider,
                canvas,
                budget.frames,
                true,
            ))
        })
        .expect("compositing does not fail once the first frame has");
        compositor.reset_stats();
        let with_readback = Measurement::ms(GROUP, name.clone(), samples);

        let samples = rounds::<String>(budget.rounds, |_| {
            Ok(sweep(
                &compositor,
                &project,
                &provider,
                canvas,
                budget.frames,
                false,
            ))
        })
        .expect("compositing does not fail once the first frame has");
        let stats = compositor.stats();
        compositor.reset_stats();

        let without =
            Measurement::ms(GROUP, format!("{name}, GPU only"), samples).with_note(format!(
                "sources {:.2} ms, composite {:.2} ms",
                stats.per_frame(stats.sources_ns) / 1e6,
                stats.per_frame(stats.composite_ns) / 1e6
            ));
        out.push(with_readback);
        out.push(without);
    }
    let _ = std::fs::remove_file(&lut);
    out
}

/// Milliseconds per frame across `frames` distinct instants.
///
/// Distinct instants, not the same one repeatedly: keyframe sampling and the
/// layout maths both depend on the time, and re-rendering one instant would let
/// a future cache make the benchmark report zero.
fn sweep(
    compositor: &Compositor,
    project: &Project,
    provider: &StillProvider,
    size: (u32, u32),
    frames: usize,
    readback: bool,
) -> f64 {
    let step = 33_333;
    // One untimed frame: the first acquires a render target and builds the
    // bind group layouts.
    let _ = compositor.render_frame(project, 0, size, provider);

    let started = std::time::Instant::now();
    for n in 0..frames {
        let at = n as Micros * step;
        if readback {
            let _ = compositor.render_frame(project, at, size, provider);
        } else {
            match compositor.render_to_texture(project, at, size, provider) {
                Ok(target) => {
                    // `render_to_texture` submits and returns. Without this
                    // wait the loop would measure command encoding and hand
                    // back a figure that stays flat as layers are added, which
                    // is precisely the wrong conclusion.
                    let _ = compositor
                        .context()
                        .device()
                        .poll(wgpu::PollType::wait_indefinitely());
                    compositor.pool().release(target);
                }
                Err(_) => break,
            }
        }
    }
    started.elapsed().as_secs_f64() * 1000.0 / frames.max(1) as f64
}
