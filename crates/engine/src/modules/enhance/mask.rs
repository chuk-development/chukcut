//! The mask of "Remove object", in pixels: built from the clip's setting
//! for one frame, grown past the object's edge, and the crop LaMa is run on.
//!
//! A mask is one byte per pixel of the frame, 255 where the picture is to
//! be painted over and 0 elsewhere. Everything here is plain arithmetic on
//! byte buffers; the frames are at most 3840 px on the long side, where a
//! pass over the mask takes a few milliseconds.

use super::{ObjectRemoval, Stroke};

/// A rectangle of pixels, `x0..x1 × y0..y1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Rect {
    pub fn width(&self) -> usize {
        self.x1 - self.x0
    }

    pub fn height(&self) -> usize {
        self.y1 - self.y0
    }
}

/// The strokes and boxes of `removal` drawn into a `w × h` mask: the part
/// of the mask that is the same in every frame.
pub fn painted(removal: &ObjectRemoval, w: usize, h: usize) -> Vec<u8> {
    let mut mask = vec![0u8; w * h];
    let short = w.min(h) as f32;
    for b in &removal.boxes {
        let x0 = (b[0] * w as f32).floor().max(0.0) as usize;
        let y0 = (b[1] * h as f32).floor().max(0.0) as usize;
        let x1 = (((b[0] + b[2]) * w as f32).ceil() as usize).min(w);
        let y1 = (((b[1] + b[3]) * h as f32).ceil() as usize).min(h);
        for y in y0..y1 {
            mask[y * w + x0.min(x1)..y * w + x1].fill(255);
        }
    }
    for stroke in &removal.strokes {
        draw_stroke(&mut mask, w, h, stroke, short);
    }
    mask
}

/// A stroke as round-ended segments: every pixel within `radius` of the
/// polyline.
fn draw_stroke(mask: &mut [u8], w: usize, h: usize, stroke: &Stroke, short: f32) {
    let r = (stroke.radius * short).max(0.5);
    let pts: Vec<(f32, f32)> = stroke
        .points
        .iter()
        .map(|p| (p[0] * w as f32, p[1] * h as f32))
        .collect();
    let segments: Vec<((f32, f32), (f32, f32))> = if pts.len() == 1 {
        vec![(pts[0], pts[0])]
    } else {
        pts.windows(2).map(|s| (s[0], s[1])).collect()
    };
    for (a, b) in segments {
        let x0 = (a.0.min(b.0) - r).floor().max(0.0) as usize;
        let x1 = ((a.0.max(b.0) + r).ceil() as usize).min(w);
        let y0 = (a.1.min(b.1) - r).floor().max(0.0) as usize;
        let y1 = ((a.1.max(b.1) + r).ceil() as usize).min(h);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len2 = dx * dx + dy * dy;
        for y in y0..y1 {
            let py = y as f32 + 0.5;
            for x in x0..x1 {
                let px = x as f32 + 0.5;
                let t = if len2 > 0.0 {
                    (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (cx, cy) = (a.0 + t * dx - px, a.1 + t * dy - py);
                if cx * cx + cy * cy <= r * r {
                    mask[y * w + x] = 255;
                }
            }
        }
    }
}

/// Add a matte (`mw × mh`, the "Select object" matte at its bake size) to
/// a `w × h` mask: every pixel whose bilinear matte value is at least half.
pub fn add_matte(mask: &mut [u8], w: usize, h: usize, matte: &[u8], mw: usize, mh: usize) {
    if mw == 0 || mh == 0 || matte.len() != mw * mh {
        return;
    }
    let (sx, sy) = (mw as f32 / w as f32, mh as f32 / h as f32);
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (mh - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(mh - 1);
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (mw - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(mw - 1);
            let at = |xx: usize, yy: usize| matte[yy * mw + xx] as f32;
            let v = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
                + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
            if v >= 128.0 {
                mask[y * w + x] = 255;
            }
        }
    }
}

/// The mask grown by `radius` pixels in every direction (a square
/// structuring element, two separable passes of a running maximum).
pub fn grow(mask: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    if radius == 0 {
        return mask.to_vec();
    }
    let mut rows = vec![0u8; w * h];
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        let out = &mut rows[y * w..(y + 1) * w];
        dilate_line(row, out, radius);
    }
    let mut out = vec![0u8; w * h];
    let mut column = vec![0u8; h];
    let mut grown = vec![0u8; h];
    for x in 0..w {
        for y in 0..h {
            column[y] = rows[y * w + x];
        }
        dilate_line(&column, &mut grown, radius);
        for y in 0..h {
            out[y * w + x] = grown[y];
        }
    }
    out
}

/// One line of a binary dilation: `out[i]` is set when any of
/// `line[i - r ..= i + r]` is. Linear in the line's length.
fn dilate_line(line: &[u8], out: &mut [u8], r: usize) {
    let n = line.len();
    // The distance to the nearest set pixel, from the left and the right.
    let mut last: Option<usize> = None;
    for i in 0..n {
        if line[i] != 0 {
            last = Some(i);
        }
        out[i] = u8::from(last.is_some_and(|l| i - l <= r)) * 255;
    }
    let mut next: Option<usize> = None;
    for i in (0..n).rev() {
        if line[i] != 0 {
            next = Some(i);
        }
        if next.is_some_and(|nx| nx - i <= r) {
            out[i] = 255;
        }
    }
}

/// The smallest rectangle around every set pixel; `None` for an empty mask.
pub fn bounds(mask: &[u8], w: usize, h: usize) -> Option<Rect> {
    let mut rect: Option<Rect> = None;
    for y in 0..h {
        let row = &mask[y * w..(y + 1) * w];
        let Some(first) = row.iter().position(|&m| m != 0) else {
            continue;
        };
        let last = row.iter().rposition(|&m| m != 0).expect("a set pixel");
        rect = Some(match rect {
            None => Rect {
                x0: first,
                y0: y,
                x1: last + 1,
                y1: y + 1,
            },
            Some(r) => Rect {
                x0: r.x0.min(first),
                y0: r.y0,
                x1: r.x1.max(last + 1),
                y1: y + 1,
            },
        });
    }
    rect
}

/// The crop LaMa sees for a mask with `bounds`: a square around it with as
/// much picture again on every side (LaMa needs context to know what
/// continues behind the object), at least 512 px or the frame's shorter
/// side, and inside the frame. The crop is stretched to 512² in the worker;
/// a small object keeps its detail and a large one gets enough context.
pub fn crop_for(bounds: Rect, w: usize, h: usize) -> Rect {
    let long = bounds.width().max(bounds.height());
    let side = (long * 2)
        .max(512.min(w.min(h)))
        .max(long + 32)
        .min(w.max(h));
    let (sw, sh) = (side.min(w), side.min(h));
    let cx = (bounds.x0 + bounds.x1) / 2;
    let cy = (bounds.y0 + bounds.y1) / 2;
    let x0 = cx.saturating_sub(sw / 2).min(w - sw);
    let y0 = cy.saturating_sub(sh / 2).min(h - sh);
    Rect {
        x0,
        y0,
        x1: x0 + sw,
        y1: y0 + sh,
    }
}

/// Copy `rect` out of a picture of `w` pixels per row with `channels`
/// bytes per pixel.
pub fn crop(src: &[u8], w: usize, channels: usize, rect: Rect) -> Vec<u8> {
    let mut out = Vec::with_capacity(rect.width() * rect.height() * channels);
    for y in rect.y0..rect.y1 {
        out.extend_from_slice(&src[(y * w + rect.x0) * channels..(y * w + rect.x1) * channels]);
    }
    out
}

/// A soft edge for a grown mask: a box blur of `radius` (two separable
/// passes), so the fill fades into the picture over a few pixels instead
/// of ending in a hard line. The growth keeps the object itself under full
/// weight as long as it is at least the blur.
pub fn feather(mask: &[u8], w: usize, h: usize, radius: usize) -> Vec<u8> {
    if radius == 0 {
        return mask.to_vec();
    }
    let blur_line = |line: &[u8], out: &mut [u8]| {
        let n = line.len();
        let mut prefix = vec![0u32; n + 1];
        for i in 0..n {
            prefix[i + 1] = prefix[i] + line[i] as u32;
        }
        for (i, o) in out.iter_mut().enumerate() {
            let a = i.saturating_sub(radius);
            let b = (i + radius + 1).min(n);
            *o = ((prefix[b] - prefix[a]) / (b - a) as u32) as u8;
        }
    };
    let mut rows = vec![0u8; w * h];
    for y in 0..h {
        blur_line(&mask[y * w..(y + 1) * w], &mut rows[y * w..(y + 1) * w]);
    }
    let mut out = vec![0u8; w * h];
    let mut column = vec![0u8; h];
    let mut blurred = vec![0u8; h];
    for x in 0..w {
        for y in 0..h {
            column[y] = rows[y * w + x];
        }
        blur_line(&column, &mut blurred);
        for y in 0..h {
            out[y * w + x] = blurred[y];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_and_strokes_land_where_they_say() {
        let removal = ObjectRemoval {
            boxes: vec![[0.25, 0.5, 0.25, 0.25]],
            strokes: vec![Stroke {
                points: vec![[0.0, 0.1], [1.0, 0.1]],
                radius: 0.05,
            }],
            ..ObjectRemoval::new()
        };
        let (w, h) = (40, 20);
        let m = painted(&removal, w, h);
        assert_eq!(m[12 * w + 12], 255, "inside the box");
        assert_eq!(m[12 * w + 22], 0, "right of the box");
        assert_eq!(m[16 * w + 12], 0, "below the box");
        // The stroke: y = 2 px, radius 1 px of the 20 px short side.
        assert_eq!(m[2 * w + 30], 255);
        assert_eq!(m[5 * w + 30], 0);
    }

    #[test]
    fn growing_adds_the_radius_and_feathering_keeps_the_core() {
        let (w, h) = (20, 20);
        let mut m = vec![0u8; w * h];
        m[10 * w + 10] = 255;
        let g = grow(&m, w, h, 3);
        assert_eq!(g.iter().filter(|&&v| v != 0).count(), 49);
        assert_eq!(g[7 * w + 7], 255);
        assert_eq!(g[6 * w + 10], 0);
        let b = bounds(&g, w, h).unwrap();
        assert_eq!(
            b,
            Rect {
                x0: 7,
                y0: 7,
                x1: 14,
                y1: 14
            }
        );
        let f = feather(&g, w, h, 1);
        assert_eq!(f[10 * w + 10], 255, "the centre stays whole");
        assert!(f[6 * w + 10] > 0 && f[6 * w + 10] < 255);
        assert!(bounds(&[0; 4], 2, 2).is_none());
    }

    #[test]
    fn a_crop_has_context_and_stays_inside_the_frame() {
        let (w, h) = (1920, 1080);
        let small = Rect {
            x0: 1900,
            y0: 10,
            x1: 1910,
            y1: 30,
        };
        let c = crop_for(small, w, h);
        assert_eq!((c.width(), c.height()), (512, 512));
        assert_eq!(c.x1, w, "pushed back inside");
        assert_eq!(c.y0, 0);
        let big = Rect {
            x0: 400,
            y0: 200,
            x1: 1400,
            y1: 900,
        };
        let c = crop_for(big, w, h);
        assert_eq!((c.width(), c.height()), (1920, 1080), "the whole frame");
        let pixels: Vec<u8> = (0..16).collect();
        let r = Rect {
            x0: 1,
            y0: 1,
            x1: 3,
            y1: 2,
        };
        assert_eq!(crop(&pixels, 4, 1, r), vec![5, 6]);
    }

    #[test]
    fn a_matte_counts_where_it_is_at_least_half() {
        let (w, h) = (8, 8);
        let mut mask = vec![0u8; w * h];
        // A 2×2 matte with its left column on.
        add_matte(&mut mask, w, h, &[255, 0, 255, 0], 2, 2);
        assert_eq!(mask[3 * w], 255);
        assert_eq!(mask[3 * w + 7], 0);
        assert_eq!(mask[3 * w + 3], 255, "just left of the middle");
        assert_eq!(mask[3 * w + 4], 0);
    }
}
