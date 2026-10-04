//! Auto reframe: a subject-aware crop path that turns a 16:9 clip into a
//! 9:16 one (or any ratio into any other).
//!
//! The method is Google AutoFlip's (`docs/research/ml-features.md` §3.7): per
//! shot, find what matters in each analysed frame, choose the window of the
//! new shape that holds most of it, and smooth that window's path — held
//! still when the subject barely moves, panning gently when it travels, and
//! jumping only at a cut.
//!
//! **What matters** is, first, **faces**: when the ML worker is installed
//! (`modules/ml`), YuNet (OpenCV Zoo, MIT) runs on every analysed frame and
//! the window follows the faces it finds. A face missed for a moment (a head
//! turned to profile) is held for [`FACE_HOLD`] frames rather than handing
//! the frame to a weaker cue, which would make the window twitch. Frames with
//! no face, and every frame on a machine without the worker, use a saliency
//! map built without a model from three cues:
//!
//! - **motion that is not the camera's**: each frame is compared with the
//!   previous one warped by the camera motion the tracking module's KLT
//!   measures (`stabilise::MotionEstimator`), so a pan does not light up the
//!   whole frame but a person walking does;
//! - **local contrast**, centre against surround, for still subjects;
//! - **skin tone** in YCbCr, which is where faces and people usually are.
//!
//! The result is written as **position keyframes** on a clip scaled to fill
//! the new canvas — the crop window, expressed in the one keyframe system the
//! user can already see and edit (no new animatable property, no second way to
//! animate a crop). Shot changes get a hold keyframe so the window jumps with
//! the cut instead of sliding across it.

use serde::{Deserialize, Serialize};

use super::frames::{walk, Walk};
use super::jobs::JobContext;
use super::scenes::{self, Signature};
use super::stabilise::MotionEstimator;
use crate::modules::project::document::Micros;
use crate::modules::tracking::klt::{Gray, Pyramid};
use crate::modules::tracking::tracker::Similarity;

/// Height of the frames saliency is measured on.
pub const ANALYSIS_HEIGHT: u32 = 144;
/// Frames analysed per second; the path is smoothed far below this anyway.
pub const RATE: f64 = 10.0;
/// Analysed frames a face is remembered for after the detector last saw it:
/// half a second at [`RATE`].
pub const FACE_HOLD: usize = 5;
/// How much more a face counts than the strongest saliency. Faces are the
/// subject whenever there are any; saliency only breaks ties between them.
const FACE_WEIGHT: f32 = 10.0;

/// A face in an analysed frame, as fractions of the frame (x, y, w, h).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subject {
    pub bbox: [f32; 4],
    pub score: f32,
}

/// Finds faces in an RGBA frame (`rgba`, width, height). `None` when the
/// detector failed on this frame; the frame then uses saliency alone.
pub type FaceCue<'a> = &'a mut dyn FnMut(&[u8], usize, usize) -> Option<Vec<Subject>>;

/// Which way the window moves: across a frame that is too wide for the new
/// shape, or up and down one that is too tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// One analysed frame: where the window's centre should be, as a fraction of
/// the frame along the axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub t: Micros,
    pub centre: f32,
    /// How clearly one place stood out, `0..1`; a flat frame says little.
    pub confidence: f32,
}

/// What the analysis found.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Analysis {
    pub targets: Vec<Target>,
    /// Indices into `targets` where a new shot starts.
    pub cuts: Vec<usize>,
    /// Analysed frames whose target came from faces.
    pub face_frames: usize,
}

/// The window, as a fraction of the frame along `axis`, for a frame of
/// `source` shape shown in a canvas of `canvas` shape (both width over
/// height). `None` when the shapes match and there is nothing to choose.
pub fn window_fraction(source_aspect: f32, canvas_aspect: f32) -> Option<(Axis, f32)> {
    if !(source_aspect > 0.0 && canvas_aspect > 0.0) {
        return None;
    }
    let ratio = canvas_aspect / source_aspect;
    if (ratio - 1.0).abs() < 0.02 {
        None
    } else if ratio < 1.0 {
        Some((Axis::Horizontal, ratio))
    } else {
        Some((Axis::Vertical, 1.0 / ratio))
    }
}

/// Box blur by an integral image; `r` is the radius in pixels.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut integral = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let sum = integral[y1 * (w + 1) + x1]
                - integral[y0 * (w + 1) + x1]
                - integral[y1 * (w + 1) + x0]
                + integral[y0 * (w + 1) + x0];
            out[y * w + x] = (sum / ((x1 - x0) * (y1 - y0)) as f64) as f32;
        }
    }
    out
}

/// Scale a map so its 99th percentile is 1; an empty map stays zero.
fn normalise(map: &mut [f32]) {
    let mut sorted: Vec<f32> = map.to_vec();
    sorted.sort_by(f32::total_cmp);
    let top = sorted[((sorted.len() as f32 * 0.99) as usize).min(sorted.len() - 1)];
    if top > 1e-6 {
        for v in map.iter_mut() {
            *v = (*v / top).min(1.5);
        }
    } else {
        map.iter_mut().for_each(|v| *v = 0.0);
    }
}

/// The saliency of one frame, `w × h`, from its pixels, its luma, and the
/// previous frame's luma warped by the camera motion between them.
pub fn saliency(rgba: &[u8], luma: &Gray, previous: Option<(&Gray, &Similarity)>) -> Vec<f32> {
    let (w, h) = (luma.width, luma.height);
    let n = w * h;

    // Motion that is not the camera's.
    let mut motion = vec![0.0f32; n];
    if let Some((prev, s)) = previous {
        // The fit maps previous → current about the centre; invert it to
        // look up where each current pixel came from.
        let det = s.a * s.a + s.b * s.b;
        if det > 1e-6 {
            let (cx, cy) = (w as f32 * 0.5, h as f32 * 0.5);
            for y in 0..h {
                for x in 0..w {
                    let (qx, qy) = (x as f32 - cx - s.tx, y as f32 - cy - s.ty);
                    let px = (s.a * qx + s.b * qy) / det + cx;
                    let py = (-s.b * qx + s.a * qy) / det + cy;
                    if px < 0.0 || py < 0.0 || px > (w - 1) as f32 || py > (h - 1) as f32 {
                        continue;
                    }
                    motion[y * w + x] = (luma.data[y * w + x] - prev.sample(px, py)).abs();
                }
            }
            motion = box_blur(&motion, w, h, 3);
            // Compression noise and small misregistration are everywhere;
            // what is left above the median is movement.
            let mut sorted = motion.clone();
            sorted.sort_by(f32::total_cmp);
            let floor = sorted[sorted.len() / 2] * 2.0 + 2.0;
            for v in &mut motion {
                *v = (*v - floor).max(0.0);
            }
            normalise(&mut motion);
        }
    }

    // Centre against surround.
    let fine = box_blur(&luma.data, w, h, 2);
    let coarse = box_blur(&luma.data, w, h, (h / 6).max(4));
    let mut contrast: Vec<f32> = fine
        .iter()
        .zip(&coarse)
        .map(|(a, b)| (a - b).abs())
        .collect();
    contrast = box_blur(&contrast, w, h, 4);
    normalise(&mut contrast);

    // Skin tone (BT.601 YCbCr box), blurred into blobs.
    let mut skin: Vec<f32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .take(n)
        .map(|p| {
            let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
            let cb = 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b;
            let cr = 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b;
            let y = 0.299 * r + 0.587 * g + 0.114 * b;
            if (77.0..=127.0).contains(&cb) && (135.0..=173.0).contains(&cr) && y > 40.0 {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    skin = box_blur(&skin, w, h, 4);

    (0..n)
        .map(|i| {
            let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
            // A mild pull to the middle: when nothing stands out, stay put.
            let centre = 1.0 - ((x - 0.5).powi(2) + (y - 0.5).powi(2)).min(0.25) * 2.0;
            1.6 * motion[i] + 0.6 * contrast[i] + 0.8 * skin[i] + 0.1 * centre
        })
        .collect()
}

/// The weight of `faces` on a `w × h` map: a soft blob per face, larger
/// and more confident faces weighing more, so the speaker in the foreground
/// wins over a face in the crowd. For a vertical window the blob sits half a
/// face below the face's centre, which leaves head room above it.
pub fn face_map(faces: &[Subject], w: usize, h: usize, axis: Axis) -> Vec<f32> {
    let mut map = vec![0.0f32; w * h];
    for face in faces {
        let [fx, fy, fw, fh] = face.bbox;
        let cx = (fx + fw * 0.5) * w as f32;
        let mut cy = (fy + fh * 0.5) * h as f32;
        if axis == Axis::Vertical {
            cy += fh * 0.5 * h as f32;
        }
        let sx = (fw * w as f32 * 0.6).max(1.0);
        let sy = (fh * h as f32 * 0.6).max(1.0);
        let weight = face.score * (fh * 4.0).clamp(0.5, 2.0);
        let (x0, x1) = (
            (cx - 3.0 * sx).max(0.0) as usize,
            ((cx + 3.0 * sx) as usize).min(w.saturating_sub(1)),
        );
        let (y0, y1) = (
            (cy - 3.0 * sy).max(0.0) as usize,
            ((cy + 3.0 * sy) as usize).min(h.saturating_sub(1)),
        );
        for y in y0..=y1 {
            let dy = (y as f32 + 0.5 - cy) / sy;
            for x in x0..=x1 {
                let dx = (x as f32 + 0.5 - cx) / sx;
                map[y * w + x] += weight * (-0.5 * (dx * dx + dy * dy)).exp();
            }
        }
    }
    map
}

/// An RGBA frame shrunk to `height` (aspect kept) by averaging boxes of
/// source pixels: the saliency cues are tuned for [`ANALYSIS_HEIGHT`], and
/// frames for face detection are decoded larger.
pub fn shrink(rgba: &[u8], w: usize, h: usize, height: usize) -> (Vec<u8>, usize, usize) {
    if h <= height {
        return (rgba.to_vec(), w, h);
    }
    let nh = height.max(1);
    let nw = ((w * nh) as f32 / h as f32).round().max(1.0) as usize;
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        let (sy0, sy1) = (y * h / nh, ((y + 1) * h / nh).max(y * h / nh + 1));
        for x in 0..nw {
            let (sx0, sx1) = (x * w / nw, ((x + 1) * w / nw).max(x * w / nw + 1));
            let mut sum = [0u32; 4];
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let p = &rgba[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4];
                    for c in 0..4 {
                        sum[c] += p[c] as u32;
                    }
                }
            }
            let n = ((sy1 - sy0) * (sx1 - sx0)) as u32;
            for c in 0..4 {
                out[(y * nw + x) * 4 + c] = (sum[c] / n) as u8;
            }
        }
    }
    (out, nw, nh)
}

/// Where a window `fraction` of the frame wide (or tall) holds the most of
/// `map`, as its centre along `axis`, and how clearly it stood out.
pub fn best_window(map: &[f32], w: usize, h: usize, axis: Axis, fraction: f32) -> (f32, f32) {
    let (len, profile): (usize, Vec<f32>) = match axis {
        Axis::Horizontal => (
            w,
            (0..w)
                .map(|x| (0..h).map(|y| map[y * w + x]).sum())
                .collect(),
        ),
        Axis::Vertical => (
            h,
            (0..h)
                .map(|y| map[y * w..(y + 1) * w].iter().sum())
                .collect(),
        ),
    };
    let size = ((len as f32 * fraction).round() as usize).clamp(1, len);
    let mut sum: f32 = profile[..size].iter().sum();
    let (mut best, mut best_at) = (sum, 0usize);
    let mut worst = sum;
    for start in 1..=len - size {
        sum += profile[start + size - 1] - profile[start - 1];
        if sum > best {
            best = sum;
            best_at = start;
        }
        worst = worst.min(sum);
    }
    // Inside the best window, centre on the weight rather than on the
    // window, so a subject near one edge of it is not left at that edge.
    let window = &profile[best_at..best_at + size];
    let mass: f32 = window.iter().sum();
    let centroid = if mass > 1e-6 {
        window
            .iter()
            .enumerate()
            .map(|(i, v)| (best_at + i) as f32 * v)
            .sum::<f32>()
            / mass
    } else {
        best_at as f32 + size as f32 * 0.5
    };
    let half = size as f32 * 0.5;
    let centre = (centroid + 0.5).clamp(half, len as f32 - half) / len as f32;
    let confidence = if best > 1e-6 {
        ((best - worst) / best).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (centre, confidence)
}

/// Analyse `job` for a window `fraction` of the frame along `axis`. With
/// `faces`, every frame is also searched for faces (at the walk's size —
/// [`crate::modules::ml::faces::DETECTION_HEIGHT`] suits the detector) and
/// the window follows them; saliency always runs at [`ANALYSIS_HEIGHT`].
pub fn analyse(
    job: &Walk,
    axis: Axis,
    fraction: f32,
    ctx: Option<&JobContext>,
    progress_span: (f32, f32),
    mut faces: Option<FaceCue>,
) -> Result<Analysis, String> {
    let estimator = MotionEstimator::default();
    let mut previous: Option<(Pyramid, Signature)> = None;
    let mut out = Analysis::default();
    let mut scores = scenes::Scores {
        fps: job.max_rate.unwrap_or(RATE),
        ..Default::default()
    };
    // The last faces seen, and how many frames ago.
    let mut held: Option<(Vec<Subject>, usize)> = None;
    walk(job, ctx, progress_span, |frame| {
        let found = faces
            .as_mut()
            .and_then(|detect| detect(&frame.rgba, frame.width, frame.height));
        let (rgba, width, height) = shrink(
            &frame.rgba,
            frame.width,
            frame.height,
            ANALYSIS_HEIGHT as usize,
        );
        let gray = Gray {
            width,
            height,
            data: super::frames::luma(&rgba),
        };
        let pyramid = MotionEstimator::pyramid(gray.clone());
        let signature = Signature::of(&rgba, width, height);
        let motion = previous
            .as_ref()
            .and_then(|(p, _)| estimator.estimate(p, &pyramid));
        let prev_luma = previous.as_ref().map(|(p, _)| p.base());
        let mut map = saliency(&rgba, &gray, prev_luma.zip(motion.as_ref()));
        // A cut ends whatever was held: the face belonged to the last shot.
        let cut = previous
            .as_ref()
            .is_some_and(|(_, s)| signature.distance(s) > 0.3);
        held = match found {
            Some(f) if !f.is_empty() => Some((f, 0)),
            _ => held
                .take()
                .filter(|(_, age)| *age < FACE_HOLD && !cut)
                .map(|(f, age)| (f, age + 1)),
        };
        let on_faces = held.as_ref().map(|(f, _)| f.as_slice());
        if let Some(subjects) = on_faces {
            normalise(&mut map);
            let weights = face_map(subjects, width, height, axis);
            for (m, f) in map.iter_mut().zip(weights) {
                *m = 0.1 * *m + FACE_WEIGHT * f;
            }
            out.face_frames += 1;
        }
        let (centre, mut confidence) = best_window(&map, width, height, axis, fraction);
        if on_faces.is_some() {
            confidence = confidence.max(0.9);
        }
        scores.times.push(frame.pts);
        scores.scores.push(
            previous
                .as_ref()
                .map_or(0.0, |(_, s)| signature.distance(s)),
        );
        out.targets.push(Target {
            t: frame.pts,
            centre,
            confidence,
        });
        previous = Some((pyramid, signature));
        Ok(())
    })?;
    if out.targets.is_empty() {
        return Err("the clip has no frames to analyse".into());
    }
    out.cuts = scenes::detect(&scores, 0.5);
    Ok(out)
}

/// One point of the smoothed path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathPoint {
    pub t: Micros,
    pub centre: f32,
    /// The first point of a new shot: the window jumps here.
    pub cut: bool,
}

/// The smoothed window path for `analysis`, a window `fraction` of the frame.
///
/// Per shot: a shot whose subject stays within a fifth of the window is held
/// still at its median; otherwise the targets are smoothed (Gaussian, 0.6 s,
/// confidence-weighted) and the pan speed is limited to half a window per
/// second, forwards and backwards, so the window eases into a move instead of
/// lurching.
pub fn smooth(analysis: &Analysis, fraction: f32) -> Vec<PathPoint> {
    let targets = &analysis.targets;
    let n = targets.len();
    let mut bounds: Vec<usize> = analysis
        .cuts
        .iter()
        .copied()
        .filter(|&c| c > 0 && c < n)
        .collect();
    bounds.insert(0, 0);
    bounds.push(n);
    bounds.dedup();
    let half = fraction * 0.5;
    let mut out = Vec::with_capacity(n);
    for shot in bounds.windows(2) {
        let (a, b) = (shot[0], shot[1]);
        let part = &targets[a..b];
        let mut centres: Vec<f32> = part.iter().map(|t| t.centre).collect();
        let mut sorted = centres.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        let spread = sorted[(sorted.len() as f32 * 0.9) as usize % sorted.len()]
            - sorted[(sorted.len() as f32 * 0.1) as usize];
        if spread < fraction * 0.2 {
            centres.iter_mut().for_each(|c| *c = median);
        } else {
            let per_second = if part.len() > 1 {
                (part.len() - 1) as f32 / ((part[part.len() - 1].t - part[0].t).max(1) as f32 / 1e6)
            } else {
                RATE as f32
            };
            let sigma = (0.6 * per_second).max(0.5);
            let radius = (sigma * 3.0).ceil() as usize;
            let smoothed: Vec<f32> = (0..part.len())
                .map(|i| {
                    let (mut s, mut sw) = (0.0f32, 0.0f32);
                    let lo = i.saturating_sub(radius);
                    let hi = (i + radius).min(part.len() - 1);
                    for (j, target) in part.iter().enumerate().take(hi + 1).skip(lo) {
                        let d = i.abs_diff(j) as f32;
                        let w =
                            (-(d * d) / (2.0 * sigma * sigma)).exp() * (0.2 + target.confidence);
                        s += w * target.centre;
                        sw += w;
                    }
                    s / sw
                })
                .collect();
            centres = smoothed;
            let max_step = 0.5 * fraction / per_second;
            for i in 1..centres.len() {
                let d = (centres[i] - centres[i - 1]).clamp(-max_step, max_step);
                centres[i] = centres[i - 1] + d;
            }
            for i in (0..centres.len().saturating_sub(1)).rev() {
                let d = (centres[i] - centres[i + 1]).clamp(-max_step, max_step);
                centres[i] = centres[i + 1] + d;
            }
        }
        for (i, (t, c)) in part.iter().zip(centres).enumerate() {
            out.push(PathPoint {
                t: t.t,
                centre: c.clamp(half, 1.0 - half),
                cut: i == 0 && a > 0,
            });
        }
    }
    out
}

/// The points of `path` that linear interpolation needs to stay within
/// `tolerance` of it (Ramer–Douglas–Peucker, per shot), plus every cut.
pub fn simplify(path: &[PathPoint], tolerance: f32) -> Vec<PathPoint> {
    fn rdp(points: &[PathPoint], tolerance: f32, keep: &mut Vec<bool>, offset: usize) {
        if points.len() < 3 {
            return;
        }
        let (first, last) = (points[0], points[points.len() - 1]);
        let span = (last.t - first.t).max(1) as f32;
        let mut worst = (0usize, 0.0f32);
        for (i, p) in points.iter().enumerate().skip(1).take(points.len() - 2) {
            let f = (p.t - first.t) as f32 / span;
            let line = first.centre + (last.centre - first.centre) * f;
            let d = (p.centre - line).abs();
            if d > worst.1 {
                worst = (i, d);
            }
        }
        if worst.1 > tolerance {
            keep[offset + worst.0] = true;
            rdp(&points[..=worst.0], tolerance, keep, offset);
            rdp(&points[worst.0..], tolerance, keep, offset + worst.0);
        }
    }
    if path.is_empty() {
        return Vec::new();
    }
    let mut keep = vec![false; path.len()];
    let mut start = 0;
    for i in 1..=path.len() {
        if i == path.len() || path[i].cut {
            keep[start] = true;
            keep[i - 1] = true;
            rdp(&path[start..i], tolerance, &mut keep, start);
            start = i;
        }
    }
    path.iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_nine_into_nine_sixteen_is_a_narrow_horizontal_window() {
        let (axis, f) = window_fraction(16.0 / 9.0, 9.0 / 16.0).unwrap();
        assert_eq!(axis, Axis::Horizontal);
        assert!((f - 0.3164).abs() < 1e-3);
        let (axis, _) = window_fraction(9.0 / 16.0, 1.0).unwrap();
        assert_eq!(axis, Axis::Vertical);
        assert!(window_fraction(1.0, 1.0).is_none());
    }

    #[test]
    fn the_best_window_holds_the_bright_spot() {
        let (w, h) = (100, 20);
        let mut map = vec![0.0f32; w * h];
        for y in 5..15 {
            for x in 70..80 {
                map[y * w + x] = 1.0;
            }
        }
        let (centre, confidence) = best_window(&map, w, h, Axis::Horizontal, 0.3);
        assert!((centre - 0.75).abs() < 0.02, "{centre}");
        assert!(confidence > 0.9);
    }

    #[test]
    fn a_still_subject_holds_the_window_and_a_cut_jumps_it() {
        let mut analysis = Analysis::default();
        for i in 0..40 {
            let centre = if i < 20 {
                0.3 + 0.01 * (i % 3) as f32
            } else {
                0.7
            };
            analysis.targets.push(Target {
                t: i * 100_000,
                centre,
                confidence: 1.0,
            });
        }
        analysis.cuts = vec![20];
        let path = smooth(&analysis, 0.3);
        assert!(path[..20]
            .iter()
            .all(|p| (p.centre - path[0].centre).abs() < 1e-6));
        assert!(path[20].cut && (path[20].centre - 0.7).abs() < 1e-6);
        let simple = simplify(&path, 0.005);
        assert_eq!(simple.len(), 4, "{simple:?}");
    }

    #[test]
    fn a_moving_subject_is_followed_without_lurching() {
        let analysis = Analysis {
            targets: (0..50)
                .map(|i| Target {
                    t: i * 100_000,
                    centre: 0.2 + 0.6 * i as f32 / 49.0,
                    confidence: 1.0,
                })
                .collect(),
            cuts: Vec::new(),
            face_frames: 0,
        };
        let path = smooth(&analysis, 0.3);
        for pair in path.windows(2) {
            assert!(pair[1].centre - pair[0].centre <= 0.5 * 0.3 / 10.0 + 1e-5);
        }
        assert!(path[25].centre > 0.4 && path[25].centre < 0.6);
    }

    #[test]
    fn a_face_outweighs_a_brighter_distraction() {
        let (w, h) = (128usize, 72usize);
        // Saliency peaks on the left; a face sits on the right.
        let mut saliency = vec![0.0f32; w * h];
        for y in 20..50 {
            for x in 5..30 {
                saliency[y * w + x] = 3.0;
            }
        }
        normalise(&mut saliency);
        let face = Subject {
            bbox: [0.75, 0.3, 0.08, 0.2],
            score: 0.9,
        };
        let faces = face_map(&[face], w, h, Axis::Horizontal);
        let map: Vec<f32> = saliency
            .iter()
            .zip(&faces)
            .map(|(s, f)| 0.1 * s + FACE_WEIGHT * f)
            .collect();
        let (centre, _) = best_window(&map, w, h, Axis::Horizontal, 0.3);
        assert!((centre - 0.79).abs() < 0.05, "{centre}");
    }

    #[test]
    fn the_foreground_face_wins_over_a_small_one() {
        let (w, h) = (160usize, 90usize);
        let big = Subject {
            bbox: [0.1, 0.2, 0.12, 0.35],
            score: 0.8,
        };
        let small = Subject {
            bbox: [0.8, 0.4, 0.03, 0.06],
            score: 0.95,
        };
        let map = face_map(&[big, small], w, h, Axis::Horizontal);
        let (centre, _) = best_window(&map, w, h, Axis::Horizontal, 0.3);
        assert!(centre < 0.3, "{centre}");
    }

    #[test]
    fn shrinking_averages_and_keeps_the_aspect() {
        let (w, h) = (8usize, 4usize);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                if i % w < 4 {
                    [0, 0, 0, 255]
                } else {
                    [200, 200, 200, 255]
                }
            })
            .collect();
        let (out, nw, nh) = shrink(&rgba, w, h, 2);
        assert_eq!((nw, nh), (4, 2));
        assert_eq!(out[0], 0);
        assert_eq!(out[3 * 4], 200);
    }

    #[test]
    fn motion_saliency_finds_the_mover_not_the_pan() {
        let (w, h) = (96usize, 54usize);
        let texture = |x: f32, y: f32| 100.0 + 50.0 * (x * 0.37).sin() * (y * 0.29).cos();
        let mut prev = Gray::new(w, h);
        let mut cur = Gray::new(w, h);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                prev.data[y * w + x] = texture(x as f32, y as f32);
                // The background pans by 2 px; a bright block appears at
                // x 70..80 in the current frame only.
                let mut v = texture(x as f32 - 2.0, y as f32);
                if (70..80).contains(&x) && (20..34).contains(&y) {
                    v = 250.0;
                }
                cur.data[y * w + x] = v;
                rgba[(y * w + x) * 4..(y * w + x) * 4 + 4]
                    .copy_from_slice(&[v as u8, v as u8, v as u8, 255]);
            }
        }
        let pan = Similarity {
            a: 1.0,
            b: 0.0,
            tx: 2.0,
            ty: 0.0,
            scale: 1.0,
            inliers: 100,
        };
        let map = saliency(&rgba, &cur, Some((&prev, &pan)));
        let (centre, _) = best_window(&map, w, h, Axis::Horizontal, 0.3);
        assert!(centre > 0.65, "{centre}");
    }
}
