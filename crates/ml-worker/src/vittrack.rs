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
//!
//! **Re-finding.** OpenCV's tracker only ever looks around the last box, so
//! an object that passes behind something and comes out further on, or
//! leaves the frame and comes back elsewhere, is never found again. When the
//! caller asks for it ([`scan_boxes`]), the worker runs the same template
//! against search windows laid over the whole frame and takes the strongest
//! answer, without the Hann window (which exists to prefer staying put) and
//! with a stricter threshold ([`REDETECT_THRESHOLD`]), and only where the
//! colours inside the box match the object's at the start
//! ([`colour_signature`], [`MIN_COLOUR_MATCH`]): VitTrack alone found a
//! "ball" in testsrc2's colour bars while the real one was behind a bar. A box that changes size implausibly between two
//! frames ([`plausible`]) counts as lost too: that is what a tracker sliding
//! onto an occluder looks like.

/// The windowed score below which a frame is lost. OpenCV uses 0.2. On the
/// occlusion fixture a ball sliding behind a bar kept scoring 0.2–0.25 while
/// the box crept onto the bar, so the track never counted as lost and never
/// searched for the ball again; 0.3 lost a blurred frame of the thrown ball.
pub const SCORE_THRESHOLD: f32 = 0.25;
/// The windowed score above which a box is trusted as the object's size:
/// what scans are sized by and what [`plausible`] compares against, so a
/// run of doubtful frames cannot inflate the box step by step.
pub const CONFIDENT: f32 = 0.45;
/// The raw (unwindowed) confidence a whole-frame scan must reach before it
/// re-acquires the object. The held object scores 0.7–0.9 raw on the
/// fixtures; background texture stays under 0.4.
pub const REDETECT_THRESHOLD: f32 = 0.5;
/// The Bhattacharyya coefficient a re-detected box's colours must reach
/// against the object's colours in the first frame. The ball on the
/// fixtures scores above 0.8 wherever it is; the false hits scored below 0.4.
pub const MIN_COLOUR_MATCH: f32 = 0.6;
/// Levels per channel of the colour signature: coarse, so compression,
/// blur and a change of light do not move a colour into another bin.
const COLOUR_LEVELS: usize = 8;
/// How much the box's side may grow or shrink from one frame to the next.
/// A real object at 30 fps does not double in size between two frames.
pub const MAX_SIZE_STEP: f32 = 1.6;
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
/// 16×16, `size` and `offset` 2×16×16), searched around `last`, or `None`
/// with the score when the object is lost. `held` is the last box the
/// tracker was confident of; a box whose size strays too far from it is lost.
pub fn update(
    last: Rect,
    held: Rect,
    conf: &[f32],
    size: &[f32],
    offset: &[f32],
) -> (Option<Rect>, f32) {
    let (next, score) = locate(last, conf, size, offset, true);
    match next {
        Some(next) if score >= SCORE_THRESHOLD && plausible(held, next) => (Some(next), score),
        _ => (None, score),
    }
}

/// The best box from a scan window: like [`update`] but unwindowed, with
/// the box only, so the caller can compare windows by their raw score.
pub fn scan_hit(window: Rect, conf: &[f32], size: &[f32], offset: &[f32]) -> (Option<Rect>, f32) {
    locate(window, conf, size, offset, false)
}

/// The peak of the confidence map (weighted by the Hann window when
/// `windowed`) and the box it describes, in frame pixels.
fn locate(
    last: Rect,
    conf: &[f32],
    size: &[f32],
    offset: &[f32],
    windowed: bool,
) -> (Option<Rect>, f32) {
    let window = hann();
    let n = MAP * MAP;
    if conf.len() < n || size.len() < 2 * n || offset.len() < 2 * n {
        return (None, 0.0);
    }
    let (mut best, mut at) = (f32::MIN, 0usize);
    for i in 0..n {
        let v = if windowed {
            conf[i] * window[i]
        } else {
            conf[i]
        };
        if v > best {
            best = v;
            at = i;
        }
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

/// Whether `next` can be the same object as `last` (the last box held with
/// confidence): its side (the square root of the area) changed by less than
/// [`MAX_SIZE_STEP`] either way.
pub fn plausible(last: Rect, next: Rect) -> bool {
    let side = |r: Rect| ((r.w.max(1) as f32) * (r.h.max(1) as f32)).sqrt();
    let ratio = side(next) / side(last);
    (1.0 / MAX_SIZE_STEP..=MAX_SIZE_STEP).contains(&ratio)
}

/// Boxes the size of `like`, laid over a `width × height` frame so that
/// their search regions (4× the box) overlap by half: wherever the object
/// is, some window has it within a quarter of its search region from the
/// centre, where the network is reliable. Row by row from the top left; a
/// frame smaller than half a search region gives one window in the middle.
pub fn scan_boxes(width: usize, height: usize, like: Rect) -> Vec<Rect> {
    let side = crop_side(like, 4).max(1) as f32;
    let step = (side / 2.0).max(1.0);
    // The outermost centres sit a quarter of a search region in from the
    // edges, so an object entering the frame is as close to a centre as one
    // in the middle; the rest are spread evenly between them.
    let margin = side / 4.0;
    let centres = |extent: usize| -> Vec<f32> {
        let extent = extent as f32;
        let span = extent - 2.0 * margin;
        if span <= 0.0 {
            return vec![extent / 2.0];
        }
        let count = (span / step).ceil() as usize + 1;
        (0..count)
            .map(|i| margin + span * i as f32 / (count - 1) as f32)
            .collect()
    };
    let (xs, ys) = (centres(width), centres(height));
    let mut boxes = Vec::with_capacity(xs.len() * ys.len());
    for &cy in &ys {
        for &cx in &xs {
            boxes.push(Rect {
                x: (cx - like.w as f32 / 2.0).round() as i32,
                y: (cy - like.h as f32 / 2.0).round() as i32,
                w: like.w,
                h: like.h,
            });
        }
    }
    boxes
}

/// The colours inside `r` (its central 60 %, which is the object rather than
/// the background around it), as a normalised histogram of
/// [`COLOUR_LEVELS`]³ bins. Empty when the box is outside the frame.
pub fn colour_signature(rgba: &[u8], width: usize, height: usize, r: Rect) -> Vec<f32> {
    let bins = COLOUR_LEVELS * COLOUR_LEVELS * COLOUR_LEVELS;
    let mut hist = vec![0.0f32; bins];
    let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
    let (hw, hh) = (r.w as f32 * 0.3, r.h as f32 * 0.3);
    let x0 = ((cx - hw).floor() as i32).clamp(0, width as i32);
    let x1 = ((cx + hw).ceil() as i32).clamp(0, width as i32);
    let y0 = ((cy - hh).floor() as i32).clamp(0, height as i32);
    let y1 = ((cy + hh).ceil() as i32).clamp(0, height as i32);
    let q = |v: u8| v as usize * COLOUR_LEVELS / 256;
    let mut n = 0.0f32;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &rgba[(y as usize * width + x as usize) * 4..][..3];
            hist[(q(p[0]) * COLOUR_LEVELS + q(p[1])) * COLOUR_LEVELS + q(p[2])] += 1.0;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return Vec::new();
    }
    hist.iter_mut().for_each(|v| *v /= n);
    hist
}

/// How alike two colour signatures are: the Bhattacharyya coefficient, 1
/// for the same colours, 0 for none in common.
pub fn colour_match(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| (x * y).sqrt()).sum()
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
        let (next, score) = update(last, last, &conf, &size, &offset);
        assert!(score > 0.8);
        assert_eq!(next.unwrap(), last);
    }

    #[test]
    fn a_box_that_doubles_in_one_frame_is_lost() {
        let last = Rect {
            x: 100,
            y: 50,
            w: 40,
            h: 40,
        };
        let n = MAP * MAP;
        let mut conf = vec![0.0f32; n];
        conf[8 * MAP + 8] = 0.9;
        // A box as large as the whole search crop: four times the side.
        let size = vec![1.0f32; 2 * n];
        let (next, score) = update(last, last, &conf, &size, &vec![0.0; 2 * n]);
        assert!(score > 0.8);
        assert!(next.is_none());
    }

    #[test]
    fn the_scan_covers_the_frame_with_overlapping_windows() {
        let like = Rect {
            x: 0,
            y: 0,
            w: 24,
            h: 24,
        };
        let boxes = scan_boxes(640, 360, like);
        let side = crop_side(like, 4) as f32;
        // Every point of the frame is within a quarter of the search
        // region of some window's centre.
        for py in (0..360).step_by(7) {
            for px in (0..640).step_by(7) {
                let near = boxes.iter().any(|b| {
                    let cx = b.x as f32 + b.w as f32 / 2.0;
                    let cy = b.y as f32 + b.h as f32 / 2.0;
                    (cx - px as f32).abs() <= side / 4.0 + 1.0
                        && (cy - py as f32).abs() <= side / 4.0 + 1.0
                });
                assert!(near, "({px}, {py}) is not covered");
            }
        }
        assert!(boxes.len() < 120, "{} windows", boxes.len());
        // A box whose search region dwarfs the frame: one window.
        let big = Rect {
            x: 0,
            y: 0,
            w: 400,
            h: 400,
        };
        assert_eq!(scan_boxes(640, 360, big).len(), 1);
    }

    #[test]
    fn colours_match_themselves_and_not_another_colour() {
        // Left half orange, right half blue, 8x4.
        let mut rgba = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                rgba.extend_from_slice(if x < 4 {
                    &[250, 140, 20, 255]
                } else {
                    &[20, 60, 220, 255]
                });
            }
        }
        let at = |x| Rect {
            x,
            y: 0,
            w: 4,
            h: 4,
        };
        let orange = colour_signature(&rgba, 8, 4, at(0));
        let blue = colour_signature(&rgba, 8, 4, at(4));
        assert!((colour_match(&orange, &orange) - 1.0).abs() < 1e-5);
        assert!(colour_match(&orange, &blue) < 1e-5);
        assert!(colour_signature(&rgba, 8, 4, at(40)).is_empty());
    }

    #[test]
    fn a_weak_peak_is_lost() {
        let n = MAP * MAP;
        let conf = vec![0.1f32; n];
        let last = Rect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        };
        let (next, score) = update(last, last, &conf, &vec![0.2; 2 * n], &vec![0.0; 2 * n]);
        assert!(next.is_none() && score < SCORE_THRESHOLD);
    }
}
