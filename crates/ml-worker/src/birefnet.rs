//! BiRefNet lite: the alpha matte of a frame's main object, one frame at a
//! time, without a runtime.
//!
//! BiRefNet (Zheng et al., 2024; `github.com/ZhengPeng7/BiRefNet`, MIT) is
//! a dichotomous segmentation network: it cuts out whatever the picture is
//! about — a product, a pet, a car, a person — with fine edges. It has no
//! state, so it does not know the frame before; a video matte made with it
//! can flicker at the edges where RVM (people, recurrent) would not. That is
//! the trade: any object, at the price of temporal stability.
//!
//! The ONNX export (onnx-community, for transformers.js) takes
//! `input_image`, `[1, 3, 1024, 1024]`, RGB normalised with ImageNet's mean
//! and deviation, and answers `output_image`, `[1, 1, 1024, 1024]`, logits
//! (the sigmoid is not in the graph). Both sizes are fixed: a frame is
//! stretched to the square and the matte stretched back, which is what the
//! authors' own pipeline does.

/// The network's input and output size, both sides.
pub const SIZE: usize = 1024;

const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

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

/// The `input_image` tensor of an RGBA8 frame: stretched to 1024², planar,
/// normalised. Alpha is ignored.
pub fn input(rgba: &[u8], width: usize, height: usize) -> Vec<f32> {
    let plane = SIZE * SIZE;
    let mut out = vec![0.0f32; 3 * plane];
    let (sx, sy) = (width as f32 / SIZE as f32, height as f32 / SIZE as f32);
    for y in 0..SIZE {
        let fy = (y as f32 + 0.5) * sy;
        for x in 0..SIZE {
            let fx = (x as f32 + 0.5) * sx;
            for c in 0..3 {
                let v = sample(rgba, width, height, fx, fy, c) / 255.0;
                out[c * plane + y * SIZE + x] = (v - MEAN[c]) / STD[c];
            }
        }
    }
    out
}

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

/// The network's logits as a `width × height` matte, one byte per pixel:
/// the sigmoid, sampled back from the square.
pub fn matte(logits: &[f32], width: usize, height: usize) -> Vec<u8> {
    let (sx, sy) = (SIZE as f32 / width as f32, SIZE as f32 / height as f32);
    let at = |x: usize, y: usize| logits[y * SIZE + x];
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, (SIZE - 1) as f32);
        let (y0, ty) = (fy as usize, fy.fract());
        let y1 = (y0 + 1).min(SIZE - 1);
        for x in 0..width {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, (SIZE - 1) as f32);
            let (x0, tx) = (fx as usize, fx.fract());
            let x1 = (x0 + 1).min(SIZE - 1);
            // Interpolate the logits, then squash: interpolating alphas
            // instead blurs a hard edge over two input pixels.
            let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
            let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
            let logit = top * (1.0 - ty) + bottom * ty;
            out.push((sigmoid(logit) * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_input_is_the_frame_stretched_and_normalised() {
        // A 2×1 frame: red on the left, blue on the right.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 255];
        let t = input(&rgba, 2, 1);
        assert_eq!(t.len(), 3 * SIZE * SIZE);
        let plane = SIZE * SIZE;
        let red_left = t[0];
        let red_right = t[SIZE - 1];
        assert!((red_left - (1.0 - MEAN[0]) / STD[0]).abs() < 1e-4);
        assert!((red_right - (0.0 - MEAN[0]) / STD[0]).abs() < 1e-4);
        let blue_right = t[2 * plane + SIZE - 1];
        assert!((blue_right - (1.0 - MEAN[2]) / STD[2]).abs() < 1e-4);
        // Every row is the same.
        assert_eq!(t[0], t[(SIZE - 1) * SIZE]);
    }

    #[test]
    fn the_matte_is_the_sigmoid_sampled_back() {
        // Left half "object" (+10), right half background (-10).
        let logits: Vec<f32> = (0..SIZE * SIZE)
            .map(|i| if i % SIZE < SIZE / 2 { 10.0 } else { -10.0 })
            .collect();
        let m = matte(&logits, 64, 4);
        assert_eq!(m.len(), 64 * 4);
        assert_eq!(m[0], 255);
        assert_eq!(m[63], 0);
        // The edge stays within one output pixel of the middle.
        let edge = m[..64].iter().position(|&a| a < 128).unwrap();
        assert!((31..=33).contains(&edge), "edge at {edge}");
    }
}
