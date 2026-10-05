//! Removing an object from consecutive frames: the mask grown, LaMa on a
//! crop around it, the fill blended back, and the result steadied over time.
//!
//! LaMa fills each frame on its own, so a plain per-frame removal flickers:
//! the texture it invents changes from frame to frame. Two things steady it,
//! both only while the camera holds still (the picture around the mask
//! barely changes from the frame before, [`STILL`]):
//!
//! - **Background memory.** Where the object has moved on, the background
//!   behind it was seen in an earlier frame. Each pixel outside the mask is
//!   remembered; a masked pixel seen within the last [`MEMORY_FRAMES`]
//!   frames is filled with what was there, and LaMa is asked to fill only
//!   what was never seen. This is the true background, not an invention,
//!   and it does not flicker.
//! - **Smoothing.** What LaMa does fill is mixed with the previous frame's
//!   fill ([`SMOOTH`] of the old), so its texture settles instead of
//!   boiling.
//!
//! - **The clean plate.** The memory only knows the past: at the start of a
//!   clip the object covers background nobody has seen yet. A first pass
//!   over the frames ([`PlateBuilder`]) keeps, for every still stretch of
//!   the shot, the last background seen at each pixel; a masked pixel the
//!   memory does not know is filled from it. Only for a selected object,
//!   which moves: a painted stroke or a box covers the same pixels in every
//!   frame, so nothing behind it is ever seen.
//!
//! - **Following the camera.** When the camera pans or tilts, the picture
//!   around the mask moves as a whole. [`camera_move`] finds that move (a
//!   shift, to a tenth of a pixel, on a luma pyramid outside both masks);
//!   when the shift explains the change (what is left after it is below
//!   [`FOLLOWED`]), the memory, its ages and the previous frame's output
//!   are moved with the picture, and the memory and the smoothing work as in
//!   a still shot. Behind a static logo on a panning shot the background
//!   slides past: the memory then knows most of what the logo covers (it
//!   was beside the logo a few frames ago), and what LaMa still invents is
//!   steadied by the moved previous fill instead of being reinvented every
//!   frame. The memory is moved by whole pixels; the fraction is carried to
//!   the next frame, so it is never resampled (it would blur) and never
//!   more than a pixel off.
//!
//! When the camera moves in a way a shift does not explain (a zoom, a
//! rotation, a cut), all of this is dropped and the frame is filled on its
//! own: a remembered pixel would be from another place in the scene, and a
//! mixed fill would smear.
//!
//! The state is one frame's worth of pixels and lives here, in the engine,
//! so a restarted worker loses nothing.

use std::sync::atomic::AtomicBool;

use super::mask::{self, Rect};
use crate::modules::ml::inpaint;

/// The mean change (code values) of the picture around the mask from one
/// frame to the next below which the camera counts as still. Sensor noise
/// and compression move a still shot by 1–3.
pub const STILL: f32 = 4.0;

/// How many frames a remembered background pixel stays usable.
pub const MEMORY_FRAMES: u16 = 150;

/// The share of the previous frame's fill kept in a still shot.
pub const SMOOTH: f32 = 0.5;

/// The mean change (code values) left after the camera's shift is taken
/// out below which the shift counts as the camera's move. Twice [`STILL`]:
/// what a still shot's noise leaves, plus the bilinear sampling of a
/// fractional shift.
pub const FOLLOWED: f32 = 2.0 * STILL;

/// The largest camera move from one frame to the next that is followed, in
/// pixels on each axis.
pub const MAX_SHIFT: f32 = 96.0;

/// A still stretch shorter than this many frames gets no clean plate.
const MIN_PLATE_FRAMES: u32 = 3;

/// At most this many clean plates per bake (a frame of pixels each).
const MAX_PLATES: usize = 24;

/// The mean change of the picture outside both masks from `before` to
/// `now` (code values), sampled on every fourth pixel of every fourth row:
/// how much the camera moved. `f32::MAX` when nothing is left to compare.
fn change(
    w: usize,
    h: usize,
    before: &[u8],
    before_mask: &[u8],
    now: &[u8],
    now_mask: &[u8],
) -> f32 {
    let (mut sum, mut n) = (0u64, 0u64);
    for y in (0..h).step_by(4) {
        for x in (0..w).step_by(4) {
            let i = y * w + x;
            if before_mask[i] != 0 || now_mask[i] != 0 {
                continue;
            }
            for c in 0..3 {
                sum += (before[i * 4 + c] as i32 - now[i * 4 + c] as i32).unsigned_abs() as u64;
            }
            n += 3;
        }
    }
    if n == 0 {
        f32::MAX
    } else {
        sum as f32 / n as f32
    }
}

/// The luma of `rgba` (`w × h`) with a validity flag (outside `mask`).
struct Plane {
    w: usize,
    h: usize,
    luma: Vec<f32>,
    valid: Vec<bool>,
}

impl Plane {
    fn of(rgba: &[u8], mask: &[u8], w: usize, h: usize) -> Plane {
        let luma = (0..w * h)
            .map(|i| {
                let p = &rgba[i * 4..i * 4 + 3];
                0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
            })
            .collect();
        let valid = mask.iter().map(|&m| m == 0).collect();
        Plane { w, h, luma, valid }
    }

    /// Half the size, 2×2 averages; a block with a masked pixel is masked.
    fn half(&self) -> Plane {
        let (w, h) = (self.w / 2, self.h / 2);
        let mut luma = Vec::with_capacity(w * h);
        let mut valid = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let i = |dx: usize, dy: usize| (2 * y + dy) * self.w + 2 * x + dx;
                let at = [i(0, 0), i(1, 0), i(0, 1), i(1, 1)];
                luma.push(at.iter().map(|&k| self.luma[k]).sum::<f32>() / 4.0);
                valid.push(at.iter().all(|&k| self.valid[k]));
            }
        }
        Plane { w, h, luma, valid }
    }

    /// Mean absolute difference of `now` against this plane shifted by
    /// `(dx, dy)` (`now[x, y]` against `self[x − dx, y − dy]`), over every
    /// `step`-th pixel both see. `f32::MAX` when they share less than a
    /// quarter of the picture.
    fn cost(&self, now: &Plane, dx: i32, dy: i32, step: usize) -> f32 {
        let (mut sum, mut n, mut total) = (0.0f64, 0u64, 0u64);
        for y in (0..now.h).step_by(step) {
            let sy = y as i32 - dy;
            for x in (0..now.w).step_by(step) {
                total += 1;
                let sx = x as i32 - dx;
                if sx < 0 || sy < 0 || sx >= self.w as i32 || sy >= self.h as i32 {
                    continue;
                }
                let (i, j) = (y * now.w + x, sy as usize * self.w + sx as usize);
                if !now.valid[i] || !self.valid[j] {
                    continue;
                }
                sum += (now.luma[i] - self.luma[j]).abs() as f64;
                n += 1;
            }
        }
        if n == 0 || n * 4 < total {
            f32::MAX
        } else {
            (sum / n as f64) as f32
        }
    }

    /// The integer shift within `centre ± radius` with the lowest cost.
    fn search(&self, now: &Plane, centre: (i32, i32), radius: i32, step: usize) -> (i32, i32, f32) {
        let mut best = (centre.0, centre.1, f32::MAX);
        for dy in centre.1 - radius..=centre.1 + radius {
            for dx in centre.0 - radius..=centre.0 + radius {
                let c = self.cost(now, dx, dy, step);
                // Ties go to the smaller move.
                if c < best.2 || (c == best.2 && dx.abs() + dy.abs() < best.0.abs() + best.1.abs())
                {
                    best = (dx, dy, c);
                }
            }
        }
        best
    }
}

/// The vertex of the parabola through three costs at −1, 0 and +1: the
/// fraction of a pixel the minimum lies off the middle one, within ±0.5.
fn vertex(left: f32, middle: f32, right: f32) -> f32 {
    let curve = left - 2.0 * middle + right;
    if curve.is_nan() || curve <= 1e-6 || left == f32::MAX || right == f32::MAX {
        return 0.0;
    }
    (0.5 * (left - right) / curve).clamp(-0.5, 0.5)
}

/// How the camera moved from `before` to `now` (RGBA8, `w × h`, each with
/// its grown object mask): the shift `(dx, dy)` in pixels, to a tenth or
/// so, that carries the earlier picture onto the later one, and the mean
/// change left after it (code values, as [`change`] measures it). `None`
/// when the frames share too little picture to tell.
///
/// The shift is searched on a luma pyramid outside both masks: within
/// [`MAX_SHIFT`] at an eighth of the size, refined by ±2 pixels at each
/// level up, then to a fraction of a pixel by a parabola through the costs
/// around the best whole shift.
pub fn camera_move(
    w: usize,
    h: usize,
    before: &[u8],
    before_mask: &[u8],
    now: &[u8],
    now_mask: &[u8],
) -> Option<(f32, f32, f32)> {
    let mut pyramid = vec![(
        Plane::of(before, before_mask, w, h),
        Plane::of(now, now_mask, w, h),
    )];
    while pyramid.len() < 4 && pyramid.last().is_some_and(|(b, _)| b.w >= 96 && b.h >= 96) {
        let (b, n) = pyramid.last().expect("not empty");
        let next = (b.half(), n.half());
        pyramid.push(next);
    }
    let levels = pyramid.len() as i32;
    let scale = (1 << (levels - 1)) as f32;
    let radius = (MAX_SHIFT / scale).ceil() as i32 + 1;
    let (mut dx, mut dy) = (0i32, 0i32);
    let mut cost = f32::MAX;
    for (level, (b, n)) in pyramid.iter().enumerate().rev() {
        let coarsest = level as i32 == levels - 1;
        // Every pixel at the small levels, every second at full size.
        let step = if level == 0 { 2 } else { 1 };
        let found = if coarsest {
            b.search(n, (0, 0), radius, step)
        } else {
            b.search(n, (dx * 2, dy * 2), 2, step)
        };
        (dx, dy, cost) = found;
    }
    if cost == f32::MAX {
        return None;
    }
    let (b, n) = &pyramid[0];
    let fx = vertex(b.cost(n, dx - 1, dy, 2), cost, b.cost(n, dx + 1, dy, 2));
    let fy = vertex(b.cost(n, dx, dy - 1, 2), cost, b.cost(n, dx, dy + 1, 2));
    let (sx, sy) = (dx as f32 + fx, dy as f32 + fy);
    let left = shifted_change(w, h, before, before_mask, now, now_mask, sx, sy);
    (left < f32::MAX).then_some((sx, sy, left))
}

/// [`change`] with `before` shifted by `(dx, dy)` (bilinear): what the
/// camera's move does not explain.
#[allow(clippy::too_many_arguments)]
fn shifted_change(
    w: usize,
    h: usize,
    before: &[u8],
    before_mask: &[u8],
    now: &[u8],
    now_mask: &[u8],
    dx: f32,
    dy: f32,
) -> f32 {
    let (mut sum, mut n) = (0.0f64, 0u64);
    for y in (0..h).step_by(4) {
        let sy = y as f32 - dy;
        if sy < 0.0 || sy > (h - 1) as f32 {
            continue;
        }
        for x in (0..w).step_by(4) {
            let sx = x as f32 - dx;
            if sx < 0.0 || sx > (w - 1) as f32 {
                continue;
            }
            let i = y * w + x;
            let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
            let corners = [y0 * w + x0, y0 * w + x1, y1 * w + x0, y1 * w + x1];
            if now_mask[i] != 0 || corners.iter().any(|&k| before_mask[k] != 0) {
                continue;
            }
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let weights = [
                (1.0 - fx) * (1.0 - fy),
                fx * (1.0 - fy),
                (1.0 - fx) * fy,
                fx * fy,
            ];
            for c in 0..3 {
                let v: f32 = corners
                    .iter()
                    .zip(weights)
                    .map(|(&k, wt)| before[k * 4 + c] as f32 * wt)
                    .sum();
                sum += (v - now[i * 4 + c] as f32).abs() as f64;
            }
            n += 3;
        }
    }
    if n == 0 {
        f32::MAX
    } else {
        (sum / n as f64) as f32
    }
}

/// `values` (`w × h` of `T`, `channels` per pixel) moved by whole pixels
/// `(dx, dy)`: `out[x, y] = values[x − dx, y − dy]`, `fill` where that lies
/// outside.
fn shift<T: Copy>(
    values: &[T],
    w: usize,
    h: usize,
    channels: usize,
    dx: i32,
    dy: i32,
    fill: T,
) -> Vec<T> {
    let mut out = vec![fill; values.len()];
    for y in 0..h {
        let sy = y as i32 - dy;
        if sy < 0 || sy >= h as i32 {
            continue;
        }
        let (x0, x1) = (
            (dx.max(0) as usize).min(w),
            ((w as i32 + dx.min(0)).max(0) as usize).min(w),
        );
        if x0 >= x1 {
            continue;
        }
        let row = y * w * channels;
        let src = sy as usize * w * channels;
        let from = (x0 as i32 - dx) as usize;
        out[row + x0 * channels..row + x1 * channels]
            .copy_from_slice(&values[src + from * channels..src + (from + x1 - x0) * channels]);
    }
    out
}

/// The background of one still stretch of a shot: the last value seen at
/// each pixel outside the (grown) mask, from its first frame to its last.
pub struct Plate {
    pub first: crate::modules::project::document::Micros,
    pub last: crate::modules::project::document::Micros,
    rgba: Vec<u8>,
    seen: Vec<bool>,
}

/// The clean plates of a run of consecutive frames, built in a first pass.
pub struct PlateBuilder {
    width: usize,
    height: usize,
    grow: usize,
    previous: Option<(Vec<u8>, Vec<u8>)>,
    current: Option<(Plate, u32)>,
    plates: Vec<Plate>,
}

impl PlateBuilder {
    /// A builder for `width × height` frames whose mask grows by `grow` (a
    /// fraction of the shorter side), as the [`Remover`] grows it.
    pub fn new(width: usize, height: usize, grow: f32) -> Self {
        Self {
            width,
            height,
            grow: grow_pixels(width, height, grow),
            previous: None,
            current: None,
            plates: Vec::new(),
        }
    }

    /// The next frame (shown from `pts`) and its object mask.
    pub fn add(
        &mut self,
        pts: crate::modules::project::document::Micros,
        frame: &[u8],
        object: &[u8],
    ) {
        let (w, h) = (self.width, self.height);
        let grown = mask::grow(object, w, h, self.grow);
        let still = self.previous.as_ref().is_some_and(|(before, before_mask)| {
            change(w, h, before, before_mask, frame, &grown) < STILL
        });
        if !still {
            self.close();
            if self.plates.len() < MAX_PLATES {
                self.current = Some((
                    Plate {
                        first: pts,
                        last: pts,
                        rgba: vec![0; w * h * 4],
                        seen: vec![false; w * h],
                    },
                    0,
                ));
            }
        }
        if let Some((plate, frames)) = &mut self.current {
            for (i, &m) in grown.iter().enumerate() {
                if m == 0 {
                    plate.rgba[i * 4..i * 4 + 4].copy_from_slice(&frame[i * 4..i * 4 + 4]);
                    plate.seen[i] = true;
                }
            }
            plate.last = pts;
            *frames += 1;
        }
        self.previous = Some((frame.to_vec(), grown));
    }

    fn close(&mut self) {
        if let Some((plate, frames)) = self.current.take() {
            if frames >= MIN_PLATE_FRAMES {
                self.plates.push(plate);
            }
        }
    }

    /// The plates, in order.
    pub fn finish(mut self) -> Vec<Plate> {
        self.close();
        self.plates
    }
}

/// The plate of the still stretch that shows `pts`, if any.
pub fn plate_at(
    plates: &[Plate],
    pts: crate::modules::project::document::Micros,
) -> Option<&Plate> {
    plates.iter().find(|p| (p.first..=p.last).contains(&pts))
}

fn grow_pixels(width: usize, height: usize, grow: f32) -> usize {
    (grow * width.min(height) as f32).round().max(1.0) as usize
}

/// Removal over consecutive frames of one size.
pub struct Remover {
    width: usize,
    height: usize,
    /// The grown mask's radius and the soft edge's, in pixels.
    grow: usize,
    feather: usize,
    /// The previous input frame (RGBA8), its grown mask and its output.
    previous: Option<(Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// The last background seen at each pixel (RGBA8) and how many frames
    /// ago (`u16::MAX`: never).
    memory: Vec<u8>,
    age: Vec<u16>,
    /// The camera's move summed since the first frame, and the whole
    /// pixels of it the memory has been moved by (`camera_move`).
    travelled: (f32, f32),
    moved: (i32, i32),
    /// Follow a moving camera (on by default). Off is the removal as it was
    /// before it could, for a comparison.
    pub follow_camera: bool,
    /// How many frames were followed through a camera move.
    pub followed: u32,
    /// Where the model ran, once it has.
    pub provider: Option<String>,
    /// The model's own time, summed.
    pub model_millis: f64,
    pub model_runs: u32,
}

impl Remover {
    /// A remover for `width × height` frames whose mask grows by `grow` (a
    /// fraction of the shorter side, `ObjectRemoval::grow`).
    pub fn new(width: usize, height: usize, grow: f32) -> Self {
        let grow_px = grow_pixels(width, height, grow);
        Self {
            width,
            height,
            grow: grow_px,
            feather: (grow_px / 2).max(1),
            previous: None,
            memory: vec![0; width * height * 4],
            age: vec![u16::MAX; width * height],
            travelled: (0.0, 0.0),
            moved: (0, 0),
            follow_camera: true,
            followed: 0,
            provider: None,
            model_millis: 0.0,
            model_runs: 0,
        }
    }

    fn change(&self, before: &[u8], before_mask: &[u8], now: &[u8], now_mask: &[u8]) -> f32 {
        change(self.width, self.height, before, before_mask, now, now_mask)
    }

    /// Remove what `object` (a `width × height` mask, nonzero = remove)
    /// covers from `frame` (RGBA8), with the clean plate of its still
    /// stretch when there is one. Frames must come in order; a jump (a
    /// seek) is just a frame whose picture changed a lot, which drops the
    /// memory and the smoothing on its own.
    pub fn next(
        &mut self,
        frame: &[u8],
        object: &[u8],
        plate: Option<&Plate>,
        cancel: Option<&AtomicBool>,
    ) -> Result<Vec<u8>, String> {
        let (w, h) = (self.width, self.height);
        let grown = mask::grow(object, w, h, self.grow);
        let Some(_) = mask::bounds(&grown, w, h) else {
            // Nothing to remove in this frame: the picture as it is, and
            // everything it shows is background.
            self.remember(frame, &grown);
            self.previous = Some((frame.to_vec(), grown, frame.to_vec()));
            return Ok(frame.to_vec());
        };
        let still = self.steady(frame, &grown);
        if !still {
            self.age.fill(u16::MAX);
        }

        // Fill what the memory knows; LaMa gets the rest.
        let mut picture = frame.to_vec();
        let mut unknown = grown.clone();
        for (i, m) in unknown.iter_mut().enumerate() {
            if *m == 0 {
                continue;
            }
            let from = if still && self.age[i] <= MEMORY_FRAMES {
                &self.memory
            } else if let Some(plate) = plate.filter(|p| p.seen[i]) {
                &plate.rgba
            } else {
                continue;
            };
            picture[i * 4..i * 4 + 4].copy_from_slice(&from[i * 4..i * 4 + 4]);
            *m = 0;
        }
        let mut out = picture.clone();
        if let Some(bounds) = mask::bounds(&unknown, w, h) {
            let rect = mask::crop_for(bounds, w, h);
            let crop = mask::crop(&picture, w, 4, rect);
            let crop_mask = mask::crop(&unknown, w, 1, rect);
            let filled = inpaint::inpaint(&crop, &crop_mask, rect.width(), rect.height(), cancel)
                .map_err(|e| match e {
                crate::modules::ml::MlError::Cancelled => "cancelled".to_string(),
                other => other.to_string(),
            })?;
            self.provider.get_or_insert(filled.provider.clone());
            self.model_millis += filled.millis as f64;
            self.model_runs += 1;
            let soft = mask::feather(&unknown, w, h, self.feather);
            self.paste(&mut out, &filled.rgba, rect, &soft);
            if still {
                self.smooth(&mut out, &unknown);
            }
        }
        self.remember(frame, &grown);
        self.previous = Some((frame.to_vec(), grown, out.clone()));
        Ok(out)
    }

    /// Whether this frame continues the last one: the camera held still,
    /// or it moved by a shift that explains the change, in which case the
    /// memory, its ages and the previous frame are moved with the picture
    /// first (by whole pixels, the fraction carried over).
    fn steady(&mut self, frame: &[u8], grown: &[u8]) -> bool {
        let (w, h) = (self.width, self.height);
        let Some((before, before_mask, _)) = &self.previous else {
            return false;
        };
        if self.change(before, before_mask, frame, grown) < STILL {
            return true;
        }
        if !self.follow_camera {
            return false;
        }
        let Some((dx, dy, left)) = camera_move(w, h, before, before_mask, frame, grown) else {
            return false;
        };
        if left >= FOLLOWED || dx.abs() > MAX_SHIFT || dy.abs() > MAX_SHIFT {
            return false;
        }
        self.travelled.0 += dx;
        self.travelled.1 += dy;
        let to = (
            self.travelled.0.round() as i32,
            self.travelled.1.round() as i32,
        );
        let (sx, sy) = (to.0 - self.moved.0, to.1 - self.moved.1);
        self.moved = to;
        if (sx, sy) != (0, 0) {
            self.memory = shift(&self.memory, w, h, 4, sx, sy, 0);
            self.age = shift(&self.age, w, h, 1, sx, sy, u16::MAX);
            if let Some((_, before_mask, before_out)) = &mut self.previous {
                // What came in from outside the picture was never filled:
                // marked, so the smoothing leaves it alone.
                *before_mask = shift(before_mask, w, h, 1, sx, sy, 0);
                *before_out = shift(before_out, w, h, 4, sx, sy, 0);
                let mut inside = vec![1u8; w * h];
                inside = shift(&inside, w, h, 1, sx, sy, 0);
                for (m, &i) in before_mask.iter_mut().zip(&inside) {
                    if i == 0 {
                        *m = 0;
                    }
                }
            }
        }
        self.followed += 1;
        true
    }

    /// Blend `filled` (the crop `rect`, RGBA8) into `out` by the soft mask.
    fn paste(&self, out: &mut [u8], filled: &[u8], rect: Rect, soft: &[u8]) {
        let w = self.width;
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let a = soft[y * w + x] as u32;
                if a == 0 {
                    continue;
                }
                let o = (y * w + x) * 4;
                let f = ((y - rect.y0) * rect.width() + (x - rect.x0)) * 4;
                for c in 0..3 {
                    let mixed = out[o + c] as u32 * (255 - a) + filled[f + c] as u32 * a;
                    out[o + c] = ((mixed + 127) / 255) as u8;
                }
            }
        }
    }

    /// Mix the fill with the previous frame's where both frames filled.
    fn smooth(&self, out: &mut [u8], unknown: &[u8]) {
        let Some((_, before_mask, before_out)) = &self.previous else {
            return;
        };
        let keep = (SMOOTH * 256.0) as u32;
        for (i, &m) in unknown.iter().enumerate() {
            if m == 0 || before_mask[i] == 0 {
                continue;
            }
            for c in 0..3 {
                let o = i * 4 + c;
                out[o] = ((out[o] as u32 * (256 - keep) + before_out[o] as u32 * keep) >> 8) as u8;
            }
        }
    }

    /// Remember every pixel outside the mask as background, age the rest.
    fn remember(&mut self, frame: &[u8], grown: &[u8]) {
        for (i, &m) in grown.iter().enumerate() {
            if m == 0 {
                self.memory[i * 4..i * 4 + 4].copy_from_slice(&frame[i * 4..i * 4 + 4]);
                self.age[i] = 0;
            } else {
                self.age[i] = self.age[i].saturating_add(1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_still_shot_fills_from_memory_without_the_model() {
        // A 32×16 grey gradient; the "object" is a white block that moves
        // right by 8 px. In the second frame its old place is seen and its
        // new place was seen in the first frame: memory fills all of it.
        let (w, h) = (32usize, 16usize);
        let background: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = (i % w * 6) as u8;
                [v, v, v, 255]
            })
            .collect();
        let with_object = |x0: usize| {
            let mut f = background.clone();
            let mut m = vec![0u8; w * h];
            for y in 4..12 {
                for x in x0..x0 + 6 {
                    f[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(255);
                    m[y * w + x] = 255;
                }
            }
            (f, m)
        };
        let mut remover = Remover::new(w, h, 0.0);
        // The first frame shows no object: everything is remembered.
        remover
            .next(&background, &vec![0; w * h], None, None)
            .unwrap();
        let (frame, object) = with_object(20);
        let out = remover.next(&frame, &object, None, None).unwrap();
        assert_eq!(remover.model_runs, 0, "no model needed");
        assert_eq!(out, background, "the true background");
    }

    #[test]
    fn the_plate_knows_what_the_object_covered_before_it_moved() {
        // The object sits on the left for the first frames, then leaves:
        // in frame 0 nothing behind it has been seen yet, but the plate
        // of the shot has it.
        let (w, h) = (24usize, 8usize);
        let background: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = (i % w * 10) as u8;
                [v, v, v, 255]
            })
            .collect();
        let frame_with = |x0: usize| {
            let mut f = background.clone();
            let mut m = vec![0u8; w * h];
            for y in 2..6 {
                for x in x0..x0 + 4 {
                    f[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(255);
                    m[y * w + x] = 255;
                }
            }
            (f, m)
        };
        let shots: Vec<(Vec<u8>, Vec<u8>)> =
            [2, 2, 8, 14, 18].iter().map(|&x| frame_with(x)).collect();
        let mut builder = PlateBuilder::new(w, h, 0.0);
        for (n, (f, m)) in shots.iter().enumerate() {
            builder.add(n as i64 * 33_333, f, m);
        }
        let plates = builder.finish();
        assert_eq!(plates.len(), 1, "one still shot");
        let plate = plate_at(&plates, 0).expect("frame 0 is in it");
        let mut remover = Remover::new(w, h, 0.0);
        let out = remover
            .next(&shots[0].0, &shots[0].1, Some(plate), None)
            .unwrap();
        assert_eq!(remover.model_runs, 0, "no model needed");
        assert_eq!(out, background, "the background from the later frames");
        assert!(plate_at(&plates, 999_999).is_none());
    }

    /// A picture with detail everywhere and no repeats: a hash of the
    /// position, smoothed a little so a fractional shift interpolates.
    fn texture(w: usize, h: usize, x0: i32, y0: i32) -> Vec<u8> {
        let at = |x: i32, y: i32| -> f32 {
            let mut v = (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)) as u32;
            v ^= v >> 13;
            v = v.wrapping_mul(0x5bd1_e995);
            v ^= v >> 15;
            (v % 200) as f32 + 28.0
        };
        (0..w * h)
            .flat_map(|i| {
                let (x, y) = ((i % w) as i32 + x0, (i / w) as i32 + y0);
                let v = (at(x, y) * 2.0 + at(x + 1, y) + at(x, y + 1)) / 4.0;
                let v = v as u8;
                [v, v.wrapping_add(17), 255 - v, 255]
            })
            .collect()
    }

    #[test]
    fn the_camera_move_is_found_outside_the_masks() {
        let (w, h) = (160usize, 120usize);
        let before = texture(w, h, 0, 0);
        // The camera pans right and down: the picture moves left and up.
        let now = texture(w, h, 7, 3);
        let mut mask = vec![0u8; w * h];
        for y in 40..80 {
            for x in 60..100 {
                mask[y * w + x] = 255;
            }
        }
        let (dx, dy, left) = camera_move(w, h, &before, &mask, &now, &mask).expect("a move");
        assert!(
            (dx + 7.0).abs() < 0.3 && (dy + 3.0).abs() < 0.3,
            "{dx} {dy}"
        );
        assert!(left < 2.0, "the shift explains it: {left}");
        let still = camera_move(w, h, &before, &mask, &before, &mask).unwrap();
        assert!(still.0.abs() < 0.3 && still.1.abs() < 0.3 && still.2 < 0.5);
    }

    #[test]
    fn a_panning_shot_fills_from_the_moved_memory_without_the_model() {
        // A static "logo" over a picture that pans 3 px a frame: what the
        // logo covers slid in from beside it, so the moved memory knows it.
        let (w, h) = (128usize, 96usize);
        let mut logo = vec![0u8; w * h];
        for y in 30..60 {
            for x in 40..70 {
                logo[y * w + x] = 255;
            }
        }
        let mut remover = Remover::new(w, h, 0.0);
        // The first frame shows no logo yet.
        remover
            .next(&texture(w, h, 0, 0), &vec![0; w * h], None, None)
            .unwrap();
        for n in 1..6 {
            let clean = texture(w, h, 3 * n, 0);
            let mut frame = clean.clone();
            for (i, &m) in logo.iter().enumerate() {
                if m != 0 {
                    frame[i * 4..i * 4 + 3].fill(255);
                }
            }
            let out = remover.next(&frame, &logo, None, None).unwrap();
            assert_eq!(remover.model_runs, 0, "frame {n}: no model needed");
            assert_eq!(out, clean, "frame {n}: the true background");
        }
        assert_eq!(remover.followed, 5);

        // Without following the camera every frame is new to it.
        let mut still_only = Remover::new(w, h, 0.0);
        still_only.follow_camera = false;
        still_only
            .next(&texture(w, h, 0, 0), &vec![0; w * h], None, None)
            .unwrap();
        assert!(!still_only.steady(&texture(w, h, 3, 0), &logo));
    }

    #[test]
    fn shifting_moves_by_whole_pixels_and_fills_the_edge() {
        let values: Vec<u8> = (0..12).collect(); // 4 × 3
        let moved = shift(&values, 4, 3, 1, 1, -1, 99);
        assert_eq!(moved, vec![99, 4, 5, 6, 99, 8, 9, 10, 99, 99, 99, 99]);
        assert_eq!(shift(&values, 4, 3, 1, 0, 0, 99), values);
        assert!(shift(&values, 4, 3, 1, 9, 0, 7).iter().all(|&v| v == 7));
    }

    #[test]
    fn a_moving_camera_drops_the_memory() {
        let (w, h) = (16usize, 8usize);
        let grey = |v: u8| [v, v, v, 255].repeat(w * h);
        let mut remover = Remover::new(w, h, 0.0);
        remover
            .next(&grey(10), &vec![0; w * h], None, None)
            .unwrap();
        assert!(remover.age.iter().all(|&a| a == 0));
        // The whole picture changed by 100: not still. With nothing masked
        // the frame comes back as it is and is remembered afresh.
        let out = remover
            .next(&grey(110), &vec![0; w * h], None, None)
            .unwrap();
        assert_eq!(out, grey(110));
        assert!(remover.change(&grey(10), &[0; 128], &grey(110), &[0; 128]) > STILL);
    }
}
