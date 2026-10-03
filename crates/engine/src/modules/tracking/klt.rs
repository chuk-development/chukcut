//! Pyramidal Lucas–Kanade optical flow, in plain Rust.
//!
//! Bouguet's formulation ("Pyramidal implementation of the Lucas Kanade
//! feature tracker", 2000): a Gaussian pyramid of each frame, Shi–Tomasi
//! corners to track, and per point an iterative solve of the 2×2 normal
//! equations from the coarsest level down, each level starting from the one
//! above's estimate. The pyramid is what lets a window of 15 px follow a
//! motion of 60 px. No SIMD and no threads: at the analysis size (640 px
//! long side) a frame of sixty points takes a few milliseconds, well under
//! the decode that feeds it.

/// A single-channel image, values `0..255`, row-major.
#[derive(Debug, Clone)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Gray {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    /// Luma (BT.601 weights) from tightly packed RGBA8.
    pub fn from_rgba(rgba: &[u8], width: usize, height: usize) -> Self {
        let data = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .take(width * height)
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        Self {
            width,
            height,
            data,
        }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> f32 {
        self.data[y * self.width + x]
    }

    /// Bilinear sample, clamped to the edge.
    #[inline]
    pub fn sample(&self, x: f32, y: f32) -> f32 {
        let max_x = (self.width - 1) as f32;
        let max_y = (self.height - 1) as f32;
        let x = x.clamp(0.0, max_x);
        let y = y.clamp(0.0, max_y);
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let x0 = x0 as usize;
        let y0 = y0 as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let row0 = y0 * self.width;
        let row1 = y1 * self.width;
        let a = self.data[row0 + x0];
        let b = self.data[row0 + x1];
        let c = self.data[row1 + x0];
        let d = self.data[row1 + x1];
        let top = a + (b - a) * fx;
        let bottom = c + (d - c) * fx;
        top + (bottom - top) * fy
    }

    /// Half the size, after a 5-tap binomial blur (`[1 4 6 4 1] / 16`), which
    /// is the pyramid filter Bouguet uses.
    pub fn downsample(&self) -> Gray {
        let (w, h) = (self.width, self.height);
        let nw = w.div_ceil(2).max(1);
        let nh = h.div_ceil(2).max(1);
        const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
        let clamp = |v: isize, n: usize| v.clamp(0, n as isize - 1) as usize;

        // Horizontal pass at the destination's column spacing only.
        let mut horizontal = vec![0.0f32; nw * h];
        for y in 0..h {
            let row = &self.data[y * w..(y + 1) * w];
            for nx in 0..nw {
                let cx = (nx * 2) as isize;
                let mut acc = 0.0;
                for (k, weight) in K.iter().enumerate() {
                    acc += weight * row[clamp(cx + k as isize - 2, w)];
                }
                horizontal[y * nw + nx] = acc;
            }
        }
        let mut out = Gray::new(nw, nh);
        for ny in 0..nh {
            let cy = (ny * 2) as isize;
            for nx in 0..nw {
                let mut acc = 0.0;
                for (k, weight) in K.iter().enumerate() {
                    acc += weight * horizontal[clamp(cy + k as isize - 2, h) * nw + nx];
                }
                out.data[ny * nw + nx] = acc;
            }
        }
        out
    }

    /// Scharr derivatives, normalised so a unit ramp has gradient 1.
    fn gradients(&self) -> (Gray, Gray) {
        let (w, h) = (self.width, self.height);
        let mut gx = Gray::new(w, h);
        let mut gy = Gray::new(w, h);
        if w < 3 || h < 3 {
            return (gx, gy);
        }
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let p = |dx: isize, dy: isize| {
                    self.data[(y as isize + dy) as usize * w + (x as isize + dx) as usize]
                };
                let dx = 3.0 * (p(1, -1) - p(-1, -1))
                    + 10.0 * (p(1, 0) - p(-1, 0))
                    + 3.0 * (p(1, 1) - p(-1, 1));
                let dy = 3.0 * (p(-1, 1) - p(-1, -1))
                    + 10.0 * (p(0, 1) - p(0, -1))
                    + 3.0 * (p(1, 1) - p(1, -1));
                gx.data[y * w + x] = dx / 32.0;
                gy.data[y * w + x] = dy / 32.0;
            }
        }
        (gx, gy)
    }
}

/// One level of a pyramid: the image and its derivatives.
#[derive(Debug, Clone)]
pub struct Level {
    pub image: Gray,
    pub gx: Gray,
    pub gy: Gray,
}

/// A frame as the tracker sees it.
#[derive(Debug, Clone)]
pub struct Pyramid {
    pub levels: Vec<Level>,
}

impl Pyramid {
    /// Up to `max_levels` levels, stopping before a level gets narrower than
    /// a tracking window is useful on.
    pub fn new(base: Gray, max_levels: usize) -> Self {
        let mut images = vec![base];
        while images.len() < max_levels.max(1) {
            let last = images.last().expect("at least the base");
            if last.width.min(last.height) < 40 {
                break;
            }
            let next = last.downsample();
            images.push(next);
        }
        let levels = images
            .into_iter()
            .map(|image| {
                let (gx, gy) = image.gradients();
                Level { image, gx, gy }
            })
            .collect();
        Self { levels }
    }

    pub fn base(&self) -> &Gray {
        &self.levels[0].image
    }

    pub fn width(&self) -> usize {
        self.base().width
    }

    pub fn height(&self) -> usize {
        self.base().height
    }
}

/// Shi–Tomasi corners inside `region` (`x0, y0, x1, y1`, base pixels).
///
/// The smaller eigenvalue of the 5×5 structure tensor is the corner score; a
/// point is kept when it is a local maximum, above `quality` times the best
/// score in the region, and at least `min_distance` from every stronger
/// point already kept. Strongest first.
pub fn good_features(
    level: &Level,
    region: (f32, f32, f32, f32),
    max_points: usize,
    quality: f32,
    min_distance: f32,
) -> Vec<(f32, f32)> {
    let img = &level.image;
    let (w, h) = (img.width as isize, img.height as isize);
    let x0 = (region.0.floor() as isize).clamp(3, w - 4);
    let y0 = (region.1.floor() as isize).clamp(3, h - 4);
    let x1 = (region.2.ceil() as isize).clamp(3, w - 4);
    let y1 = (region.3.ceil() as isize).clamp(3, h - 4);
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let rw = (x1 - x0 + 1) as usize;
    let rh = (y1 - y0 + 1) as usize;
    let mut score = vec![0.0f32; rw * rh];
    let mut best = 0.0f32;
    for ry in 0..rh {
        for rx in 0..rw {
            let (cx, cy) = (x0 + rx as isize, y0 + ry as isize);
            let (mut a, mut b, mut c) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -2..=2 {
                let row = ((cy + dy) * w) as usize;
                for dx in -2..=2 {
                    let i = row + (cx + dx) as usize;
                    let gx = level.gx.data[i];
                    let gy = level.gy.data[i];
                    a += gx * gx;
                    b += gx * gy;
                    c += gy * gy;
                }
            }
            let half_trace = 0.5 * (a + c);
            let det_term = (0.25 * (a - c) * (a - c) + b * b).sqrt();
            let min_eig = half_trace - det_term;
            score[ry * rw + rx] = min_eig;
            best = best.max(min_eig);
        }
    }
    if best <= 1e-3 {
        return Vec::new();
    }
    let threshold = best * quality;
    let mut candidates: Vec<(f32, usize, usize)> = Vec::new();
    for ry in 1..rh.saturating_sub(1) {
        for rx in 1..rw.saturating_sub(1) {
            let s = score[ry * rw + rx];
            if s < threshold {
                continue;
            }
            let mut is_max = true;
            'n: for ny in ry - 1..=ry + 1 {
                for nx in rx - 1..=rx + 1 {
                    if (nx, ny) != (rx, ry) && score[ny * rw + nx] > s {
                        is_max = false;
                        break 'n;
                    }
                }
            }
            if is_max {
                candidates.push((s, rx, ry));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let min_d2 = min_distance * min_distance;
    let mut kept: Vec<(f32, f32)> = Vec::new();
    for (_, rx, ry) in candidates {
        let p = ((x0 + rx as isize) as f32, (y0 + ry as isize) as f32);
        if kept.iter().all(|q| {
            let (dx, dy) = (p.0 - q.0, p.1 - q.1);
            dx * dx + dy * dy >= min_d2
        }) {
            kept.push(p);
            if kept.len() >= max_points {
                break;
            }
        }
    }
    kept
}

/// Tuning of the flow solve.
#[derive(Debug, Clone, Copy)]
pub struct FlowParams {
    /// Window half-size: the window is `2r + 1` square.
    pub radius: i32,
    pub max_iterations: usize,
    /// Stop when an update moves less than this, in level pixels.
    pub epsilon: f32,
    /// Refuse a window whose smaller gradient eigenvalue per pixel is below
    /// this: there is nothing to lock on to.
    pub min_eigen: f32,
}

impl Default for FlowParams {
    fn default() -> Self {
        Self {
            radius: 7,
            max_iterations: 20,
            epsilon: 0.01,
            min_eigen: 1e-3,
        }
    }
}

/// Where `point` of `prev` went in `next`, starting from a displacement of
/// `guess` (base pixels). `None` when the window is textureless or the
/// solve walks off the image.
pub fn track_point(
    prev: &Pyramid,
    next: &Pyramid,
    point: (f32, f32),
    guess: (f32, f32),
    params: &FlowParams,
) -> Option<(f32, f32)> {
    let levels = prev.levels.len().min(next.levels.len());
    let top = levels - 1;
    let scale_top = (1u32 << top) as f32;
    let mut g = (guess.0 / scale_top, guess.1 / scale_top);
    let r = params.radius;
    let n = ((2 * r + 1) * (2 * r + 1)) as usize;
    let mut patch = Vec::with_capacity(n);
    let mut grad_x = Vec::with_capacity(n);
    let mut grad_y = Vec::with_capacity(n);

    for level in (0..levels).rev() {
        let scale = (1u32 << level) as f32;
        let p = (point.0 / scale, point.1 / scale);
        let a = &prev.levels[level];
        let b = &next.levels[level].image;
        let (w, h) = (a.image.width as f32, a.image.height as f32);

        patch.clear();
        grad_x.clear();
        grad_y.clear();
        let (mut gxx, mut gxy, mut gyy) = (0.0f32, 0.0f32, 0.0f32);
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (p.0 + dx as f32, p.1 + dy as f32);
                let ix = a.gx.sample(x, y);
                let iy = a.gy.sample(x, y);
                patch.push(a.image.sample(x, y));
                grad_x.push(ix);
                grad_y.push(iy);
                gxx += ix * ix;
                gxy += ix * iy;
                gyy += iy * iy;
            }
        }
        let det = gxx * gyy - gxy * gxy;
        let half_trace = 0.5 * (gxx + gyy);
        let min_eig = half_trace - (0.25 * (gxx - gyy) * (gxx - gyy) + gxy * gxy).sqrt();
        if min_eig / (n as f32) < params.min_eigen || det.abs() < 1e-9 {
            return None;
        }

        let mut v = (0.0f32, 0.0f32);
        for _ in 0..params.max_iterations {
            let cx = p.0 + g.0 + v.0;
            let cy = p.1 + g.1 + v.1;
            if cx < -(r as f32) || cy < -(r as f32) || cx > w + r as f32 || cy > h + r as f32 {
                return None;
            }
            let (mut bx, mut by) = (0.0f32, 0.0f32);
            let mut k = 0;
            for dy in -r..=r {
                for dx in -r..=r {
                    let diff = patch[k] - b.sample(cx + dx as f32, cy + dy as f32);
                    bx += diff * grad_x[k];
                    by += diff * grad_y[k];
                    k += 1;
                }
            }
            let ex = (gyy * bx - gxy * by) / det;
            let ey = (gxx * by - gxy * bx) / det;
            v.0 += ex;
            v.1 += ey;
            if ex * ex + ey * ey < params.epsilon * params.epsilon {
                break;
            }
        }
        if level > 0 {
            g = (2.0 * (g.0 + v.0), 2.0 * (g.1 + v.1));
        } else {
            g = (g.0 + v.0, g.1 + v.1);
        }
    }

    let out = (point.0 + g.0, point.1 + g.1);
    let (w, h) = (prev.width() as f32, prev.height() as f32);
    if !(out.0.is_finite() && out.1.is_finite())
        || out.0 < 0.0
        || out.1 < 0.0
        || out.0 >= w
        || out.1 >= h
    {
        return None;
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Smooth, corner-rich texture: a sum of blobs. Deterministic.
    pub(crate) fn texture(width: usize, height: usize, seed: u32) -> Gray {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as f32 / u32::MAX as f32
        };
        let blobs: Vec<(f32, f32, f32, f32)> = (0..(width * height / 300).max(40))
            .map(|_| {
                (
                    next() * width as f32,
                    next() * height as f32,
                    2.0 + next() * 6.0,
                    next() * 2.0 - 1.0,
                )
            })
            .collect();
        let mut img = Gray::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let mut v = 128.0;
                for &(bx, by, r, a) in &blobs {
                    let d2 = (x as f32 - bx).powi(2) + (y as f32 - by).powi(2);
                    if d2 < 9.0 * r * r {
                        v += 90.0 * a * (-d2 / (2.0 * r * r)).exp();
                    }
                }
                img.data[y * width + x] = v.clamp(0.0, 255.0);
            }
        }
        img
    }

    pub(crate) fn shifted(img: &Gray, dx: f32, dy: f32) -> Gray {
        let mut out = Gray::new(img.width, img.height);
        for y in 0..img.height {
            for x in 0..img.width {
                out.data[y * img.width + x] = img.sample(x as f32 - dx, y as f32 - dy);
            }
        }
        out
    }

    #[test]
    fn downsampling_halves_and_keeps_the_mean() {
        let img = texture(101, 64, 3);
        let half = img.downsample();
        assert_eq!((half.width, half.height), (51, 32));
        let mean = |g: &Gray| g.data.iter().sum::<f32>() / g.data.len() as f32;
        assert!((mean(&img) - mean(&half)).abs() < 2.0);
    }

    #[test]
    fn a_shift_is_recovered_to_a_tenth_of_a_pixel_even_when_large() {
        let a = texture(320, 240, 7);
        for (dx, dy) in [(1.3f32, -0.7f32), (12.5, 4.25), (-31.0, 22.0)] {
            let b = shifted(&a, dx, dy);
            let pa = Pyramid::new(a.clone(), 4);
            let pb = Pyramid::new(b, 4);
            let points = good_features(&pa.levels[0], (60.0, 60.0, 260.0, 180.0), 30, 0.05, 8.0);
            assert!(points.len() >= 15, "found {} corners", points.len());
            let mut good = 0;
            for p in &points {
                if let Some(q) = track_point(&pa, &pb, *p, (0.0, 0.0), &FlowParams::default()) {
                    if (q.0 - p.0 - dx).abs() < 0.1 && (q.1 - p.1 - dy).abs() < 0.1 {
                        good += 1;
                    }
                }
            }
            // A shift several times the window is found from the coarse
            // levels alone, where this fine texture is nearly flat; most
            // points still make it, and the tracker's fit discards the rest.
            let wanted = if dx.abs().max(dy.abs()) > 20.0 { 5 } else { 8 };
            assert!(
                good * 10 >= points.len() * wanted,
                "shift ({dx}, {dy}): {good} of {} points within 0.1 px",
                points.len()
            );
        }
    }

    #[test]
    fn a_flat_window_is_refused() {
        let flat = Gray {
            width: 64,
            height: 64,
            data: vec![100.0; 64 * 64],
        };
        let p = Pyramid::new(flat, 3);
        assert!(track_point(&p, &p, (32.0, 32.0), (0.0, 0.0), &FlowParams::default()).is_none());
        assert!(good_features(&p.levels[0], (0.0, 0.0, 63.0, 63.0), 10, 0.1, 4.0).is_empty());
    }
}
