//! Retouch: skin smoothing on the face, brighter eyes and teeth, a slimmer
//! jaw — driven by the clip's face landmarks (`modules::landmarks`).
//!
//! The effect is an ordinary entry of the effect stack (`catalog::RETOUCH`)
//! with five sliders. What makes it different is that it needs to know
//! where the faces are: the compositor looks the clip's landmarks up at the
//! frame's source time and hands each face's key points to the instance
//! ([`FxInstance::faces`], quad-local units), and [`passes`] turns them into
//! pixels of the frame being drawn and into up to three passes per face:
//!
//! 1. **Slim** (`fs_face_slim`): a landmark-driven local warp. Each side of
//!    the jaw is pulled a little towards the face's centre line by a local
//!    translation that falls off smoothly to nothing at the edge of its
//!    circle and, at the shifts used, never folds. Gustafsson's formula
//!    ("Interactive Image Warping", 1993) was tried first: it folds the
//!    picture (an ear cut in half) unless the shift is close to the radius.
//! 2. **Smooth skin** (`fs_skin_smooth`): an edge-preserving average
//!    (bilateral, a ring of taps weighted by colour difference) mixed in
//!    where the face ellipse, a skin-colour test and *not* the eyes, brows
//!    or mouth all agree. No face-parsing model: its common weights were
//!    trained on a non-commercial dataset (`docs/research/ml-features.md`
//!    §3.15), and the mesh already outlines what to leave sharp.
//! 3. **Eyes and teeth** (`fs_face_bright`): inside the eye openings, a
//!    gentle lift; inside the lips, light low-saturation pixels (teeth) lose
//!    their yellow and gain a little light, while red (lips, tongue) and
//!    dark (the mouth's inside) stay.
//!
//! A frame without landmarks (not analysed yet, or no face) is drawn as it
//! is: the effect is at rest there, as a matte mask is before its bake.

use super::render::FxInstance;
use crate::modules::landmarks::shape::{key, KeyPoints};

/// Faces one effect instance retouches.
pub const MAX_FACES: usize = 2;

/// The four looks the panel offers, as (smooth, eyes, teeth, slim), each
/// `0..100`. Named for what they do; the strength slider scales them all.
pub const PRESETS: [(&str, [f32; 4]); 4] = [
    ("Natural", [35.0, 15.0, 20.0, 0.0]),
    ("Soft", [65.0, 20.0, 25.0, 10.0]),
    ("Bright", [40.0, 45.0, 45.0, 0.0]),
    ("Sculpt", [40.0, 20.0, 25.0, 45.0]),
];

/// The parameter ids of a preset's four values.
pub const PRESET_PARAMS: [&str; 4] = ["smooth", "eyes", "teeth", "slim"];

/// Source fractions (displayed frame) to the clip's unit quad (`±0.5`, +v
/// up) through the quad's crop: what the instance carries.
pub fn to_quad(points: &KeyPoints, crop: [f32; 4]) -> KeyPoints {
    let (cw, ch) = ((crop[2] - crop[0]).max(1e-6), (crop[3] - crop[1]).max(1e-6));
    points.map(|p| [(p[0] - crop[0]) / cw - 0.5, 0.5 - (p[1] - crop[1]) / ch])
}

/// A quad-local point in pixels of a `size` frame, through the clip's
/// placement matrix.
fn to_pixels(m: &glam::Mat4, p: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    let c = *m * glam::Vec4::new(p[0], p[1], 0.0, 1.0);
    let (x, y) = (c.x / c.w, c.y / c.w);
    [
        (x + 1.0) * 0.5 * size.0 as f32,
        (1.0 - y) * 0.5 * size.1 as f32,
    ]
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn mid(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

fn len(a: [f32; 2]) -> f32 {
    a[0].hypot(a[1])
}

/// One face in pixels, measured the ways the passes need.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceGeometry {
    /// The eye line's direction (unit) and its normal pointing down the face.
    pub across: [f32; 2],
    pub down: [f32; 2],
    /// The face oval: centre and half extents along `across` and `down`.
    pub centre: [f32; 2],
    pub half: [f32; 2],
    /// Each eye opening: centre and half extents (across, down).
    pub eyes: [([f32; 2], [f32; 2]); 2],
    /// The lips (outer) and the mouth opening (inner).
    pub lips: ([f32; 2], [f32; 2]),
    pub opening: ([f32; 2], [f32; 2]),
    /// The jaw points the slim pulls, right then left.
    pub jaw: [[f32; 2]; 2],
}

impl FaceGeometry {
    pub fn of(points: &KeyPoints) -> Self {
        let k = points;
        let r_eye = mid(k[key::R_EYE_OUT], k[key::R_EYE_IN]);
        let l_eye = mid(k[key::L_EYE_OUT], k[key::L_EYE_IN]);
        let d = sub(l_eye, r_eye);
        let n = len(d).max(1e-3);
        let across = [d[0] / n, d[1] / n];
        let down = [-across[1], across[0]];
        let along = |v: [f32; 2], axis: [f32; 2]| v[0] * axis[0] + v[1] * axis[1];
        let width = len(sub(k[key::CHEEK_L], k[key::CHEEK_R]));
        let height = len(sub(k[key::CHIN], k[key::FOREHEAD]));
        let centre = mid(
            mid(k[key::FOREHEAD], k[key::CHIN]),
            mid(k[key::CHEEK_R], k[key::CHEEK_L]),
        );
        let eye = |out: usize, inner: usize, up: usize, low: usize| {
            let c = mid(mid(k[out], k[inner]), mid(k[up], k[low]));
            let w = along(sub(k[out], k[inner]), across).abs() / 2.0;
            let h = along(sub(k[low], k[up]), down).abs() / 2.0;
            (c, [w.max(1.0), h.max(1.0)])
        };
        let lips_c = mid(
            mid(k[key::MOUTH_R], k[key::MOUTH_L]),
            mid(k[key::LIP_UP], k[key::LIP_DOWN]),
        );
        let lips_half = [
            along(sub(k[key::MOUTH_L], k[key::MOUTH_R]), across).abs() / 2.0,
            along(sub(k[key::LIP_DOWN], k[key::LIP_UP]), down).abs() / 2.0,
        ];
        let open_c = mid(k[key::INNER_UP], k[key::INNER_DOWN]);
        let open_half = [
            lips_half[0] * 0.8,
            along(sub(k[key::INNER_DOWN], k[key::INNER_UP]), down).abs() / 2.0,
        ];
        Self {
            across,
            down,
            centre,
            half: [width / 2.0, height / 2.0],
            eyes: [
                eye(
                    key::R_EYE_OUT,
                    key::R_EYE_IN,
                    key::R_EYE_UP,
                    key::R_EYE_DOWN,
                ),
                eye(
                    key::L_EYE_OUT,
                    key::L_EYE_IN,
                    key::L_EYE_UP,
                    key::L_EYE_DOWN,
                ),
            ],
            lips: (lips_c, [lips_half[0].max(1.0), lips_half[1].max(1.0)]),
            opening: (open_c, [open_half[0].max(1.0), open_half[1].max(0.5)]),
            jaw: [k[key::JAW_R], k[key::JAW_L]],
        }
    }

    /// Where the slim pulls jaw point `side` (0 right, 1 left) to, at slim
    /// `amount` (`0..1`): towards the face's centre line, across the face.
    pub fn slim_target(&self, side: usize, amount: f32) -> [f32; 2] {
        let p = self.jaw[side];
        let rel = sub(p, self.centre);
        let off = rel[0] * self.across[0] + rel[1] * self.across[1];
        // At full slim the jaw comes in by 8 % of the face's width: visible,
        // still the same person.
        let pull = -off.signum() * (self.half[0] * 2.0 * 0.08 * amount).min(off.abs());
        [p[0] + self.across[0] * pull, p[1] + self.across[1] * pull]
    }
}

/// The passes of one retouch instance on a `size` frame: entry points and
/// their parameters, in order. Empty when the instance has no face or every
/// amount is zero.
pub fn passes(fx: &FxInstance, size: (u32, u32)) -> Vec<(&'static str, [[f32; 4]; 6])> {
    let Some(placement) = fx.placement else {
        return Vec::new();
    };
    let m = glam::Mat4::from_cols_array(&placement);
    let strength = fx.get("strength") / 100.0;
    let amount = |id: &str| (fx.get(id) / 100.0 * strength).clamp(0.0, 1.0);
    let (smooth, eyes, teeth, slim) = (
        amount("smooth"),
        amount("eyes"),
        amount("teeth"),
        amount("slim"),
    );
    let mut out = Vec::new();
    for face in fx.faces.iter().flatten() {
        let pixels: KeyPoints = face.map(|p| to_pixels(&m, p, size));
        let g = FaceGeometry::of(&pixels);
        if g.half[0] < 4.0 {
            // A face a few pixels wide in a small preview: nothing to see.
            continue;
        }
        if slim > 0.0 {
            let radius = g.half[0] * 0.9;
            let (r, l) = (g.slim_target(0, slim), g.slim_target(1, slim));
            let mut p = [[0.0; 4]; 6];
            p[0] = [g.jaw[0][0], g.jaw[0][1], g.jaw[1][0], g.jaw[1][1]];
            p[1] = [r[0], r[1], l[0], l[1]];
            p[2] = [radius, 0.0, 0.0, 0.0];
            out.push(("fs_face_slim", p));
        }
        if smooth > 0.0 {
            let mut p = [[0.0; 4]; 6];
            p[0] = [g.centre[0], g.centre[1], g.half[0], g.half[1] * 1.05];
            // The tap radius: a fiftieth of the face's width is pores and
            // blemishes, not the shape of the nose.
            p[1] = [
                g.across[0],
                g.across[1],
                (g.half[0] * 0.04).max(1.0),
                smooth,
            ];
            for (i, (c, h)) in g.eyes.iter().enumerate() {
                // An eye's protected area runs up over the brow.
                let up = h[1] * 2.5;
                p[2 + i] = [
                    c[0] - g.down[0] * up * 0.6,
                    c[1] - g.down[1] * up * 0.6,
                    h[0] * 1.6,
                    h[1] + up,
                ];
            }
            p[4] = [
                g.lips.0[0],
                g.lips.0[1],
                g.lips.1[0] * 1.2,
                g.lips.1[1] * 1.3,
            ];
            out.push(("fs_skin_smooth", p));
        }
        if eyes > 0.0 || teeth > 0.0 {
            let mut p = [[0.0; 4]; 6];
            for (i, (c, h)) in g.eyes.iter().enumerate() {
                p[i] = [c[0], c[1], h[0] * 1.1, h[1] * 1.3];
            }
            p[2] = [
                g.opening.0[0],
                g.opening.0[1],
                g.opening.1[0],
                g.opening.1[1],
            ];
            p[3] = [eyes, teeth, g.across[0], g.across[1]];
            out.push(("fs_face_bright", p));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::landmarks::shape::KEY_POINTS;

    /// A level face in a 200×200 frame, centred, 80 px wide and 100 tall.
    fn face_pixels() -> KeyPoints {
        let mut k = [[100.0f32, 100.0]; KEY_POINTS];
        k[key::FOREHEAD] = [100.0, 50.0];
        k[key::CHIN] = [100.0, 150.0];
        k[key::CHEEK_R] = [60.0, 100.0];
        k[key::CHEEK_L] = [140.0, 100.0];
        k[key::R_EYE_OUT] = [70.0, 85.0];
        k[key::R_EYE_IN] = [90.0, 85.0];
        k[key::R_EYE_UP] = [80.0, 82.0];
        k[key::R_EYE_DOWN] = [80.0, 88.0];
        k[key::L_EYE_OUT] = [130.0, 85.0];
        k[key::L_EYE_IN] = [110.0, 85.0];
        k[key::L_EYE_UP] = [120.0, 82.0];
        k[key::L_EYE_DOWN] = [120.0, 88.0];
        k[key::MOUTH_R] = [85.0, 125.0];
        k[key::MOUTH_L] = [115.0, 125.0];
        k[key::LIP_UP] = [100.0, 120.0];
        k[key::LIP_DOWN] = [100.0, 132.0];
        k[key::INNER_UP] = [100.0, 124.0];
        k[key::INNER_DOWN] = [100.0, 128.0];
        k[key::JAW_R] = [70.0, 130.0];
        k[key::JAW_L] = [130.0, 130.0];
        k
    }

    #[test]
    fn the_geometry_of_a_level_face() {
        let g = FaceGeometry::of(&face_pixels());
        assert!((g.across[0] - 1.0).abs() < 1e-6 && g.across[1].abs() < 1e-6);
        assert!((g.down[1] - 1.0).abs() < 1e-6);
        assert_eq!(g.half, [40.0, 50.0]);
        assert_eq!(g.eyes[0].0, [80.0, 85.0]);
        assert_eq!(g.eyes[0].1, [10.0, 3.0]);
        assert_eq!(g.opening.0, [100.0, 126.0]);
        // Slim pulls each side of the jaw inwards, never past the middle.
        let r = g.slim_target(0, 1.0);
        let l = g.slim_target(1, 1.0);
        assert!(
            r[0] > 70.0 && r[0] < 100.0 && (r[1] - 130.0).abs() < 1e-4,
            "{r:?}"
        );
        assert!(l[0] < 130.0 && l[0] > 100.0, "{l:?}");
        assert!((r[0] - 70.0 - 6.4).abs() < 1e-3);
    }

    #[test]
    fn quad_mapping_folds_the_crop() {
        let mut k = [[0.0f32; 2]; KEY_POINTS];
        k[0] = [0.25, 0.25];
        k[1] = [0.75, 0.75];
        let q = to_quad(&k, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(q[0], [-0.25, 0.25]);
        assert_eq!(q[1], [0.25, -0.25]);
        // Cropped to the left half: the same point is now at the quad's
        // horizontal middle.
        let q = to_quad(&k, [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(q[0], [0.0, 0.25]);
    }
}
