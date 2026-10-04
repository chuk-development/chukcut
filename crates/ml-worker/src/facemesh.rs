//! MediaPipe's face mesh: 478 landmarks from a face crop, without a runtime.
//!
//! The network (`registry`, id `facemesh`) sees one face at a time, upright
//! and filling a 256×256 square: `input_12`, `[1, 256, 256, 3]`, RGB in
//! 0..1, rows first (NHWC). It answers 478 points as x, y, z in that
//! square's pixels (`Identity`, 1434 values) and a face-presence logit
//! (`Identity_1`).
//!
//! The square is a **region of interest**: a centre, a side and a rotation
//! (the eye line's angle), in the frame's pixels. On the first frame it
//! comes from YuNet's box and eyes; afterwards from the last frame's own
//! landmarks — MediaPipe's tracking loop, which keeps the crop steady and
//! skips the detector while the face stays found.

/// The crop the network takes.
pub const SIZE: usize = 256;
/// Points in the mesh: 468 for the face, five per iris.
pub const POINTS: usize = 478;
/// The outer corners of the eyes in the mesh (the subject's right, then
/// left), which set the region's rotation.
pub const RIGHT_EYE_OUTER: usize = 33;
pub const LEFT_EYE_OUTER: usize = 263;
/// How much larger than the face's own extent the region is. MediaPipe's
/// value: room for the face to move between frames.
pub const ROI_SCALE: f32 = 1.5;

/// A square region of the frame: centre, side, rotation (radians).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Roi {
    pub cx: f32,
    pub cy: f32,
    pub side: f32,
    pub angle: f32,
}

impl Roi {
    pub fn from_array(a: [f32; 4]) -> Self {
        Self {
            cx: a[0],
            cy: a[1],
            side: a[2],
            angle: a[3],
        }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.cx, self.cy, self.side, self.angle]
    }

    /// The frame position of crop pixel `(u, v)` (`0..SIZE`).
    pub fn to_frame(&self, u: f32, v: f32) -> (f32, f32) {
        let k = self.side / SIZE as f32;
        let (dx, dy) = ((u - SIZE as f32 / 2.0) * k, (v - SIZE as f32 / 2.0) * k);
        let (s, c) = self.angle.sin_cos();
        (self.cx + dx * c - dy * s, self.cy + dx * s + dy * c)
    }

    /// The region for a face found by a detector: its box, made square and
    /// larger, turned by the line from the right eye to the left.
    pub fn from_detection(bbox: [f32; 4], right_eye: [f32; 2], left_eye: [f32; 2]) -> Self {
        Self {
            cx: bbox[0] + bbox[2] / 2.0,
            cy: bbox[1] + bbox[3] / 2.0,
            side: bbox[2].max(bbox[3]) * ROI_SCALE,
            angle: (left_eye[1] - right_eye[1]).atan2(left_eye[0] - right_eye[0]),
        }
    }

    /// The region for the next frame, from this frame's landmarks: their
    /// extent along the eye line, made square and larger.
    pub fn from_points(points: &[[f32; 3]]) -> Self {
        let r = points[RIGHT_EYE_OUTER];
        let l = points[LEFT_EYE_OUTER];
        let angle = (l[1] - r[1]).atan2(l[0] - r[0]);
        let (s, c) = angle.sin_cos();
        // Extent in the face's own axes, so a tilted head is not measured
        // by its diagonal.
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in points.iter().take(468) {
            let u = p[0] * c + p[1] * s;
            let v = -p[0] * s + p[1] * c;
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let (um, vm) = ((u0 + u1) / 2.0, (v0 + v1) / 2.0);
        Self {
            cx: um * c - vm * s,
            cy: um * s + vm * c,
            side: (u1 - u0).max(v1 - v0) * ROI_SCALE,
            angle,
        }
    }
}

/// The network's input: the region, sampled bilinearly from an RGBA8 frame
/// (outside the frame is black), as NHWC RGB in 0..1.
pub fn input(rgba: &[u8], w: usize, h: usize, roi: &Roi) -> Vec<f32> {
    let mut out = Vec::with_capacity(SIZE * SIZE * 3);
    let px = |x: i64, y: i64, c: usize| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.0
        } else {
            rgba[(y as usize * w + x as usize) * 4 + c] as f32
        }
    };
    for v in 0..SIZE {
        for u in 0..SIZE {
            let (x, y) = roi.to_frame(u as f32 + 0.5, v as f32 + 0.5);
            let (x, y) = (x - 0.5, y - 0.5);
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = (x - x0, y - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - fx) + px(x0 + 1, y0, c) * fx;
                let bottom = px(x0, y0 + 1, c) * (1.0 - fx) + px(x0 + 1, y0 + 1, c) * fx;
                out.push((top * (1.0 - fy) + bottom * fy) / 255.0);
            }
        }
    }
    out
}

/// The network's points (crop pixels) in the frame's pixels; z scaled the
/// same way.
pub fn points(raw: &[f32], roi: &Roi) -> Vec<[f32; 3]> {
    let k = roi.side / SIZE as f32;
    raw.as_chunks::<3>()
        .0
        .iter()
        .take(POINTS)
        .map(|p| {
            let (x, y) = roi.to_frame(p[0], p[1]);
            [x, y, p[2] * k]
        })
        .collect()
}

/// The axis-aligned box of the face's points (not the irises).
pub fn bbox(points: &[[f32; 3]]) -> [f32; 4] {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points.iter().take(468) {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    [x0, y0, x1 - x0, y1 - y0]
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_region_maps_its_centre_and_turns_with_the_eyes() {
        let roi = Roi::from_detection([100.0, 50.0, 80.0, 100.0], [120.0, 90.0], [160.0, 90.0]);
        assert_eq!((roi.cx, roi.cy), (140.0, 100.0));
        assert_eq!(roi.side, 150.0);
        assert_eq!(roi.angle, 0.0);
        let (x, y) = roi.to_frame(128.0, 128.0);
        assert!((x - 140.0).abs() < 1e-4 && (y - 100.0).abs() < 1e-4);
        // A head tilted 90 degrees: the crop's x axis runs down the frame.
        let tilted = Roi {
            angle: std::f32::consts::FRAC_PI_2,
            ..roi
        };
        let (x, y) = tilted.to_frame(256.0, 128.0);
        assert!(
            (x - 140.0).abs() < 1e-3 && (y - 175.0).abs() < 1e-3,
            "{x} {y}"
        );
    }

    #[test]
    fn points_round_trip_to_the_region_they_came_from() {
        // A synthetic "face": points on a circle in a 30-degree tilted crop.
        let roi = Roi {
            cx: 300.0,
            cy: 200.0,
            side: 180.0,
            angle: 0.5,
        };
        let mut raw = vec![0.0f32; POINTS * 3];
        for i in 0..POINTS {
            let t = i as f32 / POINTS as f32 * std::f32::consts::TAU;
            raw[i * 3] = 128.0 + 60.0 * t.cos();
            raw[i * 3 + 1] = 128.0 + 60.0 * t.sin();
        }
        // The eye corners on a horizontal line in the crop.
        raw[RIGHT_EYE_OUTER * 3] = 90.0;
        raw[RIGHT_EYE_OUTER * 3 + 1] = 110.0;
        raw[LEFT_EYE_OUTER * 3] = 166.0;
        raw[LEFT_EYE_OUTER * 3 + 1] = 110.0;
        let pts = points(&raw, &roi);
        let next = Roi::from_points(&pts);
        assert!((next.angle - 0.5).abs() < 1e-3, "{next:?}");
        assert!(
            (next.cx - 300.0).abs() < 1.0 && (next.cy - 200.0).abs() < 1.0,
            "{next:?}"
        );
        // The circle spans 120 crop pixels = 84.4 frame pixels, times 1.5.
        assert!(
            (next.side - 120.0 * 180.0 / 256.0 * 1.5).abs() < 1.0,
            "{next:?}"
        );
    }

    #[test]
    fn the_input_is_the_region_sampled_upright() {
        // A frame whose left half is red and right half blue: a crop turned
        // half a circle sees blue on its left.
        let (w, h) = (64usize, 64usize);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                rgba[i..i + 4].copy_from_slice(if x < 32 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        let roi = Roi {
            cx: 32.0,
            cy: 32.0,
            side: 32.0,
            angle: std::f32::consts::PI,
        };
        let t = input(&rgba, w, h, &roi);
        let at = |u: usize, v: usize, c: usize| t[(v * SIZE + u) * 3 + c];
        assert!(at(10, 128, 2) > 0.99 && at(10, 128, 0) < 0.01);
        assert!(at(245, 128, 0) > 0.99);
    }
}
