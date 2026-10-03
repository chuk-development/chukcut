//! A colour model of the object: which colours are the object rather than
//! what surrounds it, and where in a frame those colours are.
//!
//! The classic way to follow a ball (CamShift's back-projection). Two
//! colour histograms — the inside of the box, and a ring around it — give for
//! every colour the chance that a pixel of that colour is the object. That
//! map, summed over a box-sized window, peaks where the object is, however
//! fast it moved, however blurred it is, and whatever background it crossed —
//! the three things a feature tracker and a pixel template are worst at. It
//! knows nothing about scale or rotation; the flow supplies those.
//!
//! The ring is re-measured on every confident frame, so the model learns the
//! backgrounds the object moves over.

use super::klt::Gray;
use super::tracker::{BoxState, Frame};

/// Levels per channel. 16 keeps a red ball on a red wall apart (230 vs 255
/// fall in neighbouring bins) without making the histograms sparse.
const LEVELS: usize = 16;
const BINS: usize = LEVELS * LEVELS * LEVELS;

/// The object's colours against its surroundings.
#[derive(Clone)]
pub struct ColourModel {
    object: Vec<f32>,
    background: Vec<f32>,
    ratio: Vec<f32>,
    /// How well the colours tell object from background, `0..1`: the mean
    /// objectness of the object's own pixels minus that of the ring's.
    pub distinctness: f32,
}

#[inline]
fn bin(r: f32, g: f32, b: f32) -> usize {
    let q = |v: f32| ((v.clamp(0.0, 255.0) as usize) * LEVELS / 256).min(LEVELS - 1);
    (q(r) * LEVELS + q(g)) * LEVELS + q(b)
}

/// The colour level whose pixels make the box about 32 across: fine enough
/// for a small ball, coarse enough to be cheap and to blur compression noise.
fn level_for(frame: &Frame, b: &BoxState) -> usize {
    let mut level = 0;
    let mut size = b.w.max(b.h);
    while level + 1 < frame.colour.len() && size > 64.0 {
        size /= 2.0;
        level += 1;
    }
    level
}

fn planes(frame: &Frame, level: usize) -> &[Gray; 3] {
    &frame.colour[level]
}

impl ColourModel {
    pub fn new(frame: &Frame, b: &BoxState) -> Self {
        let mut model = Self {
            object: vec![0.0; BINS],
            background: vec![0.0; BINS],
            ratio: vec![0.0; BINS],
            distinctness: 0.0,
        };
        let (object, background) = histograms(frame, b);
        model.object = object;
        model.background = background;
        model.refresh();
        model
    }

    /// Learn from a frame where the box is known to be right: the object's
    /// colours slowly, the surroundings at once.
    pub fn update(&mut self, frame: &Frame, b: &BoxState, rate: f32) {
        let (object, background) = histograms(frame, b);
        for (o, n) in self.object.iter_mut().zip(&object) {
            *o = (1.0 - rate) * *o + rate * n;
        }
        for (o, n) in self.background.iter_mut().zip(&background) {
            *o = 0.5 * *o + 0.5 * n;
        }
        self.refresh();
    }

    fn refresh(&mut self) {
        for i in 0..BINS {
            let (o, b) = (self.object[i], self.background[i]);
            self.ratio[i] = if o + b > 1e-9 { o / (o + b) } else { 0.0 };
        }
        let inside: f32 = self
            .object
            .iter()
            .zip(&self.ratio)
            .map(|(o, r)| o * r)
            .sum();
        let outside: f32 = self
            .background
            .iter()
            .zip(&self.ratio)
            .map(|(b, r)| b * r)
            .sum();
        self.distinctness = (inside - outside).clamp(0.0, 1.0);
    }

    /// Where the object most likely is within `radius` (base pixels) of
    /// `around`, and how strongly: the box-sized window with the highest mean
    /// objectness, refined to a weighted centroid.
    pub fn detect(&self, frame: &Frame, around: &BoxState, radius: f32) -> Option<(f32, f32, f32)> {
        let level = level_for(frame, around);
        let scale = (1u32 << level) as f32;
        let [r, g, b] = planes(frame, level);
        let (w, h) = (r.width as i32, r.height as i32);
        let (bw, bh) = ((around.w / scale).max(2.0), (around.h / scale).max(2.0));
        let rad = radius / scale;
        // The window the likelihood is computed over, in level pixels.
        let x0 = ((around.cx / scale - rad - bw).floor() as i32).clamp(0, w - 1);
        let y0 = ((around.cy / scale - rad - bh).floor() as i32).clamp(0, h - 1);
        let x1 = ((around.cx / scale + rad + bw).ceil() as i32).clamp(0, w - 1);
        let y1 = ((around.cy / scale + rad + bh).ceil() as i32).clamp(0, h - 1);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (ww, wh) = ((x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize);

        // Integral image of the objectness map.
        let mut integral = vec![0.0f64; (ww + 1) * (wh + 1)];
        let mut map = vec![0.0f32; ww * wh];
        for y in 0..wh {
            let mut row = 0.0f64;
            for x in 0..ww {
                let (px, py) = (x0 as usize + x, y0 as usize + y);
                let i = py * r.width + px;
                let p = self.ratio[bin(r.data[i], g.data[i], b.data[i])];
                map[y * ww + x] = p;
                row += p as f64;
                integral[(y + 1) * (ww + 1) + x + 1] = integral[y * (ww + 1) + x + 1] + row;
            }
        }
        let rect_mean = |cx: f32, cy: f32, hw: f32, hh: f32| -> f64 {
            let ax = ((cx - hw).round() as i32 - x0).clamp(0, ww as i32) as usize;
            let ay = ((cy - hh).round() as i32 - y0).clamp(0, wh as i32) as usize;
            let bx = ((cx + hw).round() as i32 - x0).clamp(0, ww as i32) as usize;
            let by = ((cy + hh).round() as i32 - y0).clamp(0, wh as i32) as usize;
            let area = ((bx - ax) * (by - ay)) as f64;
            if area < 1.0 {
                return 0.0;
            }
            let sum = integral[by * (ww + 1) + bx]
                - integral[ay * (ww + 1) + bx]
                - integral[by * (ww + 1) + ax]
                + integral[ay * (ww + 1) + ax];
            sum / area
        };

        // Inner 70 % of the box against the ring out to 1.5× the box: the
        // object's colours inside, not also all around.
        let (ihw, ihh) = (0.35 * bw, 0.35 * bh);
        let (ohw, ohh) = (0.75 * bw, 0.75 * bh);
        let mut best: Option<(f32, f32, f32)> = None;
        let (cx0, cy0) = (around.cx / scale, around.cy / scale);
        let steps = rad.ceil() as i32;
        for dy in -steps..=steps {
            for dx in -steps..=steps {
                let (cx, cy) = (cx0 + dx as f32, cy0 + dy as f32);
                let inner = rect_mean(cx, cy, ihw, ihh);
                let outer_sum = rect_mean(cx, cy, ohw, ohh) * (4.0 * ohw * ohh) as f64
                    - inner * (4.0 * ihw * ihh) as f64;
                let ring = outer_sum / (4.0 * (ohw * ohh - ihw * ihh)).max(1.0) as f64;
                let score = (inner - 0.5 * ring) as f32;
                if best.is_none_or(|(_, _, s)| score > s) {
                    best = Some((cx, cy, score));
                }
            }
        }
        let (mut cx, mut cy, score) = best?;

        // Mean-shift refinement: the objectness centroid under the box.
        for _ in 0..4 {
            let (mut sx, mut sy, mut sw) = (0.0f32, 0.0f32, 0.0f32);
            let ax = ((cx - 0.5 * bw).floor() as i32 - x0).clamp(0, ww as i32 - 1);
            let ay = ((cy - 0.5 * bh).floor() as i32 - y0).clamp(0, wh as i32 - 1);
            let bx = ((cx + 0.5 * bw).ceil() as i32 - x0).clamp(0, ww as i32 - 1);
            let by = ((cy + 0.5 * bh).ceil() as i32 - y0).clamp(0, wh as i32 - 1);
            for y in ay..=by {
                for x in ax..=bx {
                    let p = map[y as usize * ww + x as usize];
                    let (px, py) = ((x + x0) as f32, (y + y0) as f32);
                    let u = (px - cx) / (0.5 * bw);
                    let v = (py - cy) / (0.5 * bh);
                    let k = (1.0 - u * u - v * v).max(0.0);
                    sx += p * k * px;
                    sy += p * k * py;
                    sw += p * k;
                }
            }
            if sw < 1e-3 {
                break;
            }
            let (nx, ny) = (sx / sw, sy / sw);
            let moved = (nx - cx).abs() + (ny - cy).abs();
            cx = nx;
            cy = ny;
            if moved < 0.05 {
                break;
            }
        }
        // Pixel centres sit on integer coordinates at every level, as the
        // pyramid's downsampling centres them, so a level maps by scale alone.
        Some((cx * scale, cy * scale, score))
    }

    /// The object's area in base pixels, as objectness summed over a window
    /// half again the size of the box (CamShift's zeroth moment). Absolute,
    /// so a size measured this way does not drift the way chained per-frame
    /// scale estimates do.
    pub fn mass(&self, frame: &Frame, around: &BoxState) -> f32 {
        let level = level_for(frame, around);
        let scale = (1u32 << level) as f32;
        let [r, g, b] = planes(frame, level);
        let (w, h) = (r.width as i32, r.height as i32);
        let (cx, cy) = (around.cx / scale, around.cy / scale);
        let (hw, hh) = (0.8 * around.w / scale, 0.8 * around.h / scale);
        let x0 = ((cx - hw).floor() as i32).clamp(0, w - 1);
        let x1 = ((cx + hw).ceil() as i32).clamp(0, w - 1);
        let y0 = ((cy - hh).floor() as i32).clamp(0, h - 1);
        let y1 = ((cy + hh).ceil() as i32).clamp(0, h - 1);
        let mut sum = 0.0f32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = y as usize * r.width + x as usize;
                let p = self.ratio[bin(r.data[i], g.data[i], b.data[i])];
                // Only confident pixels: the ring's own colours sit near
                // one half and would make the area grow with the window.
                if p > 0.5 {
                    sum += p;
                }
            }
        }
        sum * scale * scale
    }
}

/// Object histogram (inside the inscribed ellipse, weighted towards the
/// centre) and background histogram (the ring out to twice the box), each
/// normalised to sum to one.
fn histograms(frame: &Frame, b: &BoxState) -> (Vec<f32>, Vec<f32>) {
    let level = level_for(frame, b);
    let scale = (1u32 << level) as f32;
    let [r, g, bl] = planes(frame, level);
    let (w, h) = (r.width as i32, r.height as i32);
    let (cx, cy) = (b.cx / scale, b.cy / scale);
    let (hw, hh) = ((0.5 * b.w / scale).max(1.0), (0.5 * b.h / scale).max(1.0));
    let mut object = vec![0.0f32; BINS];
    let mut background = vec![0.0f32; BINS];
    let x0 = ((cx - 2.0 * hw).floor() as i32).clamp(0, w - 1);
    let x1 = ((cx + 2.0 * hw).ceil() as i32).clamp(0, w - 1);
    let y0 = ((cy - 2.0 * hh).floor() as i32).clamp(0, h - 1);
    let y1 = ((cy + 2.0 * hh).ceil() as i32).clamp(0, h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let u = (x as f32 - cx) / hw;
            let v = (y as f32 - cy) / hh;
            let i = y as usize * r.width + x as usize;
            let k = bin(r.data[i], g.data[i], bl.data[i]);
            let d2 = u * u + v * v;
            if d2 < 1.0 {
                object[k] += 1.0 - d2;
            } else if u.abs() > 1.0 || v.abs() > 1.0 {
                background[k] += 1.0;
            }
        }
    }
    let normalise = |h: &mut Vec<f32>| {
        let sum: f32 = h.iter().sum();
        if sum > 0.0 {
            h.iter_mut().for_each(|v| *v /= sum);
        }
    };
    normalise(&mut object);
    normalise(&mut background);
    (object, background)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disc of `colour` at `(cx, cy)` over vertical bars.
    fn scene(cx: f32, cy: f32, colour: [u8; 3]) -> Frame {
        let (w, h) = (320usize, 240usize);
        let bars: [[u8; 3]; 4] = [[255, 0, 0], [0, 255, 0], [255, 255, 0], [0, 0, 255]];
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let inside = (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2) < 15.0f32.powi(2);
                let c = if inside { colour } else { bars[x * 4 / w] };
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        Frame::from_rgba(&rgba, w, h, 4)
    }

    #[test]
    fn a_red_disc_is_found_across_bars_including_a_red_one() {
        let start = BoxState {
            cx: 40.0,
            cy: 120.0,
            w: 32.0,
            h: 32.0,
            angle: 0.0,
        };
        let model = ColourModel::new(&scene(40.0, 120.0, [230, 30, 40]), &start);
        assert!(model.distinctness > 0.5, "{}", model.distinctness);
        for (x, y) in [(60.0, 110.0), (95.0, 130.0), (150.0, 100.0)] {
            let frame = scene(x, y, [230, 30, 40]);
            let around = BoxState {
                cx: x - 12.0,
                cy: y + 9.0,
                ..start
            };
            let (fx, fy, score) = model.detect(&frame, &around, 30.0).expect("found");
            assert!(
                (fx - x).abs() < 1.0 && (fy - y).abs() < 1.0,
                "disc at ({x}, {y}) found at ({fx}, {fy}), score {score}"
            );
        }
    }
}
