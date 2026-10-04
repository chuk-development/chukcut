//! Moving pictures between RGBA8 bytes and a network's planar floats, fast.
//!
//! At 1080p these conversions were a large share of a frame: RIFE spent
//! ~50 ms of its 178 ms per frame turning two frames into planes and the
//! answer back into bytes, and Real-ESRGAN ~560 ms of 1.55 s writing its
//! 4x picture (33 M pixels) into RGBA (RTX 3060, release build; `docs/STATUS.md`,
//! "Faster AI on NVIDIA"). Two things made them slow: `f32::round` is a
//! libm call per value on baseline x86-64 (no SSE4.1 `roundss`), and one
//! thread did all of it. [`unit_byte`] rounds without the call and the
//! loops here split rows over [`THREADS`] scoped threads.

/// Threads for a conversion: the same budget as a session's intra-op
/// threads, because the editor decodes and renders beside the worker.
pub const THREADS: usize = 4;

/// Below this many values a conversion runs on the calling thread: the
/// threads would cost more than they save.
const PARALLEL_FROM: usize = 1 << 16;

/// A value in 0..1 as a byte: clamped, scaled by 255 and rounded half away
/// from zero, exactly as `(v.clamp(0.0, 1.0) * 255.0).round() as u8`, but
/// without the libm call. `x - trunc(x)` is exact for `x` below 2²³, so the
/// comparison with 0.5 decides the same way `round` does. NaN gives 0.
#[inline]
pub fn unit_byte(v: f32) -> u8 {
    let x = v.clamp(0.0, 1.0) * 255.0;
    let t = x as u32;
    (t + u32::from(x - t as f32 >= 0.5)) as u8
}

/// Run `f(first_row, rows)` over `data` cut into whole rows of `row_len`
/// elements, on up to [`THREADS`] threads.
pub fn par_rows<T: Send>(data: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len().checked_div(row_len).unwrap_or(0);
    if rows == 0 {
        return;
    }
    if data.len() < PARALLEL_FROM || rows < 2 {
        f(0, data);
        return;
    }
    let per = rows.div_ceil(THREADS.min(rows));
    std::thread::scope(|scope| {
        for (k, chunk) in data.chunks_mut(per * row_len).enumerate() {
            let f = &f;
            scope.spawn(move || f(k * per, chunk));
        }
    });
}

/// An RGBA8 picture as three planes of `(byte / 255 - mean) / std`, into
/// `out` (`3 × width × height` values, R then G then B). Alpha is ignored.
pub fn rgba_to_planes(rgba: &[u8], width: usize, height: usize, out: &mut [f32]) {
    rgba_to_planes_normalised(rgba, width, height, [0.0; 3], [1.0; 3], out);
}

/// [`rgba_to_planes`] with a per-channel mean and deviation, in 0..1 units.
pub fn rgba_to_planes_normalised(
    rgba: &[u8],
    width: usize,
    height: usize,
    mean: [f32; 3],
    std: [f32; 3],
    out: &mut [f32],
) {
    let plane = width * height;
    assert!(rgba.len() >= plane * 4 && out.len() == 3 * plane);
    // One table per channel: 256 lookups instead of a divide per value.
    let table: [[f32; 256]; 3] =
        std::array::from_fn(|c| std::array::from_fn(|v| (v as f32 / 255.0 - mean[c]) / std[c]));
    let (r, rest) = out.split_at_mut(plane);
    let (g, b) = rest.split_at_mut(plane);
    let rows = height;
    let work = |first: usize, r: &mut [f32], g: &mut [f32], b: &mut [f32]| {
        let start = first * width;
        let src = &rgba[start * 4..(start + r.len()) * 4];
        for (i, px) in src.as_chunks::<4>().0.iter().enumerate() {
            r[i] = table[0][px[0] as usize];
            g[i] = table[1][px[1] as usize];
            b[i] = table[2][px[2] as usize];
        }
    };
    if plane < PARALLEL_FROM || rows < 2 {
        work(0, r, g, b);
        return;
    }
    let per = rows.div_ceil(THREADS.min(rows)) * width;
    std::thread::scope(|scope| {
        for (k, ((r, g), b)) in r
            .chunks_mut(per)
            .zip(g.chunks_mut(per))
            .zip(b.chunks_mut(per))
            .enumerate()
        {
            let work = &work;
            scope.spawn(move || work(k * per / width, r, g, b));
        }
    });
}

/// Three planes of 0..1 values (`[R…, G…, B…]`, `width × height` each) as
/// an opaque RGBA8 picture, rounded and clamped by [`unit_byte`].
pub fn planes_to_rgba(planes: &[f32], width: usize, height: usize) -> Vec<u8> {
    let plane = width * height;
    assert!(planes.len() >= 3 * plane);
    let mut out = vec![0u8; plane * 4];
    par_rows(&mut out, width * 4, |first, rows| {
        let start = first * width;
        for (i, px) in rows.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let at = start + i;
            *px = [
                unit_byte(planes[at]),
                unit_byte(planes[plane + at]),
                unit_byte(planes[2 * plane + at]),
                255,
            ];
        }
    });
    out
}

/// How far byte picture (or mask) `output` is from `reference`: PSNR over
/// every byte, the largest byte difference and, for a mask, the IoU of the
/// two at half opacity (128). The yardstick for "Fast" against fp32
/// (`accel`, `ml bench --compare`).
pub fn compare(reference: &[u8], output: &[u8], mask: bool) -> crate::protocol::Quality {
    let n = reference.len().min(output.len()).max(1);
    let mut sum = 0u64;
    let mut max = 0u8;
    let (mut both, mut either) = (0u64, 0u64);
    for (&a, &b) in reference.iter().zip(output) {
        let d = a.abs_diff(b);
        sum += u64::from(d) * u64::from(d);
        max = max.max(d);
        if mask {
            let (a, b) = (a >= 128, b >= 128);
            both += u64::from(a && b);
            either += u64::from(a || b);
        }
    }
    let mse = sum as f64 / n as f64;
    crate::protocol::Quality {
        psnr_db: if mse > 0.0 {
            ((10.0 * (255.0f64 * 255.0 / mse).log10()) as f32).min(crate::protocol::PSNR_IDENTICAL)
        } else {
            crate::protocol::PSNR_IDENTICAL
        },
        max_diff: f32::from(max),
        iou: mask.then(|| {
            if either == 0 {
                1.0
            } else {
                both as f32 / either as f32
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_byte_rounds_exactly_like_round() {
        let reference = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        // Every byte's own value, the half-way points between bytes and
        // the floats right next to them, and values out of range.
        let mut values = vec![-1.0f32, -0.0, 1.0, 1.5, f32::NAN, f32::INFINITY];
        for k in 0..=255u32 {
            let at = k as f32 / 255.0;
            let half = (k as f32 + 0.5) / 255.0;
            for v in [
                at,
                half,
                f32::from_bits(half.to_bits() - 1),
                f32::from_bits(half.to_bits() + 1),
            ] {
                values.push(v);
            }
        }
        for i in 0..100_000 {
            values.push(i as f32 / 99_999.0);
        }
        for v in values {
            assert_eq!(unit_byte(v), reference(v), "{v}");
        }
    }

    #[test]
    fn compare_measures_psnr_and_iou() {
        let a = [0u8, 255, 200, 10];
        let same = compare(&a, &a, true);
        assert_eq!(same.psnr_db, crate::protocol::PSNR_IDENTICAL);
        assert_eq!(same.iou, Some(1.0));
        let b = [0u8, 255, 100, 10];
        let q = compare(&a, &b, true);
        // MSE 100² / 4 = 2500: PSNR = 10 log10(255² / 2500) = 14.15 dB.
        assert!((q.psnr_db - 14.15).abs() < 0.01, "{}", q.psnr_db);
        assert_eq!(q.max_diff, 100.0);
        assert_eq!(q.iou, Some(0.5));
        assert_eq!(compare(&a, &b, false).iou, None);
    }

    #[test]
    fn planes_go_both_ways_on_any_thread_count() {
        let (w, h) = (513, 300);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 256) as u8, (i * 7 % 256) as u8, (i / w) as u8, 9])
            .collect();
        let mut planes = vec![0.0f32; 3 * w * h];
        rgba_to_planes(&rgba, w, h, &mut planes);
        assert_eq!(planes[w * h + 1], 7.0 / 255.0, "G of pixel 1");
        let back = planes_to_rgba(&planes, w, h);
        for (a, b) in back.as_chunks::<4>().0.iter().zip(rgba.as_chunks::<4>().0) {
            assert_eq!(&a[..3], &b[..3]);
            assert_eq!(a[3], 255);
        }
        let mut normalised = vec![0.0f32; 3 * w * h];
        rgba_to_planes_normalised(&rgba, w, h, [0.5; 3], [0.25; 3], &mut normalised);
        assert!((normalised[0] - (0.0 - 0.5) / 0.25).abs() < 1e-6);
    }
}
