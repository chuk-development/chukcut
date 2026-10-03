//! GPU tests for the built-in effects, run through the real compositor.
//!
//! Every expectation comes from a CPU reference written here from the same
//! description as the shader, or from a structural property (a flat picture
//! stays flat, a seeded effect repeats, an effect clip leaves what is above it
//! alone). Never from a previous render: a change to a shader fails a test
//! instead of agreeing with itself.
//!
//! The test sources are uploaded as `Rgba8UnormSrgb`, like the real decoder's
//! RGBA path, so a clip with no effect reads back byte-identical and every
//! difference is the effect's.

// Per-channel loops over `[f32; 4]` read better indexed.
#![allow(clippy::needless_range_loop)]

use std::collections::HashMap;

use parking_lot::Mutex;

use super::catalog;
use super::render::*;
use crate::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use crate::modules::project::effects::{EffectMaterial, EffectValue};
use crate::modules::render::{
    Compositor, CompositorConfig, Frame, RenderContext, SourceFrame, SourceProvider, SourceRequest,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Image {
    w: u32,
    h: u32,
    /// sRGB bytes, straight alpha, row major.
    px: Vec<[u8; 4]>,
}

impl Image {
    fn new(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Self {
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                px.push(f(x, y));
            }
        }
        Self { w, h, px }
    }
}

/// A smooth picture: a gradient with no edge sharper than one code value per
/// pixel, for tests that resample between texels.
fn gradient(w: u32, h: u32) -> Image {
    Image::new(w, h, |x, y| {
        [
            (x * 255 / (w - 1)) as u8,
            (y * 255 / (h - 1)) as u8,
            ((x + y) * 127 / (w + h - 2) + 64) as u8,
            255,
        ]
    })
}

/// A picture where every pixel differs, for tests that move whole texels.
fn busy(w: u32, h: u32) -> Image {
    Image::new(w, h, |x, y| {
        let v = pcg(x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663));
        [v as u8, (v >> 8) as u8, (v >> 16) as u8, 255]
    })
}

fn flat(w: u32, h: u32, c: [u8; 4]) -> Image {
    Image::new(w, h, |_, _| c)
}

#[derive(Default)]
struct Provider {
    images: HashMap<String, Image>,
    cache: Mutex<HashMap<String, SourceFrame>>,
}

impl Provider {
    fn with(mut self, id: &str, image: Image) -> Self {
        self.images.insert(id.to_string(), image);
        self
    }
}

impl SourceProvider for Provider {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        if let Some(hit) = self.cache.lock().get(request.material_id) {
            return Ok(Some(hit.clone()));
        }
        let Some(image) = self.images.get(request.material_id) else {
            return Ok(None);
        };
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("fx test source"),
            size: wgpu::Extent3d {
                width: image.w,
                height: image.h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: Vec<u8> = image.px.iter().flatten().copied().collect();
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.w * 4),
                rows_per_image: Some(image.h),
            },
            wgpu::Extent3d {
                width: image.w,
                height: image.h,
                depth_or_array_layers: 1,
            },
        );
        let frame = SourceFrame::from_texture(std::sync::Arc::new(texture));
        self.cache
            .lock()
            .insert(request.material_id.to_string(), frame.clone());
        Ok(Some(frame))
    }
}

fn compositor() -> Option<Compositor> {
    let ctx = crate::modules::render::test_context()?;
    Some(Compositor::with_config(
        ctx,
        CompositorConfig {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            strict_sources: true,
            ..Default::default()
        },
    ))
}

fn segment(id: &str, material: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(0, duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

fn add_video(project: &mut Project, id: &str, w: u32, h: u32) {
    project.materials.videos.push(VideoMaterial {
        id: id.into(),
        path: format!("/{id}.mp4"),
        width: w,
        height: h,
        duration: 60_000_000,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
}

/// One clip, `clip`, filling a `w`x`h` canvas for ten seconds.
fn one_clip(w: u32, h: u32) -> Project {
    let mut p = Project::new(
        "fx",
        CanvasConfig {
            width: w,
            height: h,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    add_video(&mut p, "clip", w, h);
    let mut track = Track::new(TrackKind::Video, "Main");
    track.segments.push(segment("s", "clip", 0, 10_000_000));
    p.tracks.push(track);
    p
}

fn effect(kind: &str, params: &[(&str, f32)], seed: u32) -> EffectMaterial {
    let mut e = EffectMaterial::new(kind);
    e.seed = seed;
    for (name, value) in params {
        e.params
            .insert((*name).to_string(), EffectValue::Number(*value));
    }
    e
}

/// Put `e` on segment `segment_id`.
fn attach(project: &mut Project, segment_id: &str, e: EffectMaterial) {
    let id = e.id.clone();
    project.materials.effects.push(e);
    project.segment_mut(segment_id).unwrap().extras.push(id);
}

fn render(c: &Compositor, p: &Project, time: Micros, provider: &Provider) -> Frame {
    c.render(p, time, (p.canvas.width, p.canvas.height), provider)
        .expect("render")
}

// ---------------------------------------------------------------------------
// The CPU side: sRGB, premultiplied linear pixels, and comparison
// ---------------------------------------------------------------------------

fn decode(e: u8) -> f32 {
    let e = e as f32 / 255.0;
    if e <= 0.04045 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let e = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (e * 255.0).round() as u8
}

/// A picture in the effect passes' space: linear light, premultiplied.
#[derive(Clone)]
struct Linear {
    w: u32,
    h: u32,
    px: Vec<[f32; 4]>,
}

impl Linear {
    fn of(image: &Image) -> Self {
        Self {
            w: image.w,
            h: image.h,
            px: image
                .px
                .iter()
                .map(|p| {
                    let a = p[3] as f32 / 255.0;
                    [decode(p[0]) * a, decode(p[1]) * a, decode(p[2]) * a, a]
                })
                .collect(),
        }
    }

    fn at(&self, x: i32, y: i32) -> [f32; 4] {
        let x = x.clamp(0, self.w as i32 - 1) as u32;
        let y = y.clamp(0, self.h as i32 - 1) as u32;
        self.px[(y * self.w + x) as usize]
    }

    /// Bilinear sample at a UV, texel centres at `(i + 0.5) / w`, clamped:
    /// what the GPU's linear sampler does.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u * self.w as f32 - 0.5;
        let y = v * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i32, y0 as i32);
        let mut out = [0.0; 4];
        for c in 0..4 {
            let a = self.at(x0, y0)[c] * (1.0 - fx) + self.at(x0 + 1, y0)[c] * fx;
            let b = self.at(x0, y0 + 1)[c] * (1.0 - fx) + self.at(x0 + 1, y0 + 1)[c] * fx;
            out[c] = a * (1.0 - fy) + b * fy;
        }
        out
    }

    fn map(&self, f: impl Fn(u32, u32) -> [f32; 4]) -> Self {
        let mut px = Vec::with_capacity(self.px.len());
        for y in 0..self.h {
            for x in 0..self.w {
                px.push(f(x, y));
            }
        }
        Self {
            w: self.w,
            h: self.h,
            px,
        }
    }

    /// The bytes the compositor writes: the layer source-over an opaque black
    /// background, encoded.
    fn over_black(&self) -> Vec<[u8; 4]> {
        self.px
            .iter()
            .map(|p| [encode(p[0]), encode(p[1]), encode(p[2]), 255])
            .collect()
    }
}

fn assert_frame(name: &str, frame: &Frame, expected: &[[u8; 4]], tolerance: i32) {
    let mut worst = (0, 0, 0);
    for y in 0..frame.height {
        for x in 0..frame.width {
            let got = frame.pixel(x, y);
            let want = expected[(y * frame.width + x) as usize];
            for c in 0..3 {
                let d = (got[c] as i32 - want[c] as i32).abs();
                if d > worst.0 {
                    worst = (d, x, y);
                }
            }
        }
    }
    let (d, x, y) = worst;
    assert!(
        d <= tolerance,
        "{name}: ({x},{y}) rendered {:?}, the reference says {:?} ({d} code values off)",
        frame.pixel(x, y),
        expected[(y * frame.width + x) as usize],
    );
}

fn pcg(v: u32) -> u32 {
    let state = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

fn resolve(e: &EffectMaterial, time: Micros) -> FxInstance {
    FxInstance::resolve(e, time, None).expect("not at rest")
}

macro_rules! gpu {
    () => {
        match compositor() {
            Some(c) => c,
            None => {
                eprintln!("skipping: no GPU adapter");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// The contract with the compositor
// ---------------------------------------------------------------------------

#[test]
fn an_effect_at_rest_or_switched_off_changes_no_byte() {
    let c = gpu!();
    let provider = Provider::default().with("clip", busy(64, 48));
    let plain = one_clip(64, 48);
    let before = render(&c, &plain, 0, &provider);

    let mut rest = plain.clone();
    attach(
        &mut rest,
        "s",
        effect(catalog::GAUSSIAN_BLUR, &[("radius", 0.0)], 1),
    );
    assert_eq!(render(&c, &rest, 0, &provider).data, before.data, "at rest");

    let mut off = plain.clone();
    let mut e = effect(catalog::PIXELATE, &[("size", 80.0)], 1);
    e.enabled = false;
    attach(&mut off, "s", e);
    assert_eq!(
        render(&c, &off, 0, &provider).data,
        before.data,
        "switched off"
    );

    let mut unknown = plain.clone();
    attach(&mut unknown, "s", effect("from_a_later_build", &[], 1));
    assert_eq!(
        render(&c, &unknown, 0, &provider).data,
        before.data,
        "unknown kind"
    );
}

#[test]
fn an_effect_chain_with_nothing_to_do_is_a_lossless_round_trip() {
    // Mirror "left onto right" over a picture that is already symmetric
    // leaves every pixel where it was, so the import and export passes alone
    // are measured: straight to premultiplied half floats and back.
    let c = gpu!();
    let image = Image::new(64, 48, |x, y| {
        let m = x.min(63 - x);
        [(m * 8) as u8, (y * 5) as u8, 200, 255]
    });
    let provider = Provider::default().with("clip", image.clone());
    let mut p = one_clip(64, 48);
    attach(&mut p, "s", effect(catalog::MIRROR, &[("mode", 0.0)], 1));
    let frame = render(&c, &p, 0, &provider);
    assert_frame("round trip", &frame, &image.px, 1);
}

// ---------------------------------------------------------------------------
// Against the CPU reference
// ---------------------------------------------------------------------------

#[test]
fn gaussian_blur_matches_the_cpu_reference() {
    let c = gpu!();
    let image = busy(80, 60);
    let provider = Provider::default().with("clip", image.clone());
    for radius in [10.0, 45.0, 100.0] {
        let mut p = one_clip(80, 60);
        attach(
            &mut p,
            "s",
            effect(catalog::GAUSSIAN_BLUR, &[("radius", radius)], 1),
        );
        let frame = render(&c, &p, 0, &provider);

        let sigma = blur_sigma(radius, 60.0);
        let (taps, stride) = blur_taps(sigma);
        let pass = |src: &Linear, dx: i32, dy: i32| {
            src.map(|x, y| {
                let mut sum = [0.0f32; 4];
                let mut total = 0.0;
                for k in -taps..=taps {
                    let d = (k * stride) as f32;
                    let w = (-(d * d) / (2.0 * sigma * sigma)).exp();
                    let s = src.at(x as i32 + dx * k * stride, y as i32 + dy * k * stride);
                    for c in 0..4 {
                        sum[c] += s[c] * w;
                    }
                    total += w;
                }
                sum.map(|v| v / total)
            })
        };
        let lin = Linear::of(&image);
        let expected = pass(&pass(&lin, 1, 0), 0, 1);
        assert_frame(&format!("blur {radius}"), &frame, &expected.over_black(), 2);
    }
}

#[test]
fn pixelate_matches_the_cpu_reference() {
    let c = gpu!();
    let image = busy(70, 50);
    let provider = Provider::default().with("clip", image.clone());
    for size in [10.0, 55.0] {
        let mut p = one_clip(70, 50);
        attach(&mut p, "s", effect(catalog::PIXELATE, &[("size", size)], 1));
        let frame = render(&c, &p, 0, &provider);
        let block = pixel_block(size, 50.0) as i32;
        let origin = (35 % block, 25 % block);
        let lin = Linear::of(&image);
        let expected = lin.map(|x, y| {
            let px = x as i32 - origin.0;
            let py = y as i32 - origin.1;
            let cx = (px as f32 / block as f32).floor() as i32;
            let cy = (py as f32 / block as f32).floor() as i32;
            lin.at(
                cx * block + block / 2 + origin.0,
                cy * block + block / 2 + origin.1,
            )
        });
        assert_frame(
            &format!("pixelate {size}"),
            &frame,
            &expected.over_black(),
            1,
        );
    }
}

#[test]
fn rgb_split_moves_red_and_blue_apart_by_the_reference_offset() {
    let c = gpu!();
    let image = busy(64, 64);
    let provider = Provider::default().with("clip", image.clone());
    let e = effect(catalog::RGB_SPLIT, &[("amount", 70.0), ("angle", 30.0)], 1);
    let o = split_offset(&resolve(&e, 0), 64.0).map(|v| v.round_ties_even() as i32);
    assert!(o != [0, 0], "the test needs a visible offset");
    let mut p = one_clip(64, 64);
    attach(&mut p, "s", e);
    let frame = render(&c, &p, 0, &provider);
    let lin = Linear::of(&image);
    let expected = lin.map(|x, y| {
        let (x, y) = (x as i32, y as i32);
        let r = lin.at(x - o[0], y - o[1]);
        let g = lin.at(x, y);
        let b = lin.at(x + o[0], y + o[1]);
        [r[0], g[1], b[2], r[3].max(g[3]).max(b[3])]
    });
    assert_frame("rgb split", &frame, &expected.over_black(), 1);
}

#[test]
fn every_mirror_mode_matches_the_cpu_reference() {
    let c = gpu!();
    let image = busy(64, 48);
    let provider = Provider::default().with("clip", image.clone());
    let lin = Linear::of(&image);
    for mode in 0..catalog::MIRROR_MODES.len() {
        let mut p = one_clip(64, 48);
        attach(
            &mut p,
            "s",
            effect(catalog::MIRROR, &[("mode", mode as f32)], 1),
        );
        let frame = render(&c, &p, 0, &provider);
        let expected = lin.map(|x, y| {
            let (mut x, mut y) = (x as i32, y as i32);
            let (fx, fy) = (63 - x, 47 - y);
            match mode {
                0 if x >= 32 => x = fx,
                1 if x < 32 => x = fx,
                2 if y >= 24 => y = fy,
                3 if y < 24 => y = fy,
                4 => {
                    if x >= 32 {
                        x = fx;
                    }
                    if y >= 24 {
                        y = fy;
                    }
                }
                _ => {}
            }
            lin.at(x, y)
        });
        assert_frame(
            catalog::MIRROR_MODES[mode],
            &frame,
            &expected.over_black(),
            1,
        );
    }
}

#[test]
fn letterbox_bars_cover_exactly_the_reference_rows() {
    let c = gpu!();
    let image = flat(96, 64, [200, 180, 160, 255]);
    let provider = Provider::default().with("clip", image.clone());
    let mut p = one_clip(96, 64);
    let mut e = effect(catalog::LETTERBOX, &[("aspect", 2.4), ("opacity", 50.0)], 1);
    e.params
        .insert("color".into(), EffectValue::Color([0.2, 0.0, 0.0, 1.0]));
    attach(&mut p, "s", e);
    let frame = render(&c, &p, 0, &provider);
    let (bar_h, bar_w) = letterbox_bars(2.4, (96, 64));
    assert_eq!(bar_w, 0.0);
    let lin = Linear::of(&image);
    let expected = lin.map(|x, y| {
        let v = (y as f32 + 0.5) / 64.0;
        let base = lin.at(x as i32, y as i32);
        if v < bar_h || v > 1.0 - bar_h {
            let a = 0.5;
            [
                0.2 * a + base[0] * (1.0 - a),
                base[1] * (1.0 - a),
                base[2] * (1.0 - a),
                1.0,
            ]
        } else {
            base
        }
    });
    assert_frame("letterbox", &frame, &expected.over_black(), 1);
    // And the bars are really there: a 3:2 frame cut to 2.4:1 loses 12 of
    // its 64 rows at the top and 12 at the bottom.
    assert!((bar_h * 64.0 - 12.0).abs() < 1e-4, "{bar_h}");
}

/// Shake and gate weave share the affine resample; this is its reference.
fn transformed(lin: &Linear, offset: [f32; 2], degrees: f32, zoom: f32) -> Linear {
    let (w, h) = (lin.w as f32, lin.h as f32);
    let (s, c) = degrees.to_radians().sin_cos();
    lin.map(|x, y| {
        let px = [
            x as f32 + 0.5 - w * 0.5 - offset[0],
            y as f32 + 0.5 - h * 0.5 - offset[1],
        ];
        let src = [
            (c * px[0] + s * px[1]) / zoom,
            (-s * px[0] + c * px[1]) / zoom,
        ];
        let u = (src[0] + w * 0.5) / w;
        let v = (src[1] + h * 0.5) / h;
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            [0.0; 4]
        } else {
            lin.sample(u, v)
        }
    })
}

#[test]
fn shake_moves_the_picture_by_the_seeded_jolt_for_its_instant() {
    let c = gpu!();
    let image = gradient(80, 60);
    let provider = Provider::default().with("clip", image.clone());
    let lin = Linear::of(&image);
    let e = effect(
        catalog::SHAKE,
        &[("amplitude", 80.0), ("rotation", 60.0), ("zoom", 20.0)],
        4242,
    );
    let mut p = one_clip(80, 60);
    attach(&mut p, "s", e.clone());
    let mut seen = Vec::new();
    for at in [0, 133_000, 1_250_000] {
        let (offset, degrees, zoom) = shake_at(&resolve(&e, at), 60.0);
        let frame = render(&c, &p, at, &provider);
        let expected = transformed(&lin, offset, degrees, zoom);
        assert_frame(&format!("shake at {at}"), &frame, &expected.over_black(), 3);
        seen.push((offset, degrees));
    }
    assert!(
        seen[0] != seen[1] && seen[1] != seen[2],
        "the shake must move"
    );
    // Same seed, same instant: same jolt. Another seed: another one.
    let again = shake_at(&resolve(&e, 133_000), 60.0);
    assert_eq!(again.0, seen[1].0);
    let mut other = e.clone();
    other.seed = 7;
    assert_ne!(shake_at(&resolve(&other, 133_000), 60.0).0, seen[1].0);
}

#[test]
fn gate_weave_drifts_by_a_fraction_of_a_pixel_and_matches_the_reference() {
    let c = gpu!();
    let image = gradient(80, 60);
    let provider = Provider::default().with("clip", image.clone());
    let lin = Linear::of(&image);
    let e = effect(catalog::GATE_WEAVE, &[("amount", 100.0)], 99);
    let mut p = one_clip(80, 60);
    attach(&mut p, "s", e.clone());
    let at = 700_000;
    let (offset, degrees, zoom) = weave_at(&resolve(&e, at), 60.0);
    assert!(
        offset[1].abs() < 0.5,
        "a weave is subtle at this size: {offset:?}"
    );
    let frame = render(&c, &p, at, &provider);
    assert_frame(
        "gate weave",
        &frame,
        &transformed(&lin, offset, degrees, zoom).over_black(),
        3,
    );
}

#[test]
fn light_sweep_adds_the_reference_band_where_there_is_picture() {
    let c = gpu!();
    let image = flat(96, 54, [90, 90, 90, 255]);
    let provider = Provider::default().with("clip", image.clone());
    let lin = Linear::of(&image);
    let e = effect(
        catalog::LIGHT_SWEEP,
        &[
            ("intensity", 80.0),
            ("width", 30.0),
            ("angle", 20.0),
            ("period", 2.0),
        ],
        1,
    );
    let mut p = one_clip(96, 54);
    attach(&mut p, "s", e.clone());
    for at in [700_000, 1_000_000] {
        let fx = resolve(&e, at);
        let pos = sweep_position(&fx);
        let (s, cs) = 20f32.to_radians().sin_cos();
        let half_diag = (96f32 * 96.0 + 54.0 * 54.0).sqrt() * 0.5;
        let expected = lin.map(|x, y| {
            let base = lin.at(x as i32, y as i32);
            let d = ((x as f32 + 0.5 - 48.0) * cs + (y as f32 + 0.5 - 27.0) * s) / half_diag;
            let t = ((d - pos).abs() / 0.18).clamp(0.0, 1.0);
            let band = 1.0 - t * t * (3.0 - 2.0 * t);
            let light = 1.2 * band * band;
            [base[0] + light, base[1] + light, base[2] + light, base[3]]
        });
        let frame = render(&c, &p, at, &provider);
        assert_frame(&format!("sweep at {at}"), &frame, &expected.over_black(), 2);
    }
}

#[test]
fn grain_matches_the_cpu_reference_and_changes_every_millisecond() {
    let c = gpu!();
    let image = flat(64, 48, [128, 110, 90, 255]);
    let provider = Provider::default().with("clip", image.clone());
    let lin = Linear::of(&image);
    let e = effect(catalog::GRAIN, &[("amount", 100.0), ("size", 2.0)], 77);
    let mut p = one_clip(64, 48);
    attach(&mut p, "s", e.clone());
    let reference = |at: Micros| {
        let ms = ((at / 1000).rem_euclid(1 << 16)) as u32;
        let cell = (2.0 * 48.0 / 1080.0f32).max(1.0);
        lin.map(|x, y| {
            let base = lin.at(x as i32, y as i32);
            let cx = ((x as f32 + 0.5) / cell).floor() as u32;
            let cy = ((y as f32 + 0.5) / cell).floor() as u32;
            let h1 = pcg(cx.wrapping_add(pcg(cy.wrapping_add(pcg(ms.wrapping_add(77))))));
            let h2 = pcg(h1);
            let n = (h1 & 65535) as f32 / 65535.0 + (h2 & 65535) as f32 / 65535.0 - 1.0;
            let k = 1.0 + n * 0.25;
            [base[0] * k, base[1] * k, base[2] * k, base[3]]
        })
    };
    let a = render(&c, &p, 1_000_000, &provider);
    assert_frame("grain", &a, &reference(1_000_000).over_black(), 2);
    let b = render(&c, &p, 1_001_000, &provider);
    assert_ne!(a.data, b.data, "grain is reseeded every millisecond");
    assert_eq!(a.data, render(&c, &p, 1_000_000, &provider).data);
}

#[test]
fn the_grain_strength_is_the_grade_s() {
    let wgsl = include_str!("shaders/fx.wgsl");
    let wanted = format!(
        "const GRAIN_STRENGTH: f32 = {:?};",
        crate::modules::render::grade::GRAIN_STRENGTH
    );
    assert!(wgsl.contains(&wanted), "fx.wgsl must say {wanted}");
}

/// The frame effect's signed distance, for the reference.
fn rounded_box(q: [f32; 2], half: [f32; 2], radius: f32) -> f32 {
    let r = radius.min(half[0].min(half[1]));
    let d = [q[0].abs() - half[0] + r, q[1].abs() - half[1] + r];
    (d[0].max(0.0).hypot(d[1].max(0.0))) + d[0].max(d[1]).min(0.0) - r
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[test]
fn a_framed_picture_in_picture_matches_the_rounded_rectangle_reference() {
    let c = gpu!();
    let (w, h) = (96u32, 96u32);
    let image = flat(48, 48, [40, 200, 90, 255]);
    let provider = Provider::default().with("clip", image.clone());
    let mut p = one_clip(w, h);
    p.materials.videos[0].width = 48;
    p.materials.videos[0].height = 48;
    {
        let s = p.segment_mut("s").unwrap();
        s.transform.scale = [0.5, 0.5];
        s.transform.position = [0.25, 0.25];
    }
    let mut e = effect(
        catalog::FRAME,
        &[
            ("radius", 50.0),
            ("border", 60.0),
            ("shadow", 80.0),
            ("shadow_blur", 20.0),
            ("shadow_distance", 40.0),
            ("shadow_angle", 45.0),
        ],
        1,
    );
    e.params.insert(
        "border_color".into(),
        EffectValue::Color([1.0, 1.0, 1.0, 1.0]),
    );
    attach(&mut p, "s", e);
    let frame = render(&c, &p, 0, &provider);

    // The clip: 48x48 on screen, centred at (60, 36) — a quarter of the
    // canvas right and up from the middle.
    let (centre, half) = ([60.0f32, 36.0], [24.0f32, 24.0]);
    let radius = 0.5 * 24.0;
    let border = 0.6 * 0.03 * 96.0;
    let distance = 0.4 * 0.05 * 96.0;
    let offset = [
        45f32.to_radians().cos() * distance,
        45f32.to_radians().sin() * distance,
    ];
    let soft = (0.2 * 0.06 * 96.0f32).max(0.5);
    let green = [decode(40), decode(200), decode(90)];
    let expected: Vec<[u8; 4]> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| {
            let px = [x as f32 + 0.5, y as f32 + 0.5];
            let q = [px[0] - centre[0], px[1] - centre[1]];
            let d = rounded_box(q, half, radius);
            let inside = (0.5 - d).clamp(0.0, 1.0);
            let ring = (0.5 - (-d - border)).clamp(0.0, 1.0);
            let clip: [f32; 4] = {
                let c = [
                    green[0] * (1.0 - ring) + ring,
                    green[1] * (1.0 - ring) + ring,
                    green[2] * (1.0 - ring) + ring,
                    1.0,
                ];
                c.map(|v| v * inside)
            };
            let ds = rounded_box([q[0] - offset[0], q[1] - offset[1]], half, radius);
            let sa = 0.8 * (1.0 - smoothstep(-soft, soft, ds));
            let out = [clip[0], clip[1], clip[2], clip[3] + sa * (1.0 - clip[3])];
            // Over the black background the shadow's black adds nothing.
            let _ = out[3];
            [encode(out[0]), encode(out[1]), encode(out[2]), 255]
        })
        .collect();
    assert_frame("frame", &frame, &expected, 3);
    // The corner of the clip's square is cut away, its middle is green, and
    // its edge is the white border.
    assert_eq!(
        frame.pixel(37, 13)[1],
        0,
        "corner {:?}",
        frame.pixel(37, 13)
    );
    assert!(frame.pixel(60, 36)[1] > 190);
    assert!(
        frame.pixel(60, 12)[0] > 240,
        "border {:?}",
        frame.pixel(60, 12)
    );
}

// ---------------------------------------------------------------------------
// Structural
// ---------------------------------------------------------------------------

fn mean(frame: &Frame) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for p in frame.data.chunks(4) {
        for c in 0..3 {
            sum[c] += p[c] as f32;
        }
    }
    let n = (frame.data.len() / 4) as f32;
    sum.map(|v| v / n)
}

#[test]
fn a_flat_picture_stays_flat_under_every_effect_that_only_moves_pixels() {
    let c = gpu!();
    let colour = [120, 140, 160, 255];
    let provider = Provider::default().with("clip", flat(64, 48, colour));
    let cases: [(&str, &[(&str, f32)]); 6] = [
        (catalog::GAUSSIAN_BLUR, &[("radius", 80.0)]),
        (catalog::ZOOM_BLUR, &[("strength", 90.0)]),
        (catalog::PIXELATE, &[("size", 60.0)]),
        (catalog::RGB_SPLIT, &[("amount", 80.0)]),
        (
            catalog::GLITCH,
            &[("intensity", 100.0), ("color_shift", 0.0)],
        ),
        (catalog::KALEIDOSCOPE, &[("segments", 7.0)]),
    ];
    for (kind, params) in cases {
        let mut p = one_clip(64, 48);
        attach(&mut p, "s", effect(kind, params, 3));
        let frame = render(&c, &p, 500_000, &provider);
        for px in frame.data.chunks(4) {
            for ch in 0..3 {
                assert!(
                    (px[ch] as i32 - colour[ch] as i32).abs() <= 1,
                    "{kind}: {px:?} where the picture was flat {colour:?}"
                );
            }
        }
    }
}

#[test]
fn glow_bloom_and_halation_leave_dark_pictures_alone_and_spread_light_from_bright_ones() {
    let c = gpu!();
    let (w, h) = (96u32, 64u32);
    let dark = flat(w, h, [40, 40, 40, 255]);
    // A small bright square in a dark frame.
    let spot = Image::new(w, h, |x, y| {
        if (44..52).contains(&x) && (28..36).contains(&y) {
            [255, 255, 255, 255]
        } else {
            [20, 20, 20, 255]
        }
    });
    for (kind, params, tint) in [
        (
            catalog::GLOW,
            vec![("intensity", 80.0), ("threshold", 50.0)],
            None,
        ),
        (
            catalog::BLOOM,
            vec![("amount", 80.0), ("threshold", 50.0)],
            None,
        ),
        (
            catalog::HALATION,
            vec![("amount", 100.0), ("threshold", 50.0)],
            Some("red"),
        ),
    ] {
        let mut p = one_clip(w, h);
        attach(&mut p, "s", effect(kind, &params, 1));
        let quiet = render(&c, &p, 0, &Provider::default().with("clip", dark.clone()));
        assert_frame(&format!("{kind} below threshold"), &quiet, &dark.px, 1);
        let lit = render(&c, &p, 0, &Provider::default().with("clip", spot.clone()));
        // Four pixels off the square's edge, light has arrived.
        let near = lit.pixel(55, 32);
        let far = lit.pixel(2, 2);
        assert!(
            near[1] > 30 || near[0] > 30,
            "{kind}: no light next to the spot: {near:?}"
        );
        assert!(
            far[0] <= 22 && far[1] <= 22,
            "{kind}: light reached the corner: {far:?}"
        );
        if tint == Some("red") {
            assert!(near[0] > near[2] + 5, "halation is red-orange: {near:?}");
        }
    }
}

#[test]
fn zoom_blur_streaks_towards_the_centre_and_not_across() {
    let c = gpu!();
    let (w, h) = (96u32, 96u32);
    // A bright dot to the right of the centre.
    let dot = Image::new(w, h, |x, y| {
        if (76..80).contains(&x) && (46..50).contains(&y) {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let provider = Provider::default().with("clip", dot);
    let mut p = one_clip(w, h);
    attach(
        &mut p,
        "s",
        effect(catalog::ZOOM_BLUR, &[("strength", 60.0)], 1),
    );
    let frame = render(&c, &p, 0, &provider);
    // On the line between the dot and the centre (outside the dot), light;
    // the same distance off that line, none.
    let along = frame.pixel(86, 48);
    let across = frame.pixel(78, 60);
    assert!(along[0] > 20, "no streak along the ray: {along:?}");
    assert!(across[0] < 5, "a streak across the ray: {across:?}");
}

#[test]
fn a_kaleidoscope_of_two_is_mirror_symmetric() {
    let c = gpu!();
    let provider = Provider::default().with("clip", busy(64, 64));
    let mut p = one_clip(64, 64);
    attach(
        &mut p,
        "s",
        effect(catalog::KALEIDOSCOPE, &[("segments", 2.0)], 1),
    );
    let frame = render(&c, &p, 0, &provider);
    for y in 0..64 {
        for x in 0..64 {
            let a = frame.pixel(x, y);
            let b = frame.pixel(63 - x, y);
            for ch in 0..3 {
                assert!(
                    (a[ch] as i32 - b[ch] as i32).abs() <= 2,
                    "({x},{y}) {a:?} against its mirror {b:?}"
                );
            }
        }
    }
}

#[test]
fn glitch_and_vhs_repeat_for_a_seed_and_differ_for_another() {
    let c = gpu!();
    let provider = Provider::default().with("clip", busy(96, 64));
    for kind in [catalog::GLITCH, catalog::VHS] {
        let params: &[(&str, f32)] = &[("intensity", 100.0)];
        let mut a = one_clip(96, 64);
        attach(&mut a, "s", effect(kind, params, 11));
        let mut b = one_clip(96, 64);
        attach(&mut b, "s", effect(kind, params, 12));
        let plain = render(&c, &one_clip(96, 64), 400_000, &provider);
        let first = render(&c, &a, 400_000, &provider);
        assert_ne!(first.data, plain.data, "{kind} changed nothing");
        assert_eq!(
            first.data,
            render(&c, &a, 400_000, &provider).data,
            "{kind} is not repeatable"
        );
        assert_ne!(
            first.data,
            render(&c, &b, 400_000, &provider).data,
            "{kind} ignores its seed"
        );
        // Not a different picture, a damaged one: the colour balance holds.
        let (m0, m1) = (mean(&plain), mean(&first));
        for ch in 0..3 {
            assert!(
                (m0[ch] - m1[ch]).abs() < 20.0,
                "{kind}: mean moved {m0:?} -> {m1:?}"
            );
        }
    }
    // A glitch holds still between its jumps.
    let mut g = one_clip(96, 64);
    attach(
        &mut g,
        "s",
        effect(catalog::GLITCH, &[("intensity", 100.0), ("speed", 4.0)], 5),
    );
    assert_eq!(
        render(&c, &g, 1_010_000, &provider).data,
        render(&c, &g, 1_200_000, &provider).data
    );
    assert_ne!(
        render(&c, &g, 1_200_000, &provider).data,
        render(&c, &g, 1_300_000, &provider).data
    );
}

#[test]
fn keyframes_animate_a_parameter_over_the_clip() {
    let c = gpu!();
    let provider = Provider::default().with("clip", busy(64, 48));
    let mut p = one_clip(64, 48);
    let mut e = effect(catalog::GAUSSIAN_BLUR, &[], 1);
    let mut keys = Vec::new();
    crate::modules::project::effects::put_keyframe(
        &mut keys,
        0,
        0.0,
        crate::modules::project::document::Easing::Linear,
    );
    crate::modules::project::effects::put_keyframe(
        &mut keys,
        2_000_000,
        60.0,
        crate::modules::project::document::Easing::Linear,
    );
    e.keyframes.insert("radius".into(), keys);
    attach(&mut p, "s", e);
    let sharp = render(&c, &one_clip(64, 48), 0, &provider);
    assert_eq!(
        render(&c, &p, 0, &provider).data,
        sharp.data,
        "radius 0 at the head"
    );
    let mut fixed = one_clip(64, 48);
    attach(
        &mut fixed,
        "s",
        effect(catalog::GAUSSIAN_BLUR, &[("radius", 30.0)], 1),
    );
    assert_eq!(
        render(&c, &p, 1_000_000, &provider).data,
        render(&c, &fixed, 1_000_000, &provider).data,
        "halfway through the ramp is radius 30"
    );
}

// ---------------------------------------------------------------------------
// Effect clips, transitions, decode paths and the export
// ---------------------------------------------------------------------------

/// Three lanes: a full-frame clip, a small clip over it, an effect lane over
/// both, and a fourth lane above the effect lane with a clip of its own.
fn stacked() -> (Project, Provider) {
    let mut p = one_clip(64, 48);
    add_video(&mut p, "small", 16, 16);
    add_video(&mut p, "top", 16, 16);
    let mut middle = Track::new(TrackKind::Video, "Middle");
    let mut small = segment("small-s", "small", 0, 10_000_000);
    small.transform.scale = [0.25, 0.25];
    small.transform.position = [-0.5, 0.0];
    small.render_index = 1;
    middle.segments.push(small);
    p.tracks.push(middle);
    let mut lane = Track::new(TrackKind::Effect, "Effects 1");
    let e = effect(catalog::MIRROR, &[("mode", 0.0)], 1);
    let mut clip = segment("fx-s", &e.id, 1_000_000, 2_000_000);
    clip.render_index = 2;
    p.materials.effects.push(e);
    lane.segments.push(clip);
    p.tracks.push(lane);
    let mut over = Track::new(TrackKind::Video, "Over");
    let mut top = segment("top-s", "top", 0, 10_000_000);
    top.transform.scale = [0.25, 0.25];
    top.transform.position = [-0.5, 0.5];
    top.render_index = 3;
    over.segments.push(top);
    p.tracks.push(over);
    let provider = Provider::default()
        .with("clip", flat(64, 48, [30, 30, 90, 255]))
        .with("small", flat(16, 16, [250, 40, 40, 255]))
        .with("top", flat(16, 16, [40, 250, 40, 255]));
    (p, provider)
}

#[test]
fn an_effect_clip_applies_to_everything_beneath_it_and_nothing_above() {
    let c = gpu!();
    let (p, provider) = stacked();
    let before = render(&c, &p, 500_000, &provider);
    let during = render(&c, &p, 1_500_000, &provider);
    // Before the effect clip starts: the red clip on the left only.
    assert!(before.pixel(16, 24)[0] > 200);
    assert!(before.pixel(47, 24)[0] < 60);
    // During it: the composite beneath is mirrored left onto right, so the
    // red clip appears on the right too...
    assert!(during.pixel(16, 24)[0] > 200);
    assert!(during.pixel(47, 24)[0] > 200, "{:?}", during.pixel(47, 24));
    // ...and the green clip above the effect lane is not mirrored.
    assert!(during.pixel(16, 12)[1] > 200);
    assert!(during.pixel(47, 12)[1] < 60, "{:?}", during.pixel(47, 12));
    // An effect clip is not a missing clip and validates.
    assert!(p
        .validate()
        .iter()
        .all(|i| !i.message.contains("unknown material")));
    assert!(crate::modules::export::job::missing_media(&p)
        .iter()
        .all(|l| !l.contains("removed from the project")));
}

#[test]
fn a_hidden_effect_lane_applies_nothing() {
    let c = gpu!();
    let (mut p, provider) = stacked();
    let lane = p
        .tracks
        .iter_mut()
        .find(|t| t.kind == TrackKind::Effect)
        .unwrap();
    lane.hidden = true;
    let during = render(&c, &p, 1_500_000, &provider);
    assert!(during.pixel(47, 24)[0] < 60);
}

#[test]
fn the_preview_and_the_export_agree_on_a_frame_with_effects() {
    let c = gpu!();
    let (mut p, provider) = stacked();
    attach(
        &mut p,
        "small-s",
        effect(catalog::GAUSSIAN_BLUR, &[("radius", 40.0)], 1),
    );
    attach(&mut p, "s", effect(catalog::GRAIN, &[("amount", 60.0)], 1));
    let at = 1_500_000;
    let preview = c.render(&p, at, (64, 48), &provider).expect("preview");
    let Ok(export) = c.render_nv12(&p, at, (64, 48), &provider) else {
        eprintln!("skipping: no RGBA to NV12 compute pass on this device");
        return;
    };
    // 2x2 block means: grain is per pixel and NV12 chroma per 2x2 block.
    for by in (0..48).step_by(2) {
        for bx in (0..64).step_by(2) {
            let mut a = [0.0f32; 3];
            let mut b = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let pp = preview.pixel(bx + dx, by + dy);
                let (cb, cr) = export.chroma(bx + dx, by + dy);
                let y = (export.luma(bx + dx, by + dy) as f32 - 16.0) * (255.0 / 219.0);
                let cb = (cb as f32 - 128.0) * (255.0 / 224.0);
                let cr = (cr as f32 - 128.0) * (255.0 / 224.0);
                let rgb = [
                    y + 1.402 * cr,
                    y - 0.344_136 * cb - 0.714_136 * cr,
                    y + 1.772 * cb,
                ];
                for ch in 0..3 {
                    a[ch] += pp[ch] as f32 / 4.0;
                    b[ch] += rgb[ch].clamp(0.0, 255.0) / 4.0;
                }
            }
            for ch in 0..3 {
                assert!(
                    (a[ch] - b[ch]).abs() <= 4.0,
                    "block ({bx},{by}) channel {ch}: preview {a:?} export {b:?}"
                );
            }
        }
    }
}

#[test]
fn a_transition_carries_each_side_s_effects_into_its_layer() {
    use crate::modules::project::document::{TransitionKind, TransitionMaterial};
    let c = gpu!();
    let mut p = one_clip(64, 48);
    add_video(&mut p, "b", 64, 48);
    p.tracks[0].segments[0].target_range = TimeRange::new(0, 2_000_000);
    p.tracks[0].segments[0].source_range = TimeRange::new(0, 2_000_000);
    let mut right = segment("r", "b", 2_000_000, 2_000_000);
    let t = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
    right.extras.push(t.id.clone());
    p.materials.transitions.push(t);
    p.tracks[0].segments.push(right);
    let image = busy(64, 48);
    let provider = Provider::default()
        .with("clip", image.clone())
        .with("b", flat(64, 48, [0, 0, 0, 255]));
    // Just inside the window the outgoing side dominates; mirror it.
    let mut plain = p.clone();
    plain.tracks[0].segments[0].extras.clear();
    attach(&mut p, "s", effect(catalog::MIRROR, &[("mode", 1.0)], 1));
    let at = 1_500_000;
    let with = render(&c, &p, at, &provider);
    let without = render(&c, &plain, at, &provider);
    assert_ne!(with.data, without.data);
    // Mirrored right onto left: the two halves of the outgoing side match.
    for y in [5u32, 20, 40] {
        let a = with.pixel(3, y);
        let b = with.pixel(60, y);
        for ch in 0..3 {
            assert!((a[ch] as i32 - b[ch] as i32).abs() <= 1, "{a:?} {b:?}");
        }
    }
}

#[test]
fn a_library_transition_renders_through_the_compositor_and_the_export_agrees() {
    use crate::modules::project::document::TransitionMaterial;
    let c = gpu!();
    let mut p = one_clip(64, 48);
    add_video(&mut p, "b", 64, 48);
    p.tracks[0].segments[0].target_range = TimeRange::new(0, 2_000_000);
    p.tracks[0].segments[0].source_range = TimeRange::new(0, 2_000_000);
    let mut right = segment("r", "b", 2_000_000, 2_000_000);
    for preset in ["seamless:zoom_in", "gl:cube", "gl:wipeleft"] {
        right.extras.clear();
        p.materials.transitions.clear();
        let t = TransitionMaterial::library(preset, 1_000_000);
        right.extras.push(t.id.clone());
        p.materials.transitions.push(t);
        if p.tracks[0].segments.len() == 1 {
            p.tracks[0].segments.push(right.clone());
        } else {
            p.tracks[0].segments[1] = right.clone();
        }
        let provider = Provider::default()
            .with("clip", flat(64, 48, [220, 60, 40, 255]))
            .with("b", flat(64, 48, [30, 90, 220, 255]));
        let at = 2_000_000;
        let preview = c.render(&p, at, (64, 48), &provider).expect("preview");
        let Ok(export) = c.render_nv12(&p, at, (64, 48), &provider) else {
            eprintln!("skipping: no RGBA to NV12 compute pass on this device");
            return;
        };
        // Mid-window, neither clip alone fills the frame.
        let mut saw = [false, false];
        for px in preview.data.chunks(4) {
            saw[0] |= px[0] > 150;
            saw[1] |= px[2] > 150;
        }
        assert!(saw[0] || saw[1], "{preset}: a blank frame");
        for (x, y) in [(2u32, 2u32), (32, 24), (60, 44)] {
            let pp = preview.pixel(x, y);
            let (cb, cr) = export.chroma(x, y);
            let yy = (export.luma(x, y) as f32 - 16.0) * (255.0 / 219.0);
            let cb = (cb as f32 - 128.0) * (255.0 / 224.0);
            let cr = (cr as f32 - 128.0) * (255.0 / 224.0);
            let rgb = [
                yy + 1.402 * cr,
                yy - 0.344_136 * cb - 0.714_136 * cr,
                yy + 1.772 * cb,
            ];
            for ch in 0..3 {
                assert!(
                    (pp[ch] as f32 - rgb[ch].clamp(0.0, 255.0)).abs() <= 12.0,
                    "{preset} at ({x},{y}): preview {pp:?} export {rgb:?}"
                );
            }
        }
    }
}

/// What a frame costs with effects on it, for STATUS. Not a gate: run with
/// `--ignored --nocapture` on a quiet machine.
#[test]
#[ignore]
fn measure_effect_cost_per_frame() {
    let c = gpu!();
    let (w, h) = (1080u32, 1920u32);
    let provider = Provider::default().with("clip", gradient(w, h));
    let time = |p: &Project| {
        for _ in 0..3 {
            render(&c, p, 500_000, &provider);
        }
        let started = std::time::Instant::now();
        for i in 0..20 {
            render(&c, p, 500_000 + i * 33_333, &provider);
        }
        started.elapsed().as_secs_f64() * 1000.0 / 20.0
    };
    let base = time(&one_clip(w, h));
    println!("1080x1920, no effect: {base:.1} ms/frame (render + readback)");
    for kind in catalog::catalog().iter().map(|d| d.id) {
        let mut p = one_clip(w, h);
        attach(&mut p, "s", effect(kind, &[], 1));
        if FxInstance::resolve(&p.materials.effects[0], 0, None).is_none() {
            continue;
        }
        println!("  + {kind}: {:.1} ms/frame", time(&p) - base);
    }
}

// ---------------------------------------------------------------------------
// Faint alpha on an 8-bit target
// ---------------------------------------------------------------------------
//
// An 8-bit blender may round its blend factors to the target's precision:
// asked for `SrcAlpha, OneMinusSrcAlpha`, NVIDIA rounds the source alpha of an
// `Rgba8UnormSrgb` target to 1/255 before the multiply. The over draw
// therefore premultiplies in the shader. These tests hold it to the CPU
// arithmetic at alphas that fall between the 1/255 steps.

/// Alphas between the 1/255 steps, and some ordinary ones.
const FAINT: [f32; 8] = [
    0.0018,
    1.4 / 255.0,
    2.6 / 255.0,
    4.45 / 255.0,
    0.03,
    0.1,
    0.5,
    1.0,
];

/// `v` as half-float bits, rounded to nearest. Normal values only, which is
/// all these tests write.
fn f16_bits(v: f32) -> u16 {
    if v == 0.0 {
        return 0;
    }
    let bits = v.to_bits();
    let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    assert!((1..31).contains(&exponent), "{v} is not a normal half");
    let mantissa = ((bits & 0x7f_ffff) + 0x1000) >> 13;
    let (exponent, mantissa) = if mantissa == 0x400 {
        (exponent + 1, 0)
    } else {
        (exponent, mantissa)
    };
    ((exponent as u16) << 10) | mantissa as u16
}

fn f16_value(h: u16) -> f32 {
    if h == 0 {
        return 0.0;
    }
    let exponent = ((h >> 10) & 0x1f) as i32 - 15;
    let mantissa = (h & 0x3ff) as f32 / 1024.0;
    2f32.powi(exponent) * (1.0 + mantissa)
}

/// The over draw of a finished layer, from an `Rgba16Float` layer (whose
/// alpha is not held to 1/255 the way an 8-bit layer's is) onto the
/// compositor's `Rgba8UnormSrgb` target cleared to black: each pixel must be
/// the layer's light times its alpha, encoded.
#[test]
fn an_over_draw_of_a_faint_float_layer_matches_the_cpu_reference() {
    let Some(ctx) = crate::modules::render::test_context() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    use crate::modules::render::texture_pool::{TextureKey, TexturePool, DEFAULT_BUDGET_BYTES};
    const SRGB: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
    let w = FAINT.len() as u32;
    let colours: [[u8; 3]; 2] = [[255, 255, 255], [230, 180, 40]];
    let h = colours.len() as u32;

    // Straight alpha, linear light, as halves.
    let mut want = Vec::new();
    let mut bytes = Vec::new();
    for colour in colours {
        for alpha in FAINT {
            let texel = [
                f16_bits(decode(colour[0])),
                f16_bits(decode(colour[1])),
                f16_bits(decode(colour[2])),
                f16_bits(alpha),
            ];
            let a = f16_value(texel[3]);
            want.push([
                encode(f16_value(texel[0]) * a),
                encode(f16_value(texel[1]) * a),
                encode(f16_value(texel[2]) * a),
            ]);
            for half in texel {
                bytes.extend_from_slice(&half.to_le_bytes());
            }
        }
    }
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let layer = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("fx faint layer"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &layer,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 8),
            rows_per_image: Some(h),
        },
        size,
    );
    let layer_view = layer.create_view(&Default::default());

    let fx = FxRenderer::new(&ctx);
    let pool = TexturePool::new(DEFAULT_BUDGET_BYTES);
    let target = pool.acquire(
        ctx.device(),
        TextureKey::new(
            w,
            h,
            SRGB,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        ),
    );
    let mut frame = fx.begin(&ctx, &pool);
    let over = frame.prepare_over(&layer_view, SRGB, (w, h));
    let mut encoder = ctx.device().create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("fx faint over"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view(),
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        over.draw(&mut pass);
    }
    // Rows of 256 bytes: the copy alignment, and more than `w * 4`.
    const PADDED: u32 = 256;
    let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("fx faint readback"),
        size: (PADDED * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: target.texture(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PADDED),
                rows_per_image: Some(h),
            },
        },
        size,
    );
    ctx.queue().submit(Some(encoder.finish()));
    frame.finish();
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = slice.get_mapped_range().unwrap();

    let mut failures = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let at = (y * PADDED + x * 4) as usize;
            let got = [mapped[at], mapped[at + 1], mapped[at + 2]];
            let wanted = want[(y * w + x) as usize];
            if (0..3).any(|i| (got[i] as i32 - wanted[i] as i32).abs() > 1) {
                failures.push(format!(
                    "{:?} at alpha {:.2}/255: got {got:?}, want {wanted:?}",
                    colours[y as usize],
                    FAINT[x as usize] * 255.0
                ));
            }
        }
    }
    drop(mapped);
    buffer.unmap();
    pool.release(target);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same through the compositor: an effected clip at a faint opacity over
/// the black background. Its layer is the compositor's 8-bit format, so the
/// opacity lands on the 1/255 grid there, on every adapter, before the over
/// draw sees it; what the over draw adds must be exactly that.
#[test]
fn an_effected_clip_at_a_faint_opacity_matches_the_cpu_reference() {
    let c = gpu!();
    let colour = [230, 180, 40, 255];
    let provider = Provider::default().with("clip", flat(32, 16, colour));
    for opacity in FAINT {
        let mut p = one_clip(32, 16);
        p.segment_mut("s").unwrap().transform.opacity = opacity;
        attach(&mut p, "s", effect(catalog::PIXELATE, &[("size", 60.0)], 3));
        let frame = render(&c, &p, 500_000, &provider);
        let a = (opacity * 255.0).round() / 255.0;
        let want = [0, 1, 2].map(|i| encode(decode(colour[i]) * a));
        let got = &frame.data[(8 * 32 + 16) * 4..][..3];
        for i in 0..3 {
            assert!(
                (got[i] as i32 - want[i] as i32).abs() <= 1,
                "opacity {:.2}/255: got {got:?}, want {want:?}",
                opacity * 255.0
            );
        }
    }
}
