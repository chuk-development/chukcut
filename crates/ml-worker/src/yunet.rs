//! YuNet face detection: the tensor in and the faces out, without a runtime.
//!
//! A port of OpenCV's `FaceDetectorYN` (`modules/objdetect/src/face_detect.cpp`),
//! which is the reference the model was published against:
//!
//! - **Input**: the frame as BGR, `0..255` floats, NCHW, padded with zeros at
//!   the right and bottom to a multiple of 32 (the coarsest stride). No mean,
//!   no scale.
//! - **Output**: per stride (8, 16, 32), one anchor per grid cell, row-major:
//!   `cls` and `obj` scores, a box `(dx, dy, log w, log h)` and five landmarks
//!   `(dx, dy)`, all in units of the stride. The score is
//!   `sqrt(clamp(cls) · clamp(obj))`.
//! - Then non-maximum suppression at IoU 0.3.
//!
//! Kept free of `ort` so it is tested without a runtime or a model.

use crate::protocol::Face;

pub const STRIDES: [usize; 3] = [8, 16, 32];
/// OpenCV's default; faces overlapping more than this are one face.
pub const NMS_THRESHOLD: f32 = 0.3;
/// More than this many faces in one frame is noise for our purposes.
pub const TOP_K: usize = 5000;

/// `n` rounded up to the padding the network needs.
pub fn padded(n: usize) -> usize {
    n.div_ceil(32).max(1) * 32
}

/// The input tensor for an RGBA8 frame: `[1, 3, padded(h), padded(w)]`, BGR.
pub fn input(rgba: &[u8], width: usize, height: usize) -> (Vec<i64>, Vec<f32>) {
    let (pw, ph) = (padded(width), padded(height));
    let plane = pw * ph;
    let mut data = vec![0.0f32; 3 * plane];
    for y in 0..height {
        for x in 0..width {
            let p = &rgba[(y * width + x) * 4..(y * width + x) * 4 + 3];
            let o = y * pw + x;
            data[o] = p[2] as f32;
            data[plane + o] = p[1] as f32;
            data[2 * plane + o] = p[0] as f32;
        }
    }
    (vec![1, 3, ph as i64, pw as i64], data)
}

/// One stride's raw outputs, as flat slices.
pub struct StrideOutput<'a> {
    pub cls: &'a [f32],
    pub obj: &'a [f32],
    pub bbox: &'a [f32],
    pub kps: &'a [f32],
}

/// Decode every anchor scoring at least `threshold`, for a padded input of
/// `pw × ph`, then suppress overlaps and clip to the `width × height` frame.
pub fn decode(
    outputs: &[StrideOutput; 3],
    pw: usize,
    ph: usize,
    width: usize,
    height: usize,
    threshold: f32,
) -> Vec<Face> {
    let mut faces = Vec::new();
    for (stride, out) in STRIDES.iter().zip(outputs) {
        let s = *stride as f32;
        let cols = pw / stride;
        let rows = ph / stride;
        let anchors = (rows * cols)
            .min(out.cls.len())
            .min(out.obj.len())
            .min(out.bbox.len() / 4)
            .min(out.kps.len() / 10);
        for idx in 0..anchors {
            let score = (out.cls[idx].clamp(0.0, 1.0) * out.obj[idx].clamp(0.0, 1.0)).sqrt();
            if score < threshold {
                continue;
            }
            let (r, c) = ((idx / cols) as f32, (idx % cols) as f32);
            let b = &out.bbox[idx * 4..idx * 4 + 4];
            let cx = (c + b[0]) * s;
            let cy = (r + b[1]) * s;
            let w = b[2].exp() * s;
            let h = b[3].exp() * s;
            let k = &out.kps[idx * 10..idx * 10 + 10];
            let mut landmarks = [[0.0f32; 2]; 5];
            for (n, l) in landmarks.iter_mut().enumerate() {
                *l = [(k[2 * n] + c) * s, (k[2 * n + 1] + r) * s];
            }
            faces.push(Face {
                bbox: [cx - w / 2.0, cy - h / 2.0, w, h],
                landmarks,
                score,
            });
        }
    }
    let mut kept = nms(faces, NMS_THRESHOLD, TOP_K);
    for face in &mut kept {
        clip(&mut face.bbox, width as f32, height as f32);
    }
    kept.retain(|f| f.bbox[2] > 1.0 && f.bbox[3] > 1.0);
    kept
}

fn clip(b: &mut [f32; 4], w: f32, h: f32) {
    let (x0, y0) = (b[0].clamp(0.0, w), b[1].clamp(0.0, h));
    let (x1, y1) = ((b[0] + b[2]).clamp(0.0, w), (b[1] + b[3]).clamp(0.0, h));
    *b = [x0, y0, x1 - x0, y1 - y0];
}

/// Intersection over union of two `(x, y, w, h)` boxes.
pub fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
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

/// Greedy non-maximum suppression, best score first.
pub fn nms(mut faces: Vec<Face>, threshold: f32, top_k: usize) -> Vec<Face> {
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    faces.truncate(top_k);
    let mut kept: Vec<Face> = Vec::new();
    for face in faces {
        if kept.iter().all(|k| iou(&k.bbox, &face.bbox) <= threshold) {
            kept.push(face);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_rounds_up_to_the_coarsest_stride() {
        assert_eq!(padded(1), 32);
        assert_eq!(padded(32), 32);
        assert_eq!(padded(33), 64);
        assert_eq!(padded(360), 384);
    }

    #[test]
    fn the_input_is_bgr_planes_padded_with_zeros() {
        // One red pixel in a 1x1 frame.
        let (shape, data) = input(&[200, 10, 20, 255], 1, 1);
        assert_eq!(shape, [1, 3, 32, 32]);
        let plane = 32 * 32;
        assert_eq!(data[0], 20.0); // blue
        assert_eq!(data[plane], 10.0); // green
        assert_eq!(data[2 * plane], 200.0); // red
        assert_eq!(data[1], 0.0);
    }

    #[test]
    fn one_confident_anchor_becomes_one_face_in_frame_pixels() {
        // A 64x64 input: stride 8 has an 8x8 grid. Light anchor (row 2, col 3).
        let (pw, ph) = (64usize, 64usize);
        let grids: Vec<usize> = STRIDES.iter().map(|s| (pw / s) * (ph / s)).collect();
        let mut cls: Vec<Vec<f32>> = grids.iter().map(|n| vec![0.0; *n]).collect();
        let mut obj = cls.clone();
        let mut bbox: Vec<Vec<f32>> = grids.iter().map(|n| vec![0.0; n * 4]).collect();
        let kps: Vec<Vec<f32>> = grids.iter().map(|n| vec![0.5; n * 10]).collect();
        let idx = 2 * 8 + 3;
        cls[0][idx] = 0.81;
        obj[0][idx] = 1.0;
        // Centre offset (0.5, 0.5) cells, size e^ln(2) = 2 cells = 16 px.
        bbox[0][idx * 4..idx * 4 + 4].copy_from_slice(&[0.5, 0.5, 2f32.ln(), 2f32.ln()]);
        let outputs: [StrideOutput; 3] = std::array::from_fn(|i| StrideOutput {
            cls: &cls[i],
            obj: &obj[i],
            bbox: &bbox[i],
            kps: &kps[i],
        });
        let faces = decode(&outputs, pw, ph, 60, 60, 0.6);
        assert_eq!(faces.len(), 1);
        let f = faces[0];
        assert!((f.score - 0.9).abs() < 1e-5);
        // Centre (3.5 * 8, 2.5 * 8) = (28, 20), 16 px square.
        assert!((f.bbox[0] - 20.0).abs() < 1e-4 && (f.bbox[1] - 12.0).abs() < 1e-4);
        assert!((f.bbox[2] - 16.0).abs() < 1e-4 && (f.bbox[3] - 16.0).abs() < 1e-4);
        assert!((f.landmarks[0][0] - 28.0).abs() < 1e-4);
    }

    #[test]
    fn overlapping_boxes_keep_only_the_best() {
        let face = |x: f32, score: f32| Face {
            bbox: [x, 0.0, 10.0, 10.0],
            landmarks: [[0.0; 2]; 5],
            score,
        };
        let kept = nms(
            vec![face(0.0, 0.7), face(1.0, 0.9), face(50.0, 0.65)],
            0.3,
            10,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].bbox[0], 50.0);
    }
}
