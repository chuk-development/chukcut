//! LaMa: fill a masked part of a picture, without a runtime.
//!
//! LaMa (Suvorov et al., WACV 2022, "Resolution-robust Large Mask Inpainting
//! with Fourier Convolutions"; `github.com/advimman/lama`, Apache-2.0,
//! weights included) paints what plausibly lies behind a mask from the rest
//! of the picture. Its fast Fourier convolutions see the whole input at
//! once, which is why it continues long structures (a fence, a horizon)
//! through a hole better than older inpainters.
//!
//! The ONNX file is Carve's `lama_fp32.onnx` (Apache-2.0), an fp32 export
//! of big-lama with its Fourier units rewritten for ONNX. It takes:
//!
//! - `image`: `[1, 3, 512, 512]`, planar RGB in 0..1;
//! - `mask`: `[1, 1, 512, 512]`, 1 where the picture is to be filled, 0
//!   elsewhere (the graph blanks the masked pixels itself);
//!
//! and answers `output`, `[1, 3, 512, 512]`, planar RGB in **0..255**. The
//! size is fixed by the export (its Fourier units are traced at 512), so a
//! crop of any size is stretched to the square and the answer stretched
//! back. The engine sends a crop around the mask, not the whole frame
//! (`modules/enhance`), so the stretch costs little detail where it matters.

/// The network's input and output size, both sides.
pub const SIZE: usize = 512;

/// Bilinear sample of channel `c` of an RGBA8 image at continuous pixel
/// coordinates (pixel centres at `i + 0.5`).
fn sample(rgba: &[u8], w: usize, h: usize, x: f32, y: f32, c: usize) -> f32 {
    let x = (x - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (y - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |xx: usize, yy: usize| rgba[(yy * w + xx) * 4 + c] as f32;
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// The `image` tensor of an RGBA8 picture: stretched to 512², planar,
/// 0..1. Alpha is ignored.
pub fn image_input(rgba: &[u8], width: usize, height: usize) -> Vec<f32> {
    let plane = SIZE * SIZE;
    let mut out = vec![0.0f32; 3 * plane];
    let (sx, sy) = (width as f32 / SIZE as f32, height as f32 / SIZE as f32);
    for y in 0..SIZE {
        let fy = (y as f32 + 0.5) * sy;
        for x in 0..SIZE {
            let fx = (x as f32 + 0.5) * sx;
            for c in 0..3 {
                out[c * plane + y * SIZE + x] = sample(rgba, width, height, fx, fy, c) / 255.0;
            }
        }
    }
    out
}

/// The `mask` tensor of a `width × height` mask (nonzero = fill): 1 for
/// every 512² cell that covers any masked pixel. Conservative on purpose: a
/// thin stroke must not vanish when the crop is shrunk to the square, or
/// the network would copy the object it was asked to remove.
pub fn mask_input(mask: &[u8], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; SIZE * SIZE];
    // Each source pixel marks the cells it overlaps; for a crop larger than
    // the square that is one cell, for a smaller one a block of them.
    let (sx, sy) = (SIZE as f32 / width as f32, SIZE as f32 / height as f32);
    for y in 0..height {
        let row = &mask[y * width..(y + 1) * width];
        if row.iter().all(|&m| m == 0) {
            continue;
        }
        let y0 = ((y as f32 * sy).floor() as usize).min(SIZE - 1);
        let y1 = (((y + 1) as f32 * sy).ceil() as usize).clamp(y0 + 1, SIZE);
        for (x, &m) in row.iter().enumerate() {
            if m == 0 {
                continue;
            }
            let x0 = ((x as f32 * sx).floor() as usize).min(SIZE - 1);
            let x1 = (((x + 1) as f32 * sx).ceil() as usize).clamp(x0 + 1, SIZE);
            for cy in y0..y1 {
                out[cy * SIZE + x0..cy * SIZE + x1].fill(1.0);
            }
        }
    }
    out
}

/// The `output` tensor (planar RGB, 0..255, 512²) stretched back to
/// `width × height`, as opaque RGBA8.
pub fn output_rgba(output: &[f32], width: usize, height: usize) -> Vec<u8> {
    let plane = SIZE * SIZE;
    let (sx, sy) = (SIZE as f32 / width as f32, SIZE as f32 / height as f32);
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (SIZE - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(SIZE - 1);
        for x in 0..width {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (SIZE - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(SIZE - 1);
            for c in 0..3 {
                let at = |xx: usize, yy: usize| output[c * plane + yy * SIZE + xx];
                let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
                let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
                let v = top * (1.0 - ty) + bottom * ty;
                out.push(v.clamp(0.0, 255.0).round() as u8);
            }
            out.push(255);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_pixel_stroke_survives_the_shrink_to_the_square() {
        // A 2048-wide crop with one masked pixel: four pixels per cell.
        let (w, h) = (2048, 16);
        let mut mask = vec![0u8; w * h];
        mask[5 * w + 1001] = 255;
        let m = mask_input(&mask, w, h);
        assert_eq!(m.iter().filter(|&&v| v > 0.0).count(), 32);
        // The cell under the pixel is set: x 1001/4 = 250, y 5*32 = 160..192.
        assert_eq!(m[170 * SIZE + 250], 1.0);
        assert_eq!(m[170 * SIZE + 252], 0.0);
    }

    #[test]
    fn a_small_crop_stretches_its_mask_and_comes_back_at_its_size() {
        let (w, h) = (64, 32);
        let mut mask = vec![0u8; w * h];
        mask[0] = 1;
        let m = mask_input(&mask, w, h);
        // One source pixel covers 8 × 16 cells.
        assert_eq!(m.iter().filter(|&&v| v > 0.0).count(), 8 * 16);
        let flat: Vec<f32> = (0..3 * SIZE * SIZE)
            .map(|i| if i < SIZE * SIZE { 300.0 } else { 100.0 })
            .collect();
        let back = output_rgba(&flat, w, h);
        assert_eq!(back.len(), w * h * 4);
        assert_eq!(&back[..4], &[255, 100, 100, 255], "clamped and opaque");
    }

    #[test]
    fn the_image_goes_in_planar_in_unit_range() {
        let rgba = [255u8, 0, 128, 255].repeat(4);
        let t = image_input(&rgba, 2, 2);
        assert_eq!(t.len(), 3 * SIZE * SIZE);
        assert_eq!(t[0], 1.0);
        assert_eq!(t[SIZE * SIZE], 0.0);
        assert!((t[2 * SIZE * SIZE] - 128.0 / 255.0).abs() < 1e-6);
    }
}
