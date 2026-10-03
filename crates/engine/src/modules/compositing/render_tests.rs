//! GPU tests for masks, the chroma key and blend modes, through the real
//! compositor, on both decode paths.
//!
//! Every expectation comes from the CPU references in `render::matte` and
//! `render::blend`, or from a structural property, never from an earlier
//! render. Sources are uploaded as `Rgba8UnormSrgb` (the decoder's RGBA path)
//! or as full-range BT.709 NV12 planes (the hardware path).

#![allow(clippy::needless_range_loop)]

use std::sync::Arc;

use crate::modules::project::compositing::{
    BlendMode, ChromaKey, CompositingMaterial, Mask, MaskOp, MaskShape,
};
use crate::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use crate::modules::render::blend;
use crate::modules::render::matte::{self, MatteBlock};
use crate::modules::render::{
    Compositor, Frame, RenderContext, SourceFrame, SourceProvider, SourceRequest, YuvMatrix,
    YuvRange,
};

const W: u32 = 128;
const H: u32 = 96;

macro_rules! gpu {
    () => {
        match crate::modules::render::test_context() {
            Some(ctx) => Compositor::new(ctx),
            None => {
                eprintln!("skipping: no GPU adapter");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// A picture as sRGB bytes, and which path it is uploaded through.
#[derive(Clone)]
struct Picture {
    px: Vec<[u8; 3]>,
    planar: bool,
}

impl Picture {
    fn new(planar: bool, f: impl Fn(u32, u32) -> [u8; 3]) -> Self {
        let mut px = Vec::with_capacity((W * H) as usize);
        for y in 0..H {
            for x in 0..W {
                px.push(f(x, y));
            }
        }
        Self { px, planar }
    }

    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        self.px[(y * W + x) as usize]
    }
}

/// Full-range BT.709 Y'CbCr of encoded RGB, as bytes.
fn ycbcr(c: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = c.map(|v| v as f32 / 255.0);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    [y, (b - y) / 1.8556 + 0.5, (r - y) / 1.5748 + 0.5]
}

#[derive(Default)]
struct Provider {
    pictures: std::collections::HashMap<String, Picture>,
}

impl Provider {
    fn with(mut self, id: &str, picture: Picture) -> Self {
        self.pictures.insert(id.into(), picture);
        self
    }
}

fn texture(
    ctx: &RenderContext,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    bytes: &[u8],
) -> Arc<wgpu::Texture> {
    let texel = (bytes.len() as u32) / (w * h);
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("compositing test source"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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
        bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * texel),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    Arc::new(texture)
}

impl SourceProvider for Provider {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        let Some(picture) = self.pictures.get(request.material_id) else {
            return Ok(None);
        };
        if !picture.planar {
            let bytes: Vec<u8> = picture
                .px
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect();
            return Ok(Some(SourceFrame::from_texture(texture(
                ctx,
                W,
                H,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                &bytes,
            ))));
        }
        let luma: Vec<u8> = picture
            .px
            .iter()
            .map(|p| (ycbcr(*p)[0] * 255.0).round() as u8)
            .collect();
        // Chroma per 2x2 block: the tests use pictures that are flat over
        // every block, so the top-left sample is the block.
        let mut chroma = Vec::with_capacity((W * H / 2) as usize);
        for y in (0..H).step_by(2) {
            for x in (0..W).step_by(2) {
                let c = ycbcr(picture.at(x, y));
                chroma.push((c[1] * 255.0).round().clamp(0.0, 255.0) as u8);
                chroma.push((c[2] * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
        Ok(Some(SourceFrame::from_planes(
            texture(ctx, W, H, wgpu::TextureFormat::R8Unorm, &luma),
            texture(ctx, W / 2, H / 2, wgpu::TextureFormat::Rg8Unorm, &chroma),
            YuvMatrix::Bt709,
            YuvRange::Full,
            0,
            None,
        )))
    }
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

fn segment(id: &str, material: &str) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(0, 4_000_000),
        source_range: TimeRange::new(0, 4_000_000),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// `bottom` on the main lane and, when given, `top` on a lane above it, both
/// filling the canvas. Background black.
fn project(materials: &[&str]) -> Project {
    let mut p = Project::new(
        "compositing",
        CanvasConfig {
            width: W,
            height: H,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    for (i, id) in materials.iter().enumerate() {
        p.materials.videos.push(VideoMaterial {
            id: (*id).into(),
            path: format!("/{id}.mp4"),
            width: W,
            height: H,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, format!("V{i}"));
        let mut s = segment(&format!("s{i}"), id);
        s.render_index = i as i32;
        track.segments.push(s);
        p.tracks.push(track);
    }
    p
}

fn attach(p: &mut Project, segment_id: &str, mut m: CompositingMaterial) {
    m.id = format!("comp-{segment_id}");
    let id = m.id.clone();
    p.materials.compositing.retain(|c| c.id != id);
    p.materials.compositing.push(m);
    let s = p.segment_mut(segment_id).unwrap();
    if !s.extras.contains(&id) {
        s.extras.push(id);
    }
}

fn render(c: &Compositor, p: &Project, provider: &Provider) -> Frame {
    c.render(p, 1_000_000, (W, H), provider).expect("render")
}

fn with_masks(masks: Vec<Mask>) -> CompositingMaterial {
    CompositingMaterial {
        masks,
        ..CompositingMaterial::new()
    }
}

fn decode(e: f32) -> f32 {
    blend::decode(e)
}

fn encode_byte(l: f32) -> i32 {
    (blend::encode(l) * 255.0).round() as i32
}

fn close(name: &str, got: [u8; 4], want: [i32; 3], tolerance: i32) {
    for c in 0..3 {
        assert!(
            (got[c] as i32 - want[c]).abs() <= tolerance,
            "{name}: rendered {got:?}, the reference says {want:?}"
        );
    }
}

/// The two decode paths, each with a flat colour that is the same on both
/// (as near as 8-bit YCbCr allows; the planar expectation is read back from
/// an unmasked render, so it is exact).
fn flat_sources(colour: [u8; 3]) -> Vec<(&'static str, Picture)> {
    vec![
        ("rgba", Picture::new(false, |_, _| colour)),
        ("nv12", Picture::new(true, |_, _| colour)),
    ]
}

// ---------------------------------------------------------------------------
// At rest
// ---------------------------------------------------------------------------

/// A material whose every part is off renders byte-identical to no material,
/// on both paths; so does a blend mode this build does not know.
#[test]
fn nothing_switched_on_changes_a_byte() {
    let c = gpu!();
    for (name, picture) in flat_sources([200, 120, 60]) {
        let provider = Provider::default().with("a", picture);
        let mut p = project(&["a"]);
        let before = render(&c, &p, &provider);

        let mut off = Mask::new(MaskShape::Ellipse);
        off.enabled = false;
        let mut key = ChromaKey::new([0.0, 1.0, 0.0]);
        key.enabled = false;
        attach(
            &mut p,
            "s0",
            CompositingMaterial {
                masks: vec![off, Mask::new(MaskShape::Other("spiral".into()))],
                key: Some(key),
                blend: BlendMode::Other("vivid_light".into()),
                ..CompositingMaterial::new()
            },
        );
        let after = render(&c, &p, &provider);
        assert_eq!(
            before.data, after.data,
            "{name}: a resting material moved pixels"
        );
    }
}

// ---------------------------------------------------------------------------
// Masks
// ---------------------------------------------------------------------------

fn mask_cases() -> Vec<(&'static str, Vec<Mask>)> {
    let mut cases = Vec::new();
    let mut m = Mask::new(MaskShape::Ellipse);
    m.width = 0.7;
    m.height = 0.4;
    m.x = 0.1;
    m.y = -0.05;
    m.rotation = 30.0;
    m.feather = 0.15;
    cases.push(("feathered ellipse", vec![m]));
    let mut m = Mask::new(MaskShape::Rectangle);
    m.roundness = 0.6;
    m.rotation = -20.0;
    m.width = 0.9;
    m.height = 0.5;
    cases.push(("rounded rectangle", vec![m]));
    let mut m = Mask::new(MaskShape::Linear);
    m.rotation = 45.0;
    m.feather = 0.3;
    cases.push(("linear", vec![m]));
    let mut m = Mask::new(MaskShape::Mirror);
    m.height = 0.4;
    m.invert = true;
    cases.push(("inverted mirror", vec![m]));
    let mut m = Mask::new(MaskShape::Star);
    m.width = 0.9;
    m.height = 0.9;
    cases.push(("star", vec![m]));
    let mut m = Mask::new(MaskShape::Heart);
    m.width = 0.9;
    m.height = 0.8;
    m.feather = 0.05;
    cases.push(("heart", vec![m]));
    let mut big = Mask::new(MaskShape::Rectangle);
    big.width = 1.2;
    big.height = 0.8;
    let mut hole = Mask::new(MaskShape::Ellipse);
    hole.op = MaskOp::Subtract;
    hole.width = 0.4;
    hole.height = 0.4;
    let mut band = Mask::new(MaskShape::Mirror);
    band.op = MaskOp::Intersect;
    band.height = 0.6;
    band.rotation = 90.0;
    let mut extra = Mask::new(MaskShape::Ellipse);
    extra.width = 0.1;
    extra.height = 0.1;
    cases.push((
        "add, subtract, intersect, add",
        vec![big, hole, band, extra],
    ));
    cases
}

/// Every shape and every combination, on both decode paths, pixel by pixel
/// against `matte::mask_coverage`: what shows is the clip's colour times the
/// coverage over the black background, in light.
#[test]
fn masks_match_the_cpu_reference_on_both_decode_paths() {
    let c = gpu!();
    for (path, picture) in flat_sources([230, 180, 40]) {
        let provider = Provider::default().with("a", picture);
        let plain = render(&c, &project(&["a"]), &provider);
        let colour = plain.pixel(W / 2, H / 2);
        let light = [0, 1, 2].map(|i| decode(colour[i] as f32 / 255.0));
        for (name, masks) in mask_cases() {
            let mut p = project(&["a"]);
            let material = with_masks(masks);
            let block = MatteBlock::new(&material, 0, (W as f32, H as f32));
            attach(&mut p, "s0", material);
            let f = render(&c, &p, &provider);
            for y in (1..H).step_by(3) {
                for x in (1..W).step_by(3) {
                    let local = [(x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32];
                    let cover = matte::mask_coverage(&block, local);
                    let want = light.map(|l| encode_byte(l * cover));
                    close(
                        &format!("{path} {name} at ({x},{y})"),
                        f.pixel(x, y),
                        want,
                        2,
                    );
                }
            }
        }
    }
}

/// A mask lives in the clip's frame: moving and turning the clip moves and
/// turns the mask with it.
#[test]
fn a_mask_moves_with_its_clip() {
    let c = gpu!();
    let provider = Provider::default().with("a", Picture::new(false, |_, _| [255, 255, 255]));
    let mut small = Mask::new(MaskShape::Rectangle);
    small.width = 0.2;
    small.height = 0.2;
    let mut p = project(&["a"]);
    attach(&mut p, "s0", with_masks(vec![small]));
    let centred = render(&c, &p, &provider);
    assert!(centred.pixel(W / 2, H / 2)[0] > 250);

    // Half the canvas to the right (position is -1..1 across the canvas).
    p.segment_mut("s0").unwrap().transform.position = [0.5, 0.0];
    let moved = render(&c, &p, &provider);
    assert!(moved.pixel(W / 2, H / 2)[0] < 5, "the mask stayed behind");
    assert!(
        moved.pixel(W / 2 + W / 4, H / 2)[0] > 250,
        "the mask did not follow"
    );
}

/// Keyframes animate a mask in the clip's source time.
#[test]
fn an_animated_mask_is_where_its_keyframes_say() {
    let c = gpu!();
    let provider = Provider::default().with("a", Picture::new(false, |_, _| [255, 255, 255]));
    let mut m = Mask::new(MaskShape::Rectangle);
    m.width = 0.2;
    m.height = 0.2;
    m.put_keyframe("x", 0, -0.3);
    m.put_keyframe("x", 2_000_000, 0.3);
    let mut p = project(&["a"]);
    attach(&mut p, "s0", with_masks(vec![m]));
    // At 1 s the mask is half way: at the centre.
    let f = render(&c, &p, &provider);
    assert!(f.pixel(W / 2, H / 2)[0] > 250);
    let early = c.render(&p, 0, (W, H), &provider).unwrap();
    assert!(early.pixel(W / 2, H / 2)[0] < 5);
    assert!(early.pixel(W / 2 - (0.3 * W as f32) as u32, H / 2)[0] > 250);
}

// ---------------------------------------------------------------------------
// Chroma key
// ---------------------------------------------------------------------------

/// Eight vertical stripes of 16 px: the key colour, near it, a greenish
/// grey, skin, red, blue, white and a dark green.
const STRIPES: [[u8; 3]; 8] = [
    [20, 200, 40],
    [40, 190, 70],
    [120, 150, 120],
    [225, 170, 140],
    [220, 30, 30],
    [30, 40, 220],
    [250, 250, 250],
    [10, 80, 20],
];

fn stripes(planar: bool) -> Picture {
    Picture::new(planar, |x, _| STRIPES[(x / 16) as usize])
}

fn key_material(key: ChromaKey) -> CompositingMaterial {
    CompositingMaterial {
        key: Some(key),
        ..CompositingMaterial::new()
    }
}

/// The key on the RGBA path against `matte::key_alpha` and `matte::despill`,
/// per stripe, over a grey background so a half-keyed pixel is visible.
#[test]
fn the_key_matches_the_cpu_reference() {
    let c = gpu!();
    let provider = Provider::default().with("a", stripes(false));
    for (tolerance, softness, spill) in [(0.3, 0.1, 0.5), (0.15, 0.4, 1.0), (0.5, 0.0, 0.0)] {
        let mut key = ChromaKey::new([20.0 / 255.0, 200.0 / 255.0, 40.0 / 255.0]);
        key.tolerance = tolerance;
        key.softness = softness;
        key.spill = spill;
        let material = key_material(key);
        let block = MatteBlock::new(&material, 0, (W as f32, H as f32));
        let mut p = project(&["a"]);
        p.canvas.background = [0.5, 0.5, 0.5, 1.0];
        attach(&mut p, "s0", material);
        let f = render(&c, &p, &provider);
        for (i, colour) in STRIPES.iter().enumerate() {
            let enc = colour.map(|v| v as f32 / 255.0);
            let alpha = matte::key_alpha(&block, enc);
            let kept = matte::despill(&block, enc);
            let want = [0, 1, 2].map(|ch| {
                let l = decode(kept[ch]) * alpha + 0.5 * (1.0 - alpha);
                encode_byte(l)
            });
            let x = i as u32 * 16 + 8;
            close(
                &format!("stripe {i} tolerance {tolerance} softness {softness} spill {spill}"),
                f.pixel(x, H / 2),
                want,
                2,
            );
        }
    }
}

/// The hardware path keys the same picture to the same result, within what
/// 8-bit YCbCr can carry.
#[test]
fn the_key_is_the_same_however_the_clip_was_decoded() {
    let c = gpu!();
    let key = ChromaKey::new([20.0 / 255.0, 200.0 / 255.0, 40.0 / 255.0]);
    let mut frames = Vec::new();
    for planar in [false, true] {
        let provider = Provider::default().with("a", stripes(planar));
        let mut p = project(&["a"]);
        p.canvas.background = [0.5, 0.5, 0.5, 1.0];
        attach(&mut p, "s0", key_material(key.clone()));
        frames.push(render(&c, &p, &provider));
    }
    for i in 0..8u32 {
        let (a, b) = (
            frames[0].pixel(i * 16 + 8, 40),
            frames[1].pixel(i * 16 + 8, 40),
        );
        close(
            &format!("stripe {i}"),
            b,
            [a[0] as i32, a[1] as i32, a[2] as i32],
            4,
        );
    }
}

/// Edge shrink eats into what is kept next to what is keyed, and leaves
/// what is further away alone.
#[test]
fn edge_shrink_pulls_the_matte_in() {
    let c = gpu!();
    // Key green on the left half, white on the right.
    let provider = Provider::default().with(
        "a",
        Picture::new(false, |x, _| {
            if x < W / 2 {
                [20, 200, 40]
            } else {
                [255, 255, 255]
            }
        }),
    );
    let mut key = ChromaKey::new([20.0 / 255.0, 200.0 / 255.0, 40.0 / 255.0]);
    key.spill = 0.0;
    let mut p = project(&["a"]);
    attach(&mut p, "s0", key_material(key.clone()));
    let sharp = render(&c, &p, &provider);
    key.shrink = 1.0; // 1% of the 96 px short side: about a pixel.
    attach(&mut p, "s0", key_material(key));
    let shrunk = render(&c, &p, &provider);
    let edge = W / 2;
    assert_eq!(
        sharp.pixel(edge, 40),
        [255, 255, 255, 255],
        "kept before shrinking"
    );
    assert!(shrunk.pixel(edge, 40)[0] < 250, "the edge did not move in");
    assert_eq!(
        shrunk.pixel(edge + 6, 40),
        sharp.pixel(edge + 6, 40),
        "the inside moved"
    );
    assert_eq!(
        shrunk.pixel(8, 40),
        sharp.pixel(8, 40),
        "the keyed side changed"
    );
}

/// "Show matte" draws the alpha as grey: kept white, keyed black.
#[test]
fn the_matte_view_shows_alpha_as_grey() {
    let c = gpu!();
    let provider = Provider::default().with("a", stripes(false));
    let mut material = key_material(ChromaKey::new([20.0 / 255.0, 200.0 / 255.0, 40.0 / 255.0]));
    let mut half = Mask::new(MaskShape::Linear);
    half.rotation = 90.0; // shows the left half
    material.masks.push(half);
    material.view_matte = true;
    let mut p = project(&["a"]);
    attach(&mut p, "s0", material);
    let f = render(&c, &p, &provider);
    assert_eq!(
        f.pixel(8, 40),
        [0, 0, 0, 255],
        "the key colour is not black"
    );
    assert_eq!(f.pixel(56, 40), [255, 255, 255, 255], "skin is not white");
    assert_eq!(
        f.pixel(88, 40),
        [0, 0, 0, 255],
        "the masked half is not black"
    );
}

// ---------------------------------------------------------------------------
// Blend modes
// ---------------------------------------------------------------------------

/// The top clip: a ramp in each channel, so every mode is tested over the
/// whole range of source values, flat over 2x2 blocks for the NV12 path.
fn ramp(planar: bool) -> Picture {
    Picture::new(planar, |x, y| {
        let (x, y) = (x & !1, y & !1);
        [
            (x * 255 / (W - 2)) as u8,
            (y * 255 / (H - 2)) as u8,
            (255 - (x + y) * 255 / (W + H - 4)) as u8,
        ]
    })
}

/// Every mode, at full and at partial opacity, with the top clip on both
/// decode paths, against `blend::composite` over the bottom clip as rendered.
#[test]
fn every_blend_mode_matches_the_cpu_reference() {
    let c = gpu!();
    let bottom = Picture::new(false, |x, _| {
        if x < W / 2 {
            [60, 140, 210]
        } else {
            [230, 200, 90]
        }
    });
    for planar in [false, true] {
        let provider = Provider::default()
            .with("b", bottom.clone())
            .with("t", ramp(planar));
        let base = render(&c, &project(&["b"]), &provider);
        // The top clip alone over black: its own colour, as rendered.
        let mut alone = project(&["b", "t"]);
        alone.tracks[0].hidden = true;
        let top = render(&c, &alone, &provider);
        for mode in BlendMode::all().into_iter().skip(1) {
            for opacity in [1.0f32, 0.6] {
                let mut p = project(&["b", "t"]);
                p.segment_mut("s1").unwrap().transform.opacity = opacity;
                attach(
                    &mut p,
                    "s1",
                    CompositingMaterial {
                        blend: mode.clone(),
                        ..CompositingMaterial::new()
                    },
                );
                let f = render(&c, &p, &provider);
                for y in (0..H).step_by(5) {
                    for x in (0..W).step_by(5) {
                        let b = base.pixel(x, y);
                        let t = top.pixel(x, y);
                        let lin = |v: u8| decode(v as f32 / 255.0);
                        let out = blend::composite(
                            &mode,
                            [lin(b[0]), lin(b[1]), lin(b[2]), 1.0],
                            [lin(t[0]), lin(t[1]), lin(t[2]), opacity],
                            true,
                        );
                        let want = [out[0], out[1], out[2]].map(encode_byte);
                        close(
                            &format!("{mode} at {opacity}, planar {planar}, ({x},{y})"),
                            f.pixel(x, y),
                            want,
                            2,
                        );
                    }
                }
            }
        }
    }
}

/// A blended clip blends only with what is beneath it: a clip above it is
/// drawn normally over the result, and a masked blended clip leaves the
/// frame alone where its mask does not cover.
#[test]
fn a_blend_sees_only_what_is_beneath_and_respects_its_mask() {
    let c = gpu!();
    let provider = Provider::default()
        .with("b", Picture::new(false, |_, _| [200, 100, 50]))
        .with("t", Picture::new(false, |_, _| [128, 128, 128]))
        .with("o", Picture::new(false, |_, _| [10, 20, 30]));
    let mut p = project(&["b", "t", "o"]);
    // The overlay clip covers the left quarter only.
    p.segment_mut("s2").unwrap().transform.scale = [0.25, 1.0];
    p.segment_mut("s2").unwrap().transform.position = [-0.75, 0.0];
    let mut half = Mask::new(MaskShape::Linear);
    half.rotation = -90.0; // shows the right half
    attach(
        &mut p,
        "s1",
        CompositingMaterial {
            blend: BlendMode::Multiply,
            masks: vec![half],
            ..CompositingMaterial::new()
        },
    );
    let f = render(&c, &p, &provider);
    assert_eq!(
        &f.pixel(10, 40)[..3],
        &[10, 20, 30],
        "the clip above was blended"
    );
    // Right half: 200, 100, 50 multiplied by 128 in encoded space.
    let want =
        [200.0, 100.0, 50.0].map(|v: f32| ((v / 255.0) * (128.0 / 255.0) * 255.0).round() as i32);
    close("multiplied half", f.pixel(110, 40), want, 2);
    // Left half, outside the mask and the overlay: the bottom clip, as it was.
    assert_eq!(
        &f.pixel(48, 40)[..3],
        &[200, 100, 50],
        "the mask did not hold the blend back"
    );
}

// ---------------------------------------------------------------------------
// Preview and export
// ---------------------------------------------------------------------------

/// The preview (RGBA) and the export (NV12) agree on a frame with every part
/// live: a feathered mask, a key with shrink and spill, a blend mode.
#[test]
fn the_preview_and_the_export_agree() {
    let c = gpu!();
    let provider = Provider::default()
        .with(
            "b",
            Picture::new(false, |x, y| [(x * 2) as u8, (y * 2) as u8, 128]),
        )
        .with("t", stripes(false));
    let mut p = project(&["b", "t"]);
    let mut key = ChromaKey::new([20.0 / 255.0, 200.0 / 255.0, 40.0 / 255.0]);
    key.shrink = 0.5;
    let mut m = Mask::new(MaskShape::Ellipse);
    m.width = 1.2;
    m.height = 0.9;
    m.feather = 0.3;
    attach(
        &mut p,
        "s1",
        CompositingMaterial {
            masks: vec![m],
            key: Some(key),
            blend: BlendMode::Screen,
            ..CompositingMaterial::new()
        },
    );
    let at: Micros = 1_000_000;
    let preview = c.render(&p, at, (W, H), &provider).expect("preview");
    let Ok(export) = c.render_nv12(&p, at, (W, H), &provider) else {
        eprintln!("skipping: no RGBA to NV12 compute pass on this device");
        return;
    };
    for by in (0..H).step_by(2) {
        for bx in (0..W).step_by(2) {
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

/// The eyedropper reads the footage as shot, whatever is done to the clip.
#[test]
fn the_eyedropper_reads_the_footage_under_the_point() {
    let c = gpu!();
    let provider = Provider::default().with("a", stripes(false));
    let mut p = project(&["a"]);
    p.segment_mut("s0").unwrap().transform.opacity = 0.3;
    attach(
        &mut p,
        "s0",
        CompositingMaterial {
            key: Some(ChromaKey::new([0.0, 1.0, 0.0])),
            blend: BlendMode::Difference,
            ..CompositingMaterial::new()
        },
    );
    let probe = super::commands::key_probe_project(&p, "s0").unwrap();
    let picked =
        super::commands::pick_from(&c, &probe, 0, [8.5 / W as f32, 0.5], &provider).unwrap();
    for ch in 0..3 {
        let want = STRIPES[0][ch] as f32 / 255.0;
        assert!((picked[ch] - want).abs() < 2.0 / 255.0, "picked {picked:?}");
    }
}
