//! Real-ESRGAN: a sharper, cleaner picture at four times the size, without
//! a runtime.
//!
//! `realesr-general-x4v3` (Wang et al., "Real-ESRGAN", ICCVW 2021;
//! `github.com/xinntao/Real-ESRGAN`, BSD-3-Clause, weights included) is the
//! project's small general model: an SRVGGNetCompact of 32 convolutions at
//! the input's size and a pixel shuffle, 4.9 MB. It is trained on real-world
//! degradations (blur, noise, JPEG), so on video it also takes out
//! compression blocks and noise, which is most of what "optimise quality"
//! means to a user. It has no state: each frame is made on its own.
//!
//! The ONNX file is `CoderViking/realesr-general-x4v3-onnx` (BSD-3-Clause),
//! a re-export of the official weights: `input` `[1, 3, H, W]`, planar RGB
//! in 0..1, any size; `output` `[1, 3, 4H, 4W]`, about 0..1 and **not**
//! clamped in the graph.
//!
//! **Tiles.** A frame larger than [`TILE`] plus its padding on a side is run
//! in tiles: the side is cut into the fewest equal parts of at most
//! [`TILE`], each read with [`PAD`] pixels of context on both sides (edge
//! pixels repeated beyond the frame), of which only the middle is kept.
//! Every frame of a clip has the same size, so its tiles have one shape and
//! the CUDA provider picks its kernels once; equal parts waste less than
//! fixed-size tiles (a 1080p frame is six tiles, 11 % more pixels than the
//! frame, where 512² tiles read 70 % more); and the padding means a
//! convolution never sees a tile's edge as the picture's edge, so no seams.
//! Memory stays bounded by one tile.
//!
//! **Other scales.** The model makes 4x; 2x (and any capped size) is the 4x
//! picture reduced by area averaging ([`resize_rgba`]), which is what the
//! project's own `--outscale` does and keeps the model's detail.

/// The model's scale.
pub const SCALE: usize = 4;
/// The most picture a tile holds on a side, in input pixels.
pub const TILE: usize = 768;
/// Context around a tile, in input pixels, dropped from the output.
pub const PAD: usize = 16;

/// One stretch of an axis: the part a tile owns and the part it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// First pixel the tile's output is kept for.
    pub start: usize,
    /// How many pixels it is kept for.
    pub len: usize,
    /// First pixel the tile reads (may lie before the frame: repeated).
    pub read_start: isize,
    /// How many pixels it reads (the tensor's size on this axis).
    pub read_len: usize,
}

/// How an axis of `n` pixels is cut: one span reading exactly the frame
/// when it fits in a tile with its padding, else the fewest equal parts of
/// at most [`TILE`], each read with [`PAD`] on both sides, all the same
/// size.
pub fn spans(n: usize) -> Vec<Span> {
    if n <= TILE + 2 * PAD {
        return vec![Span {
            start: 0,
            len: n,
            read_start: 0,
            read_len: n,
        }];
    }
    let parts = n.div_ceil(TILE);
    let part = n.div_ceil(parts);
    (0..parts)
        .map(|k| {
            let start = k * part;
            Span {
                start,
                len: part.min(n - start),
                read_start: start as isize - PAD as isize,
                read_len: part + 2 * PAD,
            }
        })
        .collect()
}

/// The `input` tensor of one tile: planar RGB in 0..1, `ys.read_len ×
/// xs.read_len`, pixels beyond the frame repeated from its edge.
pub fn tile_input(rgba: &[u8], width: usize, height: usize, xs: Span, ys: Span) -> Vec<f32> {
    let (tw, th) = (xs.read_len, ys.read_len);
    let plane = tw * th;
    let mut out = vec![0.0f32; 3 * plane];
    for ty in 0..th {
        let sy = (ys.read_start + ty as isize).clamp(0, height as isize - 1) as usize;
        for tx in 0..tw {
            let sx = (xs.read_start + tx as isize).clamp(0, width as isize - 1) as usize;
            let px = &rgba[(sy * width + sx) * 4..(sy * width + sx) * 4 + 3];
            for c in 0..3 {
                out[c * plane + ty * tw + tx] = px[c] as f32 / 255.0;
            }
        }
    }
    out
}

/// Copy the kept middle of one tile's `output` (`[1, 3, 4·read_h,
/// 4·read_w]`) into `big`, the RGBA8 picture at four times `width`.
pub fn place_tile(big: &mut [u8], width: usize, output: &[f32], xs: Span, ys: Span) {
    let (ow, oh) = (xs.read_len * SCALE, ys.read_len * SCALE);
    let plane = ow * oh;
    let big_w = width * SCALE;
    let skip_x = (xs.start as isize - xs.read_start) as usize * SCALE;
    let skip_y = (ys.start as isize - ys.read_start) as usize * SCALE;
    let (r, g, b) = (
        &output[..plane],
        &output[plane..2 * plane],
        &output[2 * plane..3 * plane],
    );
    let row_bytes = big_w * 4;
    let rows = &mut big[ys.start * SCALE * row_bytes..(ys.start + ys.len) * SCALE * row_bytes];
    // 33 M pixels at 1080p: rows split over threads, and `unit_byte`
    // instead of a libm `round` per value (`pixels`).
    crate::pixels::par_rows(rows, row_bytes, |first, chunk| {
        for (k, row) in chunk.chunks_exact_mut(row_bytes).enumerate() {
            let oy = skip_y + first + k;
            let src = oy * ow + skip_x;
            let dst = &mut row[xs.start * SCALE * 4..(xs.start + xs.len) * SCALE * 4];
            for (x, px) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let i = src + x;
                *px = [
                    crate::pixels::unit_byte(r[i]),
                    crate::pixels::unit_byte(g[i]),
                    crate::pixels::unit_byte(b[i]),
                    255,
                ];
            }
        }
    });
}

/// Resize an RGBA8 picture: by area averaging where it shrinks (every
/// source pixel counts by how much of it an output pixel covers, the right
/// filter for a 4x picture reduced to 2x), bilinear where it grows.
pub fn resize_rgba(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    if (sw, sh) == (dw, dh) {
        return src.to_vec();
    }
    // A whole-number reduction (4x to 2x, the common case) is a plain
    // box average, a quarter of the time of the general filter.
    if dw > 0
        && dh > 0
        && sw.is_multiple_of(dw)
        && sh.is_multiple_of(dh)
        && sw / dw == sh / dh
        && sw > dw
    {
        return box_reduce(src, sw, dw, dh, sw / dw);
    }
    // Horizontal pass into floats, then vertical.
    let horizontal = resample_axis(src, sw, sh, dw);
    let vertical = resample_axis_f32(&horizontal, dw, sh, dh);
    vertical
        .iter()
        .map(|v| v.clamp(0.0, 255.0).round() as u8)
        .collect()
}

/// `src` (`sw` pixels a row) reduced by `k` on both sides to `dw × dh` by
/// averaging each `k × k` block, rounded.
fn box_reduce(src: &[u8], sw: usize, dw: usize, dh: usize, k: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    let area = (k * k) as u32;
    let mut acc = vec![0u32; dw * 4];
    for y in 0..dh {
        acc.fill(0);
        for row in src.chunks_exact(sw * 4).skip(y * k).take(k) {
            for (x, a) in acc.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                for px in row[x * k * 4..(x + 1) * k * 4].as_chunks::<4>().0 {
                    for c in 0..4 {
                        a[c] += px[c] as u32;
                    }
                }
            }
        }
        for (o, a) in out[y * dw * 4..(y + 1) * dw * 4].iter_mut().zip(&acc) {
            *o = ((a + area / 2) / area) as u8;
        }
    }
    out
}

/// The weights of each output pixel over the input pixels of one axis.
fn weights(n: usize, m: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = n as f32 / m as f32;
    (0..m)
        .map(|o| {
            if scale > 1.0 {
                // Area: the input interval [o·s, (o+1)·s).
                let (a, b) = (o as f32 * scale, (o + 1) as f32 * scale);
                let mut taps = Vec::new();
                let mut i = a.floor() as usize;
                while (i as f32) < b && i < n {
                    let cover = (b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0);
                    if cover > 0.0 {
                        taps.push((i, cover / scale));
                    }
                    i += 1;
                }
                taps
            } else {
                let f = ((o as f32 + 0.5) * scale - 0.5).clamp(0.0, (n - 1) as f32);
                let i0 = f as usize;
                let i1 = (i0 + 1).min(n - 1);
                let t = f - i0 as f32;
                vec![(i0, 1.0 - t), (i1, t)]
            }
        })
        .collect()
}

fn resample_axis(src: &[u8], sw: usize, sh: usize, dw: usize) -> Vec<f32> {
    let taps = weights(sw, dw);
    let mut out = vec![0.0f32; dw * sh * 4];
    for y in 0..sh {
        let row = &src[y * sw * 4..(y + 1) * sw * 4];
        for (x, taps) in taps.iter().enumerate() {
            let mut acc = [0.0f32; 4];
            for &(i, w) in taps {
                for c in 0..4 {
                    acc[c] += row[i * 4 + c] as f32 * w;
                }
            }
            out[(y * dw + x) * 4..(y * dw + x) * 4 + 4].copy_from_slice(&acc);
        }
    }
    out
}

fn resample_axis_f32(src: &[f32], w: usize, sh: usize, dh: usize) -> Vec<f32> {
    let taps = weights(sh, dh);
    let mut out = vec![0.0f32; w * dh * 4];
    for (y, taps) in taps.iter().enumerate() {
        let dst = &mut out[y * w * 4..(y + 1) * w * 4];
        for &(i, wt) in taps {
            let row = &src[i * w * 4..(i + 1) * w * 4];
            for (d, s) in dst.iter_mut().zip(row) {
                *d += s * wt;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_axis_is_one_span_and_a_large_one_tiles_of_one_size() {
        assert_eq!(
            spans(300),
            vec![Span {
                start: 0,
                len: 300,
                read_start: 0,
                read_len: 300
            }]
        );
        let s = spans(1920);
        assert_eq!(s.len(), 3);
        assert!(s.iter().all(|s| s.read_len == 640 + 2 * PAD));
        assert_eq!(s[0].read_start, -(PAD as isize));
        assert_eq!(s[2].start, 1280);
        assert_eq!(s.iter().map(|s| s.len).sum::<usize>(), 1920);
        let odd = spans(1081);
        assert_eq!(odd.len(), 2);
        assert_eq!(
            (odd[0].len, odd[1].len),
            (541, 540),
            "the last part is short"
        );
        assert!(odd.iter().all(|s| s.read_len == 541 + 2 * PAD));
    }

    #[test]
    fn tiles_put_back_together_are_the_picture_without_seams() {
        // An "identity" model: the output is the input repeated 4x4 (what
        // nearest upsampling is). The tiled result must equal the whole.
        let (w, h) = (1700usize, 70usize);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 251) as u8, (i % 7 * 30) as u8, (i / w) as u8, 255])
            .collect();
        let mut big = vec![0u8; w * SCALE * h * SCALE * 4];
        for ys in spans(h) {
            for xs in spans(w) {
                let input = tile_input(&rgba, w, h, xs, ys);
                let (tw, th) = (xs.read_len, ys.read_len);
                let (ow, oh) = (tw * SCALE, th * SCALE);
                let mut output = vec![0.0f32; 3 * ow * oh];
                for c in 0..3 {
                    for y in 0..oh {
                        for x in 0..ow {
                            output[c * ow * oh + y * ow + x] =
                                input[c * tw * th + (y / SCALE) * tw + x / SCALE];
                        }
                    }
                }
                place_tile(&mut big, w, &output, xs, ys);
            }
        }
        for (y, x) in [(0, 0), (69, 1699), (35, 566), (35, 567), (12, 1134)] {
            for (dy, dx) in [(0, 0), (3, 3)] {
                let b = ((y * SCALE + dy) * w * SCALE + x * SCALE + dx) * 4;
                let s = (y * w + x) * 4;
                assert_eq!(&big[b..b + 4], &rgba[s..s + 4], "at {x},{y}");
            }
        }
    }

    #[test]
    fn halving_averages_and_growing_interpolates() {
        // 4×2 → 2×1: each output pixel is the mean of a 2×2 block.
        let src: Vec<u8> = [0u8, 100, 200, 40, 20, 120, 220, 60]
            .iter()
            .flat_map(|&v| [v, v, v, 255])
            .collect();
        let half = resize_rgba(&src, 4, 2, 2, 1);
        assert_eq!(half[0], 60, "(0 + 100 + 20 + 120) / 4");
        assert_eq!(half[4], 130, "(200 + 40 + 220 + 60) / 4");
        assert_eq!(half[3], 255);
        // The box path and the general filter agree on a whole-number
        // reduction.
        let wide: Vec<u8> = (0..8 * 4)
            .flat_map(|i| [(i * 7 % 256) as u8, 9, 200, 255])
            .collect();
        let boxed = resize_rgba(&wide, 8, 4, 4, 2);
        let general = {
            let h = resample_axis(&wide, 8, 4, 4);
            resample_axis_f32(&h, 4, 4, 2)
                .iter()
                .map(|v| v.clamp(0.0, 255.0).round() as u8)
                .collect::<Vec<u8>>()
        };
        assert!(boxed.iter().zip(&general).all(|(a, b)| a.abs_diff(*b) <= 1));
        let same = resize_rgba(&src, 4, 2, 4, 2);
        assert_eq!(same, src);
        let grown = resize_rgba(&[0, 0, 0, 255, 255, 255, 255, 255], 2, 1, 4, 1);
        assert_eq!(grown.len(), 16);
        assert_eq!(grown[0], 0);
        assert_eq!(grown[12], 255);
        assert!(grown[4] > 0 && grown[4] < 128, "{}", grown[4]);
    }
}
