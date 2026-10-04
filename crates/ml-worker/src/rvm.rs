//! Robust Video Matting (RVM): a person's alpha matte, frame after frame,
//! without a runtime.
//!
//! RVM (Lin et al., 2021; `github.com/PeterL1n/RobustVideoMatting`, GPL-3.0)
//! is a recurrent network: four hidden states go out with every frame's
//! matte and come back in with the next frame. That is what makes it stable
//! over time (a per-frame model such as BiRefNet flickers at hair and
//! edges), and it is why a matte is only as good as the run of consecutive
//! frames before it. The caller feeds frames in order and starts a fresh
//! session at a cut or a seek.
//!
//! The ONNX export takes:
//!
//! - `src`: the frame, `[1, 3, H, W]`, RGB in 0..1;
//! - `r1i`..`r4i`: the states, `[1, 1, 1, 1]` zeros on the first frame;
//! - `downsample_ratio`: `[1]`, how far the network shrinks the frame before
//!   its encoder. Its deep guided filter brings the matte back to `H × W`.
//!
//! and answers `fgr` (foreground colour, unused here), `pha` (alpha,
//! `[1, 1, H, W]`, 0..1) and `r1o`..`r4o`.

/// The long side, in pixels, the network's encoder should see. The authors'
/// table puts every resolution at about this (0.25 at 1080p, 0.125 at 4K):
/// smaller loses hair, larger costs time and gains little.
pub const ENCODER_LONG_SIDE: f32 = 512.0;

/// The `downsample_ratio` for a `width × height` frame: the authors' rule,
/// the ratio that brings the long side to [`ENCODER_LONG_SIDE`], never
/// above 1.
pub fn downsample_ratio(width: usize, height: usize) -> f32 {
    let long = width.max(height).max(1) as f32;
    (ENCODER_LONG_SIDE / long).min(1.0)
}

/// The `src` tensor of an RGBA8 frame: planar RGB, 0..1. Alpha is ignored.
pub fn input(rgba: &[u8], width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; 3 * width * height];
    crate::pixels::rgba_to_planes(rgba, width, height, &mut out);
    out
}

/// The `pha` output as 8-bit alpha, one byte per pixel, rounded.
pub fn alpha_bytes(pha: &[f32]) -> Vec<u8> {
    pha.iter()
        .map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect()
}

/// One recurrent state tensor: its shape and its values.
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub shape: Vec<i64>,
    pub data: Vec<f32>,
}

impl State {
    /// The first frame's state: a single zero, which the network
    /// broadcasts.
    pub fn zero() -> Self {
        State {
            shape: vec![1, 1, 1, 1],
            data: vec![0.0],
        }
    }
}

/// The names of the recurrent inputs and the outputs that feed them.
pub const STATE_INPUTS: [&str; 4] = ["r1i", "r2i", "r3i", "r4i"];
pub const STATE_OUTPUTS: [&str; 4] = ["r1o", "r2o", "r3o", "r4o"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoder_sees_about_512_pixels() {
        assert!((downsample_ratio(1920, 1080) - 0.2667).abs() < 1e-3);
        assert!((downsample_ratio(960, 540) - 0.5333).abs() < 1e-3);
        // Small frames are not upscaled.
        assert_eq!(downsample_ratio(320, 240), 1.0);
        // Portrait frames use their long side too.
        assert_eq!(downsample_ratio(540, 960), downsample_ratio(960, 540));
    }

    #[test]
    fn the_input_is_planar_rgb_in_unit_range() {
        let rgba = [255, 0, 51, 9, 0, 255, 102, 9];
        let t = input(&rgba, 2, 1);
        assert_eq!(t.len(), 6);
        assert_eq!(&t[0..2], &[1.0, 0.0]);
        assert_eq!(&t[2..4], &[0.0, 1.0]);
        assert!((t[4] - 0.2).abs() < 1e-6 && (t[5] - 0.4).abs() < 1e-6);
    }

    #[test]
    fn alpha_is_clamped_and_rounded() {
        assert_eq!(
            alpha_bytes(&[-0.2, 0.0, 0.5, 0.999, 1.4]),
            [0, 0, 128, 255, 255]
        );
    }
}
