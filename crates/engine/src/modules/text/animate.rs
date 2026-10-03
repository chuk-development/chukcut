//! Per-glyph animation: the reusable layer under every text animator.
//!
//! Two pieces, both independent of *how* a glyph moves:
//!
//! - [`glyph_units`] groups the glyphs of a laid-out title into letters,
//!   words or lines, in reading order. A text animator staggers over these;
//!   a karaoke highlight can use the same grouping to find a word's glyphs.
//! - [`compose`] redraws a rasterised title with every glyph moved, scaled,
//!   turned and faded on its own ([`GlyphPose`]).
//!
//! ## Why glyphs are moved as sprites, not re-rasterised
//!
//! A title is rasterised once and cached (`cache.rs`); a sprite move per
//! frame costs a few thousand pixel copies, while re-rasterising every glyph
//! with its own transform costs the full five milliseconds every frame and
//! would duplicate the outline, stroke and shadow code. So `compose` cuts each
//! glyph out of the cached image — its ink rectangle grown by the paint that
//! spills past it (stroke, shadow) — and pastes it back transformed.
//!
//! Neighbouring rectangles overlap (kerning, italics, a wide shadow), so
//! every pixel is given to exactly one glyph first: the one whose centre is
//! nearest along the line. Without that, a pixel two glyphs claim would be
//! drawn twice and the seam would show as a darker line.
//!
//! The background box is not a glyph and must not fly apart with the letters,
//! so the caller rasterises it separately (the same title with no text paint)
//! and passes it as the backdrop, faded as one piece.

use super::layout::TextLayout;
use super::raster::RasteredText;
use crate::modules::project::animation::TextUnit;

/// How one glyph is drawn relative to where the layout put it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphPose {
    /// Offset in image pixels, y down.
    pub dx: f32,
    pub dy: f32,
    /// Uniform scale about the glyph's centre.
    pub scale: f32,
    /// Degrees clockwise about the glyph's centre.
    pub rotation: f32,
    pub opacity: f32,
}

impl GlyphPose {
    pub const REST: GlyphPose = GlyphPose {
        dx: 0.0,
        dy: 0.0,
        scale: 1.0,
        rotation: 0.0,
        opacity: 1.0,
    };

    /// `self` followed by `other`.
    pub fn then(self, other: GlyphPose) -> GlyphPose {
        GlyphPose {
            dx: self.dx + other.dx,
            dy: self.dy + other.dy,
            scale: self.scale * other.scale,
            rotation: self.rotation + other.rotation,
            opacity: self.opacity * other.opacity,
        }
    }

    fn moves(&self) -> bool {
        self.dx != 0.0 || self.dy != 0.0 || self.scale != 1.0 || self.rotation != 0.0
    }
}

impl Default for GlyphPose {
    fn default() -> Self {
        GlyphPose::REST
    }
}

/// The unit each glyph of `layout` belongs to, and how many units there are.
///
/// Units are numbered in **reading order** — by the glyph's source position,
/// not its place on screen — so "forward" means the same thing in Arabic as
/// in English. Glyphs of whitespace belong to no unit (`None`): a space has
/// nothing to animate and must not take a turn in the stagger. Every glyph
/// of one cluster (a ligature, a base with its marks) shares a unit.
pub fn glyph_units(text: &str, layout: &TextLayout, unit: TextUnit) -> (Vec<Option<usize>>, usize) {
    let blank = |range: &std::ops::Range<usize>| {
        text.get(range.clone())
            .is_none_or(|s| s.chars().all(char::is_whitespace))
    };

    // The key each glyph is grouped by, in an order that sorts as reading.
    let keys: Vec<Option<(usize, usize)>> = layout
        .glyphs
        .iter()
        .map(|g| {
            if blank(&g.cluster) {
                return None;
            }
            Some(match unit {
                TextUnit::Letter => (g.line, g.cluster.start),
                TextUnit::Word => (0, word_of(text, g.cluster.start)),
                TextUnit::Line => (g.line, 0),
            })
        })
        .collect();

    let mut distinct: Vec<(usize, usize)> = keys.iter().flatten().copied().collect();
    distinct.sort_unstable();
    distinct.dedup();
    let units = keys
        .iter()
        .map(|k| k.and_then(|k| distinct.binary_search(&k).ok()))
        .collect();
    (units, distinct.len())
}

/// Which whitespace-separated word of `text` the byte `at` is in.
fn word_of(text: &str, at: usize) -> usize {
    let mut word = 0;
    let mut in_word = false;
    for (i, c) in text.char_indices() {
        if i >= at {
            break;
        }
        if c.is_whitespace() {
            if in_word {
                word += 1;
            }
            in_word = false;
        } else {
            in_word = true;
        }
    }
    // `at` itself starts a new word if the text before it ended in a space.
    word
}

/// Redraw `glyphs` with each glyph posed, over `backdrop` faded to
/// `backdrop_opacity`.
///
/// `poses[i]` is for `glyphs.layout.glyphs[i]`; a glyph with no pose (the
/// slice is short, or the unit is `None`) is drawn at rest. `pad` is how far
/// paint spills past a glyph's ink rectangle — the stroke is already in the
/// rectangle, so this is the shadow and a pixel of antialiasing. The result is
/// straight-alpha RGBA8 the size of `glyphs`, like every other text image.
pub fn compose(
    glyphs: &RasteredText,
    backdrop: Option<&RasteredText>,
    backdrop_opacity: f32,
    poses: &[GlyphPose],
    pad: f32,
    units: Option<&[Option<usize>]>,
) -> Vec<u8> {
    let (w, h) = (glyphs.width as usize, glyphs.height as usize);
    // Premultiplied while compositing; straight on the way out.
    let mut out = vec![0f32; w * h * 4];

    if let Some(back) = backdrop.filter(|b| b.width == glyphs.width && b.height == glyphs.height) {
        let k = backdrop_opacity.clamp(0.0, 1.0);
        if k > 0.0 {
            for (dst, src) in out
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(back.pixels.as_chunks::<4>().0)
            {
                let a = src[3] as f32 / 255.0 * k;
                dst[0] = src[0] as f32 / 255.0 * a;
                dst[1] = src[1] as f32 / 255.0 * a;
                dst[2] = src[2] as f32 / 255.0 * a;
                dst[3] = a;
            }
        }
    }

    let owners = Ownership::new(glyphs, pad);
    let pivots = pivots(glyphs, units);
    for (i, rect) in owners.rects.iter().enumerate() {
        let Some(rect) = rect else {
            continue;
        };
        let pose = poses.get(i).copied().unwrap_or(GlyphPose::REST);
        let opacity = pose.opacity.clamp(0.0, 1.0);
        if opacity <= 0.0 || pose.scale.abs() < 1e-4 || !pose.scale.is_finite() {
            continue;
        }
        // Each glyph turns and scales about its unit's centre, so a word
        // pops as a word rather than as letters drifting apart.
        let centre = pivots[i].unwrap_or((
            (rect[0] + rect[2]) as f32 * 0.5,
            (rect[1] + rect[3]) as f32 * 0.5,
        ));
        if !pose.moves() {
            // At rest: a straight copy of the pixels this glyph owns.
            for y in rect[1]..rect[3] {
                for x in rect[0]..rect[2] {
                    if owners.owner(x, y) == Some(i) {
                        over(&mut out, w, x, y, texel(glyphs, x, y), opacity);
                    }
                }
            }
            continue;
        }

        // Forward-map the rectangle's corners for the destination bounds,
        // then inverse-map every destination pixel back into the glyph.
        let (sin, cos) = pose.rotation.to_radians().sin_cos();
        let forward = |x: f32, y: f32| {
            let (lx, ly) = ((x - centre.0) * pose.scale, (y - centre.1) * pose.scale);
            (
                centre.0 + pose.dx + lx * cos - ly * sin,
                centre.1 + pose.dy + lx * sin + ly * cos,
            )
        };
        let corners = [
            forward(rect[0] as f32, rect[1] as f32),
            forward(rect[2] as f32, rect[1] as f32),
            forward(rect[0] as f32, rect[3] as f32),
            forward(rect[2] as f32, rect[3] as f32),
        ];
        let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min).floor();
        let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).ceil();
        let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min).floor();
        let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).ceil();
        let x0 = (min_x.max(0.0) as usize).min(w);
        let x1 = (max_x.max(0.0) as usize).min(w);
        let y0 = (min_y.max(0.0) as usize).min(h);
        let y1 = (max_y.max(0.0) as usize).min(h);
        let inv = 1.0 / pose.scale;
        for y in y0..y1 {
            for x in x0..x1 {
                // Pixel centres, so a pose at rest samples exactly on texels.
                let (px, py) = (
                    x as f32 + 0.5 - centre.0 - pose.dx,
                    y as f32 + 0.5 - centre.1 - pose.dy,
                );
                let sx = (px * cos + py * sin) * inv + centre.0 - 0.5;
                let sy = (-px * sin + py * cos) * inv + centre.1 - 0.5;
                if let Some(c) = sample(glyphs, &owners, i, sx, sy) {
                    over(&mut out, w, x, y, c, opacity);
                }
            }
        }
    }

    let mut bytes = vec![0u8; w * h * 4];
    for (dst, src) in bytes
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(out.as_chunks::<4>().0)
    {
        let a = src[3];
        if a <= 1e-5 {
            continue;
        }
        dst[0] = ((src[0] / a).clamp(0.0, 1.0) * 255.0).round() as u8;
        dst[1] = ((src[1] / a).clamp(0.0, 1.0) * 255.0).round() as u8;
        dst[2] = ((src[2] / a).clamp(0.0, 1.0) * 255.0).round() as u8;
        dst[3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    bytes
}

/// The point each glyph is posed about: the centre of the ink of every glyph
/// in its unit. `None` (no grouping, or a glyph in no unit) poses a glyph
/// about its own centre.
fn pivots(image: &RasteredText, units: Option<&[Option<usize>]>) -> Vec<Option<(f32, f32)>> {
    let n = image.glyph_rects.len();
    let Some(units) = units else {
        return vec![None; n];
    };
    let mut boxes: std::collections::HashMap<usize, [f32; 4]> = Default::default();
    for (i, r) in image.glyph_rects.iter().enumerate() {
        let Some(Some(u)) = units.get(i) else {
            continue;
        };
        if r[2] <= r[0] || r[3] <= r[1] {
            continue;
        }
        boxes
            .entry(*u)
            .and_modify(|b| {
                *b = [
                    b[0].min(r[0]),
                    b[1].min(r[1]),
                    b[2].max(r[2]),
                    b[3].max(r[3]),
                ]
            })
            .or_insert(*r);
    }
    (0..n)
        .map(|i| {
            let u = units.get(i).copied().flatten()?;
            let b = boxes.get(&u)?;
            Some(((b[0] + b[2]) * 0.5, (b[1] + b[3]) * 0.5))
        })
        .collect()
}

/// A premultiplied texel of `image`.
fn texel(image: &RasteredText, x: usize, y: usize) -> [f32; 4] {
    let i = (y * image.width as usize + x) * 4;
    let p = &image.pixels[i..i + 4];
    let a = p[3] as f32 / 255.0;
    [
        p[0] as f32 / 255.0 * a,
        p[1] as f32 / 255.0 * a,
        p[2] as f32 / 255.0 * a,
        a,
    ]
}

/// Bilinear, premultiplied, counting only texels `glyph` owns.
fn sample(
    image: &RasteredText,
    owners: &Ownership,
    glyph: usize,
    x: f32,
    y: f32,
) -> Option<[f32; 4]> {
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let mut acc = [0f32; 4];
    let mut any = false;
    for (ox, oy, weight) in [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (1, 0, tx * (1.0 - ty)),
        (0, 1, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ] {
        let (sx, sy) = (fx as i64 + ox, fy as i64 + oy);
        if sx < 0 || sy < 0 || sx >= image.width as i64 || sy >= image.height as i64 {
            continue;
        }
        let (sx, sy) = (sx as usize, sy as usize);
        if weight <= 0.0 || owners.owner(sx, sy) != Some(glyph) {
            continue;
        }
        let t = texel(image, sx, sy);
        for c in 0..4 {
            acc[c] += t[c] * weight;
        }
        any = true;
    }
    (any && acc[3] > 0.0).then_some(acc)
}

/// Source-over of premultiplied `c` scaled by `opacity` onto pixel `(x, y)`.
fn over(out: &mut [f32], width: usize, x: usize, y: usize, c: [f32; 4], opacity: f32) {
    let i = (y * width + x) * 4;
    let a = c[3] * opacity;
    if a <= 0.0 {
        return;
    }
    let keep = 1.0 - a;
    out[i] = c[0] * opacity + out[i] * keep;
    out[i + 1] = c[1] * opacity + out[i + 1] * keep;
    out[i + 2] = c[2] * opacity + out[i + 2] * keep;
    out[i + 3] = a + out[i + 3] * keep;
}

/// Which glyph owns each pixel near the text.
struct Ownership {
    /// Each glyph's grown rectangle in whole pixels, `[x0, y0, x1, y1)`;
    /// `None` for a glyph with no ink.
    rects: Vec<Option<[usize; 4]>>,
    /// The bounding box `map` covers.
    origin: (usize, usize),
    stride: usize,
    rows: usize,
    /// Owning glyph + 1, 0 for none.
    map: Vec<u32>,
}

impl Ownership {
    fn new(image: &RasteredText, pad: f32) -> Self {
        let (w, h) = (image.width as f32, image.height as f32);
        let rects: Vec<Option<[usize; 4]>> = image
            .glyph_rects
            .iter()
            .map(|r| {
                if r[2] <= r[0] || r[3] <= r[1] {
                    return None;
                }
                let x0 = (r[0] - pad).floor().clamp(0.0, w) as usize;
                let y0 = (r[1] - pad).floor().clamp(0.0, h) as usize;
                let x1 = (r[2] + pad).ceil().clamp(0.0, w) as usize;
                let y1 = (r[3] + pad).ceil().clamp(0.0, h) as usize;
                (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
            })
            .collect();

        let bounds = rects
            .iter()
            .flatten()
            .fold(None, |acc: Option<[usize; 4]>, r| {
                Some(match acc {
                    None => *r,
                    Some(a) => [
                        a[0].min(r[0]),
                        a[1].min(r[1]),
                        a[2].max(r[2]),
                        a[3].max(r[3]),
                    ],
                })
            });
        let Some(bounds) = bounds else {
            return Self {
                rects,
                origin: (0, 0),
                stride: 0,
                rows: 0,
                map: Vec::new(),
            };
        };
        let stride = bounds[2] - bounds[0];
        let rows = bounds[3] - bounds[1];
        let mut map = vec![0u32; stride * rows];
        let mut best = vec![f32::MAX; stride * rows];
        for (i, rect) in rects.iter().enumerate() {
            let Some(r) = rect else {
                continue;
            };
            let ink = image.glyph_rects[i];
            let (cx, cy) = ((ink[0] + ink[2]) * 0.5, (ink[1] + ink[3]) * 0.5);
            for y in r[1]..r[3] {
                for x in r[0]..r[2] {
                    // Distance along the line first; across it only breaks
                    // ties between lines, which barely overlap.
                    let d = (x as f32 + 0.5 - cx).abs() + 0.25 * (y as f32 + 0.5 - cy).abs();
                    let k = (y - bounds[1]) * stride + (x - bounds[0]);
                    if d < best[k] {
                        best[k] = d;
                        map[k] = i as u32 + 1;
                    }
                }
            }
        }
        Self {
            rects,
            origin: (bounds[0], bounds[1]),
            stride,
            rows,
            map,
        }
    }

    fn owner(&self, x: usize, y: usize) -> Option<usize> {
        if x < self.origin.0 || y < self.origin.1 {
            return None;
        }
        let (lx, ly) = (x - self.origin.0, y - self.origin.1);
        if lx >= self.stride || ly >= self.rows {
            return None;
        }
        match self.map[ly * self.stride + lx] {
            0 => None,
            n => Some(n as usize - 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::text::{RasterOptions, TextRenderer, TextRequest};

    fn rastered(content: &str) -> std::sync::Arc<RasteredText> {
        let request = TextRequest {
            content: content.into(),
            font_size: 64.0,
            ..TextRequest::default()
        };
        TextRenderer::shared().rasterize(&request, &RasterOptions::canvas(800, 400))
    }

    fn ink(pixels: &[u8]) -> u64 {
        pixels.chunks_exact(4).map(|p| p[3] as u64).sum()
    }

    #[test]
    fn units_follow_letters_words_and_lines() {
        let text = rastered("ab cd\nef");
        if text.layout.glyphs.is_empty() {
            return; // no fonts on this machine
        }
        let content = "ab cd\nef";
        let (letters, n) = glyph_units(content, &text.layout, TextUnit::Letter);
        assert_eq!(n, 6);
        assert_eq!(letters.iter().flatten().count(), 6);
        let (words, n) = glyph_units(content, &text.layout, TextUnit::Word);
        assert_eq!(n, 3);
        let firsts: Vec<_> = words.iter().flatten().copied().collect();
        assert_eq!(firsts, vec![0, 0, 1, 1, 2, 2]);
        let (lines, n) = glyph_units(content, &text.layout, TextUnit::Line);
        assert_eq!(n, 2);
        assert_eq!(lines.iter().flatten().filter(|&&u| u == 0).count(), 4);
    }

    #[test]
    fn at_rest_the_composite_is_the_original_image() {
        let text = rastered("Hello");
        if text.layout.glyphs.is_empty() {
            return;
        }
        let poses = vec![GlyphPose::REST; text.layout.glyphs.len()];
        let out = compose(&text, None, 1.0, &poses, 2.0, None);
        // Compared premultiplied: a nearly transparent edge pixel's straight
        // colour is a rounding of a rounding and may differ wildly while
        // contributing nothing to the picture.
        let worst = out
            .chunks_exact(4)
            .zip(text.pixels.chunks_exact(4))
            .map(|(a, b)| {
                (0..4)
                    .map(|c| {
                        let pa = if c == 3 {
                            a[3] as i32
                        } else {
                            a[c] as i32 * a[3] as i32 / 255
                        };
                        let pb = if c == 3 {
                            b[3] as i32
                        } else {
                            b[c] as i32 * b[3] as i32 / 255
                        };
                        (pa - pb).abs()
                    })
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        assert!(worst <= 2, "differs by up to {worst}");
        assert!(ink(&out) > 0);
    }

    #[test]
    fn a_hidden_glyph_takes_its_ink_away_and_a_moved_one_keeps_it() {
        let text = rastered("HI");
        if text.layout.glyphs.len() < 2 {
            return;
        }
        let full = ink(&text.pixels);
        let mut poses = vec![GlyphPose::REST; 2];
        poses[0].opacity = 0.0;
        let half = ink(&compose(&text, None, 1.0, &poses, 2.0, None));
        assert!(half < full * 3 / 4 && half > full / 4, "{half} of {full}");

        let mut moved = vec![GlyphPose::REST; 2];
        moved[1].dy = 40.0;
        let out = compose(&text, None, 1.0, &moved, 2.0, None);
        let kept = ink(&out);
        assert!((kept as f64 - full as f64).abs() < full as f64 * 0.05);
        assert_ne!(out, text.pixels);
    }

    #[test]
    fn a_word_is_posed_about_its_own_centre() {
        let text = rastered("ab cd");
        if text.layout.glyphs.len() < 4 {
            return;
        }
        let (units, _) = glyph_units("ab cd", &text.layout, TextUnit::Word);
        let p = pivots(&text, Some(&units));
        let firsts: Vec<_> = p.iter().flatten().collect();
        assert_eq!(firsts[0], firsts[1], "both letters of a word share a pivot");
        assert_ne!(firsts[1], firsts[2], "the next word has its own");
        assert!(pivots(&text, None).iter().all(Option::is_none));
    }

    #[test]
    fn the_word_of_a_byte_counts_whitespace_runs() {
        assert_eq!(word_of("one  two three", 0), 0);
        assert_eq!(word_of("one  two three", 5), 1);
        assert_eq!(word_of("one  two three", 9), 2);
        assert_eq!(word_of("  lead", 2), 0);
    }
}
