//! Masks and the chroma key on the GPU's terms, and their CPU reference.
//!
//! Both are per-pixel alpha in the clip's own quad, so they run in
//! `quad.wgsl` — the one place a clip's pixels pass through on every path:
//! an ordinary draw, a transition side, an effected clip's layer, a blended
//! clip's layer. This module packs a [`CompositingMaterial`] into the
//! uniform block the shader reads ([`MatteBlock`]) and spells the same
//! arithmetic out in Rust ([`mask_coverage`], [`key_alpha`], [`despill`]), so
//! the pixel tests compare the GPU against numbers computed independently
//! instead of against an earlier render.
//!
//! ## Where in the shader
//!
//! ```text
//!   sample ─► key (encoded colour of the source: alpha, spill) ─► grade ─►
//!   masks (alpha, in the quad's own frame) ─► opacity
//! ```
//!
//! The key runs before the grade because it must see the footage as shot: a
//! grade that warms the picture must not move what counts as "green". The
//! masks run after it because they do not look at colour at all.
//!
//! ## Mask space
//!
//! A point on the quad is `local` (0..1 each way, y down, as displayed). The
//! masks are measured in units of the quad's **shorter side**, y up, from the
//! quad's centre, so a circle stays round on any clip and a feather is the
//! same width at 360p and at 4K. The quad's size in pixels comes from its
//! matrix ([`quad_pixel_size`]); only the anti-aliasing width (one pixel)
//! depends on the render size.

use crate::modules::project::compositing::{CompositingMaterial, MAX_MASKS};
use crate::modules::project::compositing::{FEATHER_SPAN, SHRINK_SPAN};
use crate::modules::project::document::Micros;

/// The bits of `QuadUniform::matte_flags[0]`. `quad.wgsl` spells the same
/// numbers; `the_flags_match_the_shader` checks they agree.
pub mod flag {
    pub const MASK: u32 = 1;
    pub const KEY: u32 = 2;
    pub const KEY_SHRINK: u32 = 4;
    pub const VIEW_MATTE: u32 = 8;
}

/// How far in the CbCr plane a tolerance or softness of 1 reaches. Pure
/// green is about 0.6 from grey, so a tolerance of 1 keys everything at
/// least as far from grey as the key colour is close to it.
pub const KEY_SCALE: f32 = 0.6;

/// The packed masks and key, as the shader reads them. All zeros (and no
/// flag) for a clip with neither.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatteBlock {
    /// `[flags, mask count, 0, 0]`.
    pub flags: [u32; 4],
    /// The quad in pixels: width, height, one over the shorter side; w unused.
    pub size: [f32; 4],
    /// Key colour (encoded rgb), tolerance (in CbCr distance).
    pub key: [f32; 4],
    /// Softness (CbCr distance), spill, shrink radius (fraction of the
    /// source's shorter side); w unused.
    pub key2: [f32; 4],
    /// Three rows per mask: `(cx, cy, half width, half height)`,
    /// `(cos, sin, feather width, roundness)`, `(shape, op, invert, 0)`.
    pub masks: [[f32; 4]; MAX_MASKS * 3],
}

impl Default for MatteBlock {
    fn default() -> Self {
        Self {
            flags: [0; 4],
            size: [1.0, 1.0, 1.0, 0.0],
            key: [0.0; 4],
            key2: [0.0; 4],
            masks: [[0.0; 4]; MAX_MASKS * 3],
        }
    }
}

impl MatteBlock {
    /// Pack `material` for one instant of its clip, drawn `quad` pixels big.
    /// Disabled masks and keys, and shapes or operations this build does not
    /// know, are left out; a block with nothing left carries no flag and the
    /// shader takes the path it took before masks existed.
    pub fn new(material: &CompositingMaterial, source_time: Micros, quad: (f32, f32)) -> Self {
        let mut block = Self::default();
        let (w, h) = (quad.0.max(1e-3), quad.1.max(1e-3));
        let short = w.min(h);
        block.size = [w, h, 1.0 / short, 0.0];

        let mut count = 0usize;
        for mask in material.masks.iter().filter(|m| m.enabled) {
            if count == MAX_MASKS {
                break;
            }
            let (Some(shape), Some(op)) = (mask.shape.code(), mask.op.code()) else {
                continue;
            };
            let pose = mask.pose_at(source_time);
            let theta = pose.rotation.to_radians();
            let row = count * 3;
            block.masks[row] = [
                pose.x * w / short,
                pose.y * h / short,
                pose.width * 0.5,
                pose.height * 0.5,
            ];
            block.masks[row + 1] = [
                theta.cos(),
                theta.sin(),
                pose.feather * FEATHER_SPAN,
                pose.roundness,
            ];
            block.masks[row + 2] = [shape as f32, op as f32, f32::from(mask.invert as u8), 0.0];
            count += 1;
        }
        if count > 0 {
            block.flags[0] |= flag::MASK;
            block.flags[1] = count as u32;
        }

        if let Some(key) = material.key.as_ref().filter(|k| k.enabled) {
            block.flags[0] |= flag::KEY;
            block.key = [
                key.color[0],
                key.color[1],
                key.color[2],
                key.tolerance * KEY_SCALE,
            ];
            block.key2 = [
                key.softness * KEY_SCALE,
                key.spill,
                key.shrink * SHRINK_SPAN,
                0.0,
            ];
            if key.shrink > 0.0 {
                block.flags[0] |= flag::KEY_SHRINK;
            }
        }
        if material.view_matte {
            block.flags[0] |= flag::VIEW_MATTE;
        }
        block
    }

    pub fn is_active(&self) -> bool {
        self.flags[0] != 0
    }
}

/// The quad's size in pixels of a `size` render, from its matrix: the lengths
/// of its two edges after the model-view-projection maps the unit quad.
pub fn quad_pixel_size(mvp: &[f32; 16], size: (u32, u32)) -> (f32, f32) {
    let (sw, sh) = (size.0 as f32 * 0.5, size.1 as f32 * 0.5);
    let w = (mvp[0] * sw).hypot(mvp[1] * sh);
    let h = (mvp[4] * sw).hypot(mvp[5] * sh);
    (w, h)
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

/// Outer and inner radius of the star, a regular five-pointed one.
pub const STAR_INNER: f32 = 0.381_966;

/// Where the heart curve's bounding box is centred vertically, in curve units.
const HEART_SHIFT: f32 = 2.538_374;

/// The star's ten corners in a box of half-size 1, first point up.
pub fn star_polygon() -> [[f32; 2]; 10] {
    let mut out = [[0.0; 2]; 10];
    for (k, p) in out.iter_mut().enumerate() {
        let angle = std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
        let r = if k % 2 == 0 { 1.0 } else { STAR_INNER };
        *p = [r * angle.cos(), r * angle.sin()];
    }
    out
}

/// The heart, the classic parametric curve sampled at 32 points, scaled so
/// its width spans -1..1 and centred vertically.
pub fn heart_polygon() -> [[f32; 2]; 32] {
    let mut out = [[0.0; 2]; 32];
    for (k, p) in out.iter_mut().enumerate() {
        let t = std::f32::consts::TAU * k as f32 / 32.0;
        let x = 16.0 * t.sin().powi(3);
        let y = 13.0 * t.cos() - 5.0 * (2.0 * t).cos() - 2.0 * (3.0 * t).cos() - (4.0 * t).cos();
        *p = [x / 16.0, (y + HEART_SHIFT) / 16.0];
    }
    out
}

/// Signed distance to a closed polygon: negative inside. The distance to the
/// nearest edge, signed by an even-odd crossing count.
pub fn polygon_distance(p: [f32; 2], poly: &[[f32; 2]]) -> f32 {
    let mut best = f32::MAX;
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let e = [b[0] - a[0], b[1] - a[1]];
        let w = [p[0] - a[0], p[1] - a[1]];
        let t = ((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1])).clamp(0.0, 1.0);
        let d = [w[0] - e[0] * t, w[1] - e[1] * t];
        best = best.min(d[0] * d[0] + d[1] * d[1]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    let d = best.sqrt();
    if inside {
        -d
    } else {
        d
    }
}

/// Signed distance from `q` (in the mask's own frame, short-side units) to a
/// shape with half-size `half` and corner `roundness`.
pub fn shape_distance(shape: u32, q: [f32; 2], half: [f32; 2], roundness: f32) -> f32 {
    let (a, b) = (half[0].max(1e-4), half[1].max(1e-4));
    let small = a.min(b);
    match shape {
        // Linear: one edge through the centre; the clip shows below it.
        0 => q[1],
        // Mirror: a band `height` wide.
        1 => q[1].abs() - b,
        // Ellipse: radial distance, scaled back by the smaller radius.
        2 => ((q[0] / a).hypot(q[1] / b) - 1.0) * small,
        // Rectangle with rounded corners.
        3 => {
            let r = roundness.clamp(0.0, 1.0) * small;
            let v = [q[0].abs() - a + r, q[1].abs() - b + r];
            let outside = v[0].max(0.0).hypot(v[1].max(0.0));
            outside + v[0].max(v[1]).min(0.0) - r
        }
        4 => polygon_distance([q[0] / a, q[1] / b], &star_polygon()) * small,
        5 => polygon_distance([q[0] / a, q[1] / b], &heart_polygon()) * small,
        _ => f32::MAX,
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of the clip shows at `local` (0..1 on the quad, y down) under
/// every mask in `block`, combined in order. 1 when there are none.
pub fn mask_coverage(block: &MatteBlock, local: [f32; 2]) -> f32 {
    if block.flags[0] & flag::MASK == 0 {
        return 1.0;
    }
    let [w, h, inv_short, _] = block.size;
    let p = [
        (local[0] - 0.5) * w * inv_short,
        (0.5 - local[1]) * h * inv_short,
    ];
    let mut acc = 0.0f32;
    for i in 0..block.flags[1] as usize {
        let m0 = block.masks[i * 3];
        let m1 = block.masks[i * 3 + 1];
        let m2 = block.masks[i * 3 + 2];
        let d = [p[0] - m0[0], p[1] - m0[1]];
        // Into the mask's frame: undo its clockwise turn.
        let q = [d[0] * m1[0] - d[1] * m1[1], d[0] * m1[1] + d[1] * m1[0]];
        let dist = shape_distance(m2[0] as u32, q, [m0[2], m0[3]], m1[3]);
        let soft = m1[2].max(inv_short);
        let mut c = 1.0 - smoothstep(-0.5 * soft, 0.5 * soft, dist);
        if m2[2] > 0.5 {
            c = 1.0 - c;
        }
        let op = m2[1] as u32;
        acc = if i == 0 {
            if op == 1 {
                1.0 - c
            } else {
                c
            }
        } else {
            match op {
                1 => acc.min(1.0 - c),
                2 => acc.min(c),
                _ => acc.max(c),
            }
        };
    }
    acc
}

/// BT.709 chroma of gamma-encoded RGB.
pub fn cbcr(c: [f32; 3]) -> [f32; 2] {
    let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    [(c[2] - y) / 1.8556, (c[0] - y) / 1.5748]
}

/// The key's alpha for one encoded colour: 0 at the key colour and within
/// the tolerance, a smooth ramp over the softness, 1 beyond.
pub fn key_alpha(block: &MatteBlock, c: [f32; 3]) -> f32 {
    let k = cbcr([block.key[0], block.key[1], block.key[2]]);
    let p = cbcr(c);
    let d = (p[0] - k[0]).hypot(p[1] - k[1]);
    let lo = block.key[3];
    let hi = lo + block.key2[0] + 1e-4;
    smoothstep(lo, hi, d)
}

/// The key colour's tint taken out of `c`, luma kept: the chroma component
/// along the key's direction is reduced by `spill`, fully next to the key
/// and fading out one [`KEY_SCALE`] beyond its ramp.
pub fn despill(block: &MatteBlock, c: [f32; 3]) -> [f32; 3] {
    let k = cbcr([block.key[0], block.key[1], block.key[2]]);
    let len = k[0].hypot(k[1]);
    if len < 1e-4 || block.key2[1] <= 0.0 {
        return c;
    }
    let dir = [k[0] / len, k[1] / len];
    let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    let mut p = cbcr(c);
    let d = (p[0] - k[0]).hypot(p[1] - k[1]);
    let hi = block.key[3] + block.key2[0];
    let weight = (1.0 - (d - hi) / KEY_SCALE).clamp(0.0, 1.0);
    let along = (p[0] * dir[0] + p[1] * dir[1]).max(0.0) * block.key2[1] * weight;
    p = [p[0] - dir[0] * along, p[1] - dir[1] * along];
    let r = y + 1.5748 * p[1];
    let b = y + 1.8556 * p[0];
    let g = (y - 0.2126 * r - 0.0722 * b) / 0.7152;
    [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::compositing::{ChromaKey, Mask, MaskOp, MaskShape};

    fn material(masks: Vec<Mask>) -> CompositingMaterial {
        CompositingMaterial {
            masks,
            ..CompositingMaterial::new()
        }
    }

    #[test]
    fn the_flags_and_shapes_match_the_shader() {
        let shader = include_str!("shaders/quad.wgsl");
        for (name, value) in [
            ("M_MASK", flag::MASK),
            ("M_KEY", flag::KEY),
            ("M_KEY_SHRINK", flag::KEY_SHRINK),
            ("M_VIEW_MATTE", flag::VIEW_MATTE),
        ] {
            assert!(
                shader.contains(&format!("const {name}: u32 = {value}u;")),
                "{name} differs from quad.wgsl"
            );
        }
        for (name, value) in [("KEY_SCALE", KEY_SCALE), ("STAR_INNER", STAR_INNER)] {
            assert!(
                shader.contains(&format!("const {name}: f32 = {value:?};")),
                "{name} differs from quad.wgsl"
            );
        }
        // The heart's corners are literals in the shader; they must be the
        // curve this module samples.
        let start = shader.find("const HEART").expect("HEART in quad.wgsl");
        let body = &shader[start..shader[start..].find(");").unwrap() + start];
        let numbers: Vec<f32> = body
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter(|s| s.contains('.'))
            .map(|s| s.parse().unwrap())
            .collect();
        let heart = heart_polygon();
        assert_eq!(numbers.len(), heart.len() * 2);
        for (i, p) in heart.iter().enumerate() {
            assert!((numbers[i * 2] - p[0]).abs() < 1e-5, "heart x {i}");
            assert!((numbers[i * 2 + 1] - p[1]).abs() < 1e-5, "heart y {i}");
        }
        // The shape codes are the menu order of `MaskShape`.
        assert_eq!(MaskShape::Linear.code(), Some(0));
        assert_eq!(MaskShape::Heart.code(), Some(5));
        assert_eq!(MaskOp::Subtract.code(), Some(1));
    }

    #[test]
    fn a_circle_covers_its_inside_and_not_its_outside() {
        let block = MatteBlock::new(
            &material(vec![Mask::new(MaskShape::Ellipse)]),
            0,
            (400.0, 200.0),
        );
        // Width 0.5 of the 200 px short side: radius 50 px around the centre.
        assert_eq!(mask_coverage(&block, [0.5, 0.5]), 1.0);
        assert_eq!(mask_coverage(&block, [(200.0 + 40.0) / 400.0, 0.5]), 1.0);
        assert_eq!(mask_coverage(&block, [(200.0 + 60.0) / 400.0, 0.5]), 0.0);
        // Round, not stretched by the wide quad: 40 px up is inside too.
        assert_eq!(mask_coverage(&block, [0.5, (100.0 - 40.0) / 200.0]), 1.0);
        assert_eq!(mask_coverage(&block, [0.5, (100.0 - 60.0) / 200.0]), 0.0);
    }

    #[test]
    fn invert_subtract_and_intersect_combine_in_order() {
        let mut big = Mask::new(MaskShape::Rectangle);
        big.width = 1.0;
        big.height = 1.0;
        let mut small = Mask::new(MaskShape::Ellipse);
        small.width = 0.2;
        small.height = 0.2;
        small.op = MaskOp::Subtract;
        let block = MatteBlock::new(
            &material(vec![big.clone(), small.clone()]),
            0,
            (100.0, 100.0),
        );
        assert_eq!(mask_coverage(&block, [0.5, 0.5]), 0.0, "hole in the middle");
        assert_eq!(
            mask_coverage(&block, [0.75, 0.5]),
            1.0,
            "rectangle around it"
        );

        small.op = MaskOp::Intersect;
        let block = MatteBlock::new(
            &material(vec![big.clone(), small.clone()]),
            0,
            (100.0, 100.0),
        );
        assert_eq!(mask_coverage(&block, [0.5, 0.5]), 1.0);
        assert_eq!(mask_coverage(&block, [0.75, 0.5]), 0.0);

        let mut inverted = Mask::new(MaskShape::Ellipse);
        inverted.invert = true;
        let block = MatteBlock::new(&material(vec![inverted]), 0, (100.0, 100.0));
        assert_eq!(mask_coverage(&block, [0.5, 0.5]), 0.0);
        assert_eq!(mask_coverage(&block, [0.02, 0.02]), 1.0);
    }

    #[test]
    fn rotation_is_clockwise_and_position_is_a_fraction_of_the_clip() {
        let mut line = Mask::new(MaskShape::Linear);
        line.rotation = 90.0;
        line.x = 0.25;
        let block = MatteBlock::new(&material(vec![line]), 0, (200.0, 100.0));
        // A linear mask shows below its edge; turned a quarter clockwise,
        // "below" is the left. The edge sits a quarter of the width right of
        // centre, at x = 150 px.
        assert_eq!(mask_coverage(&block, [140.0 / 200.0, 0.5]), 1.0);
        assert_eq!(mask_coverage(&block, [160.0 / 200.0, 0.5]), 0.0);
    }

    #[test]
    fn feather_is_a_ramp_centred_on_the_edge() {
        let mut mask = Mask::new(MaskShape::Linear);
        mask.feather = 0.2; // 0.1 of the short side: 10 px on a 100 px quad.
        let block = MatteBlock::new(&material(vec![mask]), 0, (100.0, 100.0));
        assert!((mask_coverage(&block, [0.5, 0.5]) - 0.5).abs() < 1e-6);
        assert_eq!(mask_coverage(&block, [0.5, 0.56]), 1.0);
        assert_eq!(mask_coverage(&block, [0.5, 0.44]), 0.0);
    }

    #[test]
    fn the_star_and_the_heart_contain_their_centres_and_not_the_corners() {
        for shape in [MaskShape::Star, MaskShape::Heart] {
            let block =
                MatteBlock::new(&material(vec![Mask::new(shape.clone())]), 0, (100.0, 100.0));
            assert_eq!(mask_coverage(&block, [0.5, 0.5]), 1.0, "{shape}");
            assert_eq!(mask_coverage(&block, [0.27, 0.27]), 0.0, "{shape}");
        }
    }

    #[test]
    fn unknown_and_disabled_masks_draw_nothing() {
        let mut off = Mask::new(MaskShape::Ellipse);
        off.enabled = false;
        let unknown = Mask::new(MaskShape::Other("spiral".into()));
        let block = MatteBlock::new(&material(vec![off, unknown]), 0, (100.0, 100.0));
        assert!(!block.is_active());
    }

    #[test]
    fn the_key_takes_out_its_colour_and_keeps_others() {
        let mut m = CompositingMaterial::new();
        m.key = Some(ChromaKey::new([0.0, 1.0, 0.0]));
        let block = MatteBlock::new(&m, 0, (100.0, 100.0));
        assert_eq!(key_alpha(&block, [0.0, 1.0, 0.0]), 0.0);
        assert_eq!(key_alpha(&block, [0.1, 0.9, 0.1]), 0.0);
        assert_eq!(key_alpha(&block, [1.0, 0.2, 0.6]), 1.0);
        assert_eq!(key_alpha(&block, [0.5, 0.5, 0.5]), 1.0);
        // Spill takes green out of a greenish grey and leaves luma alone.
        let fringe = [0.5, 0.6, 0.5];
        let out = despill(&block, fringe);
        assert!(out[1] < fringe[1]);
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        assert!((luma(out) - luma(fringe)).abs() < 1e-4);
    }
}
