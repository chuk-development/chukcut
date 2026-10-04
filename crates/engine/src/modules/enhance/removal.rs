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
//! When the camera moves, all of this is dropped and the frame is filled on
//! its own: a remembered pixel would be from another place in the scene,
//! and a mixed fill would smear.
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
        let still = self
            .previous
            .as_ref()
            .is_some_and(|(before, before_mask, _)| {
                self.change(before, before_mask, frame, &grown) < STILL
            });
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
