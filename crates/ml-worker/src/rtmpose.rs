//! Body landmarks: people found by YOLOX, each person's 17 COCO keypoints
//! read by RTMPose (OpenMMLab, Apache-2.0) from a crop around them — the
//! top-down pipeline of MMPose's own `rtmlib`, in plain Rust.
//!
//! - **Detector** (`yolox-tiny-human`): the frame letterboxed into
//!   [`DETECT_SIZE`]², top-left, the rest grey 114, **BGR** 0..255 (MMDet
//!   loads pictures as BGR and this export does not swap). Out: `dets`
//!   `[1, N, 5]` (x0, y0, x1, y1, score in input pixels), `labels`
//!   `[1, N]`; non-maximum suppression is inside the graph.
//! - **Pose** (`rtmpose-m`): the person's box, centred, grown by
//!   [`PADDING`] and widened or heightened to 3:4, resampled into a
//!   [`POSE_W`]×[`POSE_H`] crop (black outside the frame), RGB normalised
//!   with ImageNet's mean and deviation. Out: SimCC, one row of
//!   2×[`POSE_W`] bins for x and 2×[`POSE_H`] for y per keypoint; the
//!   keypoint is the peak bin / 2 in crop pixels and its confidence the
//!   smaller of the two peaks.
//!
//! The peak is refined by a parabola through it and its neighbours: the bins
//! are half a crop pixel, which on a person 1000 px tall is 2.6 frame
//! pixels — visible as jitter on a sticker that follows a hand.
//!
//! **Following.** Like the face mesh, a person found once is read in the
//! next frame from a region around last frame's keypoints
//! ([`region_of`]), and the detector runs again only when every person is
//! lost.

/// The detector's square input side.
pub const DETECT_SIZE: usize = 416;
/// The grey a letterbox is padded with.
const PAD_VALUE: f32 = 114.0;
/// The pose model's crop.
pub const POSE_W: usize = 192;
pub const POSE_H: usize = 256;
/// SimCC bins per crop pixel.
const SPLIT: f32 = 2.0;
/// The keypoints per person: COCO's 17.
pub const KEYPOINTS: usize = 17;
/// How much bigger than the person's box the crop is (MMPose's 1.25).
pub const PADDING: f32 = 1.25;
/// A keypoint at least this sure counts as seen. RTMPose's SimCC peaks are
/// not probabilities; `rtmlib` draws keypoints above 0.3.
pub const SEEN: f32 = 0.3;
/// A person is present when at least this many keypoints are seen.
pub const MIN_SEEN: usize = 4;
/// The region a person is looked for in next frame: their keypoints' box
/// grown by this (`rtmlib`'s pose tracker), before [`PADDING`].
pub const REGION_GROWTH: f32 = 1.25;

const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
const STD: [f32; 3] = [58.395, 57.12, 57.375];

/// COCO's keypoints, in the model's order.
pub const NAMES: [&str; KEYPOINTS] = [
    "nose",
    "left_eye",
    "right_eye",
    "left_ear",
    "right_ear",
    "left_shoulder",
    "right_shoulder",
    "left_elbow",
    "right_elbow",
    "left_wrist",
    "right_wrist",
    "left_hip",
    "right_hip",
    "left_knee",
    "right_knee",
    "left_ankle",
    "right_ankle",
];

/// Bilinear sample of channel `c` of an RGBA8 frame at `(x, y)`, pixel
/// centres on integers; `None` outside the frame.
fn sample(rgba: &[u8], w: usize, h: usize, x: f32, y: f32, c: usize) -> Option<f32> {
    if !(x > -1.0 && y > -1.0 && x < w as f32 && y < h as f32) {
        return None;
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xi: f32, yi: f32| -> f32 {
        let xi = xi.clamp(0.0, (w - 1) as f32) as usize;
        let yi = yi.clamp(0.0, (h - 1) as f32) as usize;
        rgba[(yi * w + xi) * 4 + c] as f32
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
    let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
    Some(top * (1.0 - fy) + bottom * fy)
}

/// The detector's input, NCHW `[1, 3, 416, 416]`, and the scale from frame
/// to input pixels.
pub fn detector_input(rgba: &[u8], w: usize, h: usize) -> (Vec<f32>, f32) {
    let n = DETECT_SIZE;
    let ratio = (n as f32 / w as f32).min(n as f32 / h as f32);
    let (rw, rh) = (
        ((w as f32 * ratio).round() as usize).min(n),
        ((h as f32 * ratio).round() as usize).min(n),
    );
    let mut out = vec![PAD_VALUE; 3 * n * n];
    for y in 0..rh {
        // cv2.resize's centre alignment.
        let sy = (y as f32 + 0.5) / ratio - 0.5;
        for x in 0..rw {
            let sx = (x as f32 + 0.5) / ratio - 0.5;
            for (plane, c) in [2usize, 1, 0].into_iter().enumerate() {
                out[plane * n * n + y * n + x] =
                    sample(rgba, w, h, sx.max(0.0), sy.max(0.0), c).unwrap_or(PAD_VALUE);
            }
        }
    }
    (out, ratio)
}

/// The people in the detector's output, as (x, y, w, h) in frame pixels
/// and a score, at least `threshold` sure, largest first. `labels`, when
/// the model gives them, keeps class 0 (person) only.
pub fn people(
    dets: &[f32],
    labels: Option<&[i64]>,
    ratio: f32,
    w: usize,
    h: usize,
    threshold: f32,
) -> Vec<([f32; 4], f32)> {
    let mut out: Vec<([f32; 4], f32)> = dets
        .as_chunks::<5>()
        .0
        .iter()
        .enumerate()
        .filter(|(i, d)| d[4] >= threshold && labels.is_none_or(|l| l.get(*i) == Some(&0)))
        .filter_map(|(_, d)| {
            let x0 = (d[0] / ratio).clamp(0.0, w as f32);
            let y0 = (d[1] / ratio).clamp(0.0, h as f32);
            let x1 = (d[2] / ratio).clamp(0.0, w as f32);
            let y1 = (d[3] / ratio).clamp(0.0, h as f32);
            (x1 - x0 >= 2.0 && y1 - y0 >= 2.0).then_some(([x0, y0, x1 - x0, y1 - y0], d[4]))
        })
        .collect();
    out.sort_by(|a, b| (b.0[2] * b.0[3]).total_cmp(&(a.0[2] * a.0[3])));
    out
}

/// The crop a person is read from: centre and size in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
}

impl Crop {
    /// MMPose's `bbox_xyxy2cs` and the aspect fix of its affine step: the
    /// box `(x, y, w, h)` grown by [`PADDING`], then the short side made
    /// longer until the shape is the crop's.
    pub fn from_box(b: [f32; 4]) -> Crop {
        let aspect = POSE_W as f32 / POSE_H as f32;
        let (mut w, mut h) = (b[2].max(1.0) * PADDING, b[3].max(1.0) * PADDING);
        if w > h * aspect {
            h = w / aspect;
        } else {
            w = h * aspect;
        }
        Crop {
            cx: b[0] + b[2] * 0.5,
            cy: b[1] + b[3] * 0.5,
            w,
            h,
        }
    }

    /// A crop pixel position (bin units already divided out) in the frame.
    fn to_frame(self, u: f32, v: f32) -> (f32, f32) {
        (
            self.cx + (u - POSE_W as f32 * 0.5) * self.w / POSE_W as f32,
            self.cy + (v - POSE_H as f32 * 0.5) * self.h / POSE_H as f32,
        )
    }
}

/// The pose model's input, NCHW `[1, 3, 256, 192]`.
pub fn pose_input(rgba: &[u8], w: usize, h: usize, crop: &Crop) -> Vec<f32> {
    let plane = POSE_W * POSE_H;
    let mut out = vec![0.0f32; 3 * plane];
    for v in 0..POSE_H {
        for u in 0..POSE_W {
            let (x, y) = crop.to_frame(u as f32, v as f32);
            for c in 0..3 {
                // Outside the frame is black, as cv2.warpAffine's border.
                let value = sample(rgba, w, h, x, y, c).unwrap_or(0.0);
                out[c * plane + v * POSE_W + u] = (value - MEAN[c]) / STD[c];
            }
        }
    }
    out
}

/// The peak of one SimCC row, refined to a fraction of a bin, and its value.
fn peak(row: &[f32]) -> (f32, f32) {
    let (i, &value) = row
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .expect("a SimCC row is never empty");
    let mut at = i as f32;
    if i > 0 && i + 1 < row.len() {
        let (l, c, r) = (row[i - 1], row[i], row[i + 1]);
        let curve = l - 2.0 * c + r;
        if curve < 0.0 {
            at += (0.5 * (l - r) / curve).clamp(-0.5, 0.5);
        }
    }
    (at, value)
}

/// The keypoints of one person from the SimCC rows: x and y in frame
/// pixels, then a confidence (0 for a keypoint the model put nowhere).
pub fn keypoints(simcc_x: &[f32], simcc_y: &[f32], crop: &Crop) -> Vec<[f32; 3]> {
    let (bx, by) = (
        (POSE_W as f32 * SPLIT) as usize,
        (POSE_H as f32 * SPLIT) as usize,
    );
    (0..KEYPOINTS)
        .map(|k| {
            let (Some(rx), Some(ry)) = (
                simcc_x.get(k * bx..(k + 1) * bx),
                simcc_y.get(k * by..(k + 1) * by),
            ) else {
                return [0.0, 0.0, 0.0];
            };
            let (ux, vx) = peak(rx);
            let (uy, vy) = peak(ry);
            let (x, y) = crop.to_frame(ux / SPLIT, uy / SPLIT);
            [x, y, vx.min(vy).max(0.0)]
        })
        .collect()
}

/// Whether enough keypoints are seen for this to be a person.
pub fn present(points: &[[f32; 3]]) -> bool {
    points.iter().filter(|p| p[2] >= SEEN).count() >= MIN_SEEN
}

/// The mean confidence over all keypoints: the person's score.
pub fn score(points: &[[f32; 3]]) -> f32 {
    if points.is_empty() {
        return 0.0;
    }
    points.iter().map(|p| p[2].min(1.0)).sum::<f32>() / points.len() as f32
}

/// The box (x, y, w, h) the seen keypoints span; `None` with fewer than two.
pub fn bbox(points: &[[f32; 3]]) -> Option<[f32; 4]> {
    let seen: Vec<&[f32; 3]> = points.iter().filter(|p| p[2] >= SEEN).collect();
    if seen.len() < 2 {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in seen {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    Some([x0, y0, x1 - x0, y1 - y0])
}

/// Where to look for this person in the next frame: their keypoints' box
/// grown by [`REGION_GROWTH`] about its centre, inside the frame.
pub fn region_of(points: &[[f32; 3]], w: usize, h: usize) -> Option<[f32; 4]> {
    let [x, y, bw, bh] = bbox(points)?;
    let (cx, cy) = (x + bw * 0.5, y + bh * 0.5);
    let (gw, gh) = (bw.max(8.0) * REGION_GROWTH, bh.max(8.0) * REGION_GROWTH);
    let x0 = (cx - gw * 0.5).clamp(0.0, w as f32);
    let y0 = (cy - gh * 0.5).clamp(0.0, h as f32);
    let x1 = (cx + gw * 0.5).clamp(0.0, w as f32);
    let y1 = (cy + gh * 0.5).clamp(0.0, h as f32);
    (x1 - x0 >= 2.0 && y1 - y0 >= 2.0).then_some([x0, y0, x1 - x0, y1 - y0])
}

/// Intersection over union of two (x, y, w, h) boxes.
pub fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union > 0.0 {
        inter / union
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crop_is_the_padded_box_at_three_by_four() {
        // A tall person: the width grows to 3:4.
        let c = Crop::from_box([100.0, 50.0, 60.0, 200.0]);
        assert_eq!((c.cx, c.cy), (130.0, 150.0));
        assert!((c.h - 250.0).abs() < 1e-3);
        assert!((c.w - 187.5).abs() < 1e-3);
        // A wide one: the height grows.
        let c = Crop::from_box([0.0, 0.0, 300.0, 100.0]);
        assert!((c.w - 375.0).abs() < 1e-3 && (c.h - 500.0).abs() < 1e-3);
        // The crop's centre pixel is the box's centre.
        let (x, y) = c.to_frame(POSE_W as f32 * 0.5, POSE_H as f32 * 0.5);
        assert_eq!((x, y), (150.0, 50.0));
    }

    #[test]
    fn a_simcc_peak_maps_back_to_the_frame_with_sub_bin_precision() {
        let crop = Crop::from_box([200.0, 100.0, 150.0, 200.0]);
        let (bx, by) = (POSE_W * 2, POSE_H * 2);
        let mut sx = vec![0.0f32; KEYPOINTS * bx];
        let mut sy = vec![0.0f32; KEYPOINTS * by];
        // Keypoint 0: a peak between bins 100 and 101 (crop x 50.25),
        // bin 300 for y (crop y 150).
        sx[100] = 0.8;
        sx[101] = 0.8;
        sx[99] = 0.2;
        sx[102] = 0.2;
        sy[300] = 0.9;
        let points = keypoints(&sx, &sy, &crop);
        assert_eq!(points.len(), KEYPOINTS);
        let (ex, ey) = crop.to_frame(50.25, 150.0);
        assert!((points[0][0] - ex).abs() < 0.05 * crop.w / POSE_W as f32);
        assert!((points[0][1] - ey).abs() < 1e-3);
        assert!((points[0][2] - 0.8).abs() < 1e-6);
        // A row of zeros is a keypoint nobody saw.
        assert_eq!(points[5][2], 0.0);
        assert!(!present(&points));
    }

    #[test]
    fn detections_become_people_in_frame_pixels_largest_first() {
        let dets = [
            10.0, 10.0, 50.0, 90.0, 0.9, // person, 40x80 in input pixels
            0.0, 0.0, 200.0, 200.0, 0.2, // too unsure
            100.0, 20.0, 200.0, 300.0, 0.7, // a bigger person
            5.0, 5.0, 5.5, 5.5, 0.95, // too small
        ];
        let found = people(&dets, Some(&[0, 0, 0, 0]), 0.5, 1000, 1000, 0.5);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, [200.0, 40.0, 200.0, 560.0]);
        assert_eq!(found[1].0, [20.0, 20.0, 80.0, 160.0]);
        // Another class is not a person.
        assert_eq!(people(&dets[..5], Some(&[3]), 1.0, 100, 100, 0.5).len(), 0);
    }

    #[test]
    fn the_letterbox_keeps_the_aspect_and_pads_grey_in_bgr() {
        let (w, h) = (832, 416);
        let mut rgba = vec![0u8; w * h * 4];
        for px in rgba.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[255, 0, 0, 255]); // red
        }
        let (input, ratio) = detector_input(&rgba, w, h);
        assert!((ratio - 0.5).abs() < 1e-6);
        let n = DETECT_SIZE;
        // Inside: red is the last plane (BGR).
        assert_eq!(input[2 * n * n + 10 * n + 10], 255.0);
        assert_eq!(input[10 * n + 10], 0.0);
        // Below the picture: grey.
        assert_eq!(input[300 * n + 10], PAD_VALUE);
    }

    #[test]
    fn the_next_region_grows_around_the_seen_keypoints() {
        let mut points = vec![[0.0, 0.0, 0.0]; KEYPOINTS];
        points[0] = [100.0, 100.0, 0.9];
        points[15] = [100.0, 300.0, 0.9];
        points[9] = [60.0, 200.0, 0.8];
        points[10] = [140.0, 200.0, 0.8];
        assert!(present(&points));
        let r = region_of(&points, 1000, 1000).unwrap();
        assert!((r[2] - 100.0).abs() < 1e-3 && (r[3] - 250.0).abs() < 1e-3);
        assert!((r[0] + r[2] * 0.5 - 100.0).abs() < 1e-3);
        assert!(iou(r, r) > 0.999 && iou(r, [900.0, 900.0, 10.0, 10.0]) == 0.0);
    }
}
