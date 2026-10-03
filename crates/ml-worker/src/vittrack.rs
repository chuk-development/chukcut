//! VitTrack single-object tracking: crops, tensors and the box update,
//! without a runtime.
//!
//! A port of OpenCV's `TrackerVit` (`modules/video/src/tracking/tracker_vit.cpp`),
//! the reference the model was published with:
//!
//! - **Template** (once): a square of side `ceil(sqrt(w·h) · 2)` around the
//!   start box, padded with black where it leaves the frame, resized to
//!   128×128.
//! - **Search** (every frame): the same around the last box but `· 4`,
//!   resized to 256×256.
//! - Both normalised as OpenCV does it: channels in BGR order, minus
//!   `(0.485, 0.456, 0.406)·255`, divided by `(0.229, 0.224, 0.225)·255`.
//!   (OpenCV applies the ImageNet RGB statistics to BGR planes; the model
//!   was validated that way, so we copy it rather than "fix" it.)
//! - **Output**: a 16×16 confidence map (weighted by a Hann window, so the
//!   tracker prefers staying near where the object was), and per cell a size
//!   and a sub-cell offset, as fractions of the search crop.
//!
//! A frame whose best confidence is below [`SCORE_THRESHOLD`] is "lost": the
//! box is not moved and the caller is told.

/// OpenCV's default: low enough to keep a blurred object, high enough to
/// stop on a black frame.
pub const SCORE_THRESHOLD: f32 = 0.20;
pub const TEMPLATE_SIZE: usize = 128;
pub const SEARCH_SIZE: usize = 256;
const MAP: usize = 16;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// An integer box, as OpenCV's `Rect` holds it: x, y, width, height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn from_f32(b: [f32; 4]) -> Self {
        Rect {
            x: b[0].round() as i32,
            y: b[1].round() as i32,
            w: (b[2].round() as i32).max(1),
            h: (b[3].round() as i32).max(1),
        }
    }

    pub fn to_f32(self) -> [f32; 4] {
        [self.x as f32, self.y as f32, self.w as f32, self.h as f32]
    }
}

/// The side of the square crop around `r` at `factor` (2 for the template,
/// 4 for the search region).
pub fn crop_side(r: Rect, factor: i32) -> i32 {
    (((r.w as f64) * (r.h as f64)).sqrt() * factor as f64).ceil() as i32
}

/// The crop's top-left corner in the frame. Integer division, like OpenCV,
/// so the box update below lands on the same pixels.
fn crop_origin(r: Rect, side: i32) -> (i32, i32) {
    (r.x + (r.w - side) / 2, r.y + (r.h - side) / 2)
}

/// The normalised NCHW tensor of the square crop of side `crop_side(r,
/// factor)` around `r`, resized to `size`. Outside the frame is black.
pub fn crop_tensor(
    rgba: &[u8],
    width: usize,
    height: usize,
    r: Rect,
    factor: i32,
    size: usize,
) -> Vec<f32> {
    let side = crop_side(r, factor).max(1);
    let (ox, oy) = crop_origin(r, side);
    let plane = size * size;
    let mut out = vec![0.0f32; 3 * plane];
    // The crop pixel (cx, cy), black where it leaves the frame; BGR.
    let pixel = |cx: i32, cy: i32| -> [f32; 3] {
        let (x, y) = (ox + cx, oy + cy);
        if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
            return [0.0; 3];
        }
        let p = &rgba[(y as usize * width + x as usize) * 4..][..3];
        [p[2] as f32, p[1] as f32, p[0] as f32]
    };
    // Bilinear with pixel centres aligned, as cv::resize INTER_LINEAR does.
    let scale = side as f32 / size as f32;
    for v in 0..size {
        let sy = ((v as f32 + 0.5) * scale - 0.5).clamp(0.0, (side - 1) as f32);
        let y0 = sy.floor() as i32;
        let y1 = (y0 + 1).min(side - 1);
        let fy = sy - y0 as f32;
        for u in 0..size {
            let sx = ((u as f32 + 0.5) * scale - 0.5).clamp(0.0, (side - 1) as f32);
            let x0 = sx.floor() as i32;
            let x1 = (x0 + 1).min(side - 1);
            let fx = sx - x0 as f32;
            let (a, b, c, d) = (pixel(x0, y0), pixel(x1, y0), pixel(x0, y1), pixel(x1, y1));
            for ch in 0..3 {
                let top = a[ch] + (b[ch] - a[ch]) * fx;
                let bottom = c[ch] + (d[ch] - c[ch]) * fx;
                let value = top + (bottom - top) * fy;
                out[ch * plane + v * size + u] = (value - MEAN[ch] * 255.0) / (STD[ch] * 255.0);
            }
        }
    }
    out
}

/// The centred 2-D Hann window over the confidence map.
pub fn hann() -> [f32; MAP * MAP] {
    let w: Vec<f32> = (0..MAP)
        .map(|i| {
            0.5 * (1.0 - (2.0 * std::f32::consts::PI / (MAP as f32 + 1.0) * (i as f32 + 1.0)).cos())
        })
        .collect();
    std::array::from_fn(|i| w[i / MAP] * w[i % MAP])
}

/// The box in the next frame from the network's three outputs (`conf`
/// 16×16, `size` and `offset` 2×16×16), or `None` with the score when the
/// object is lost.
pub fn update(last: Rect, conf: &[f32], size: &[f32], offset: &[f32]) -> (Option<Rect>, f32) {
    let window = hann();
    let n = MAP * MAP;
    if conf.len() < n || size.len() < 2 * n || offset.len() < 2 * n {
        return (None, 0.0);
    }
    let (mut best, mut at) = (f32::MIN, 0usize);
    for i in 0..n {
        let v = conf[i] * window[i];
        if v > best {
            best = v;
            at = i;
        }
    }
    if best < SCORE_THRESHOLD {
        return (None, best);
    }
    let (row, col) = (at / MAP, at % MAP);
    let cx = (col as f32 + offset[at]) / MAP as f32;
    let cy = (row as f32 + offset[n + at]) / MAP as f32;
    let (w, h) = (size[at], size[n + at]);
    let side = crop_side(last, 4);
    let (x0, y0) = crop_origin(last, side);
    let next = Rect {
        x: ((cx - w / 2.0) * side as f32 + x0 as f32).floor() as i32,
        y: ((cy - h / 2.0) * side as f32 + y0 as f32).floor() as i32,
        w: ((w * side as f32).floor() as i32).max(1),
        h: ((h * side as f32).floor() as i32).max(1),
    };
    (Some(next), best)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hann_window_peaks_in_the_middle() {
        let w = hann();
        let centre = w[7 * MAP + 7].max(w[8 * MAP + 8]);
        assert!(centre > 0.95);
        assert!(w[0] < 0.02);
    }

    #[test]
    fn a_crop_off_the_frame_is_black_and_normalised() {
        // A white 4x4 frame; a box in the corner, so most of the crop is
        // outside the frame.
        let rgba = vec![255u8; 4 * 4 * 4];
        let t = crop_tensor(
            &rgba,
            4,
            4,
            Rect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            },
            4,
            8,
        );
        let black_blue = (0.0 - MEAN[0] * 255.0) / (STD[0] * 255.0);
        let white_blue = (255.0 - MEAN[0] * 255.0) / (STD[0] * 255.0);
        assert!((t[0] - black_blue).abs() < 1e-4, "{}", t[0]);
        // The crop starts 3 px left of and above the frame and is not
        // scaled (side 8 into 8), so crop pixel (5, 5) is frame pixel (2, 2).
        assert!((t[5 * 8 + 5] - white_blue).abs() < 1e-3, "{}", t[45]);
    }

    #[test]
    fn a_peak_in_the_middle_with_no_offset_keeps_the_box_still() {
        let last = Rect {
            x: 100,
            y: 50,
            w: 40,
            h: 40,
        };
        let n = MAP * MAP;
        let mut conf = vec![0.0f32; n];
        // The search crop is 160 px; the box is a quarter of it, centred:
        // cell (8, 8) with offset 0 puts the centre at 0.5.
        conf[8 * MAP + 8] = 0.9;
        let mut size = vec![0.0f32; 2 * n];
        size[8 * MAP + 8] = 0.25;
        size[n + 8 * MAP + 8] = 0.25;
        let offset = vec![0.0f32; 2 * n];
        let (next, score) = update(last, &conf, &size, &offset);
        assert!(score > 0.8);
        assert_eq!(next.unwrap(), last);
    }

    #[test]
    fn a_weak_peak_is_lost() {
        let n = MAP * MAP;
        let conf = vec![0.1f32; n];
        let (next, score) = update(
            Rect {
                x: 0,
                y: 0,
                w: 10,
                h: 10,
            },
            &conf,
            &vec![0.2; 2 * n],
            &vec![0.0; 2 * n],
        );
        assert!(next.is_none() && score < SCORE_THRESHOLD);
    }
}
