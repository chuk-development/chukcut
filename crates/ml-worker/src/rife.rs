//! RIFE: frames between two frames, by optical flow, without a runtime.
//!
//! RIFE (Huang et al., ECCV 2022, "Real-Time Intermediate Flow Estimation
//! for Video Frame Interpolation"; `github.com/hzwer/Practical-RIFE`, MIT,
//! weights included) estimates the flow from the wanted instant to both
//! neighbours and warps and fuses them, in one network run per new frame.
//! It is what "AI slow motion" means in most editors.
//!
//! The ONNX file is `walterlow/RIFE_fp32_timestep` (MIT): the RIFE v4
//! export `FuryTMP/RIFE_fp32` with its baked `t = 0.5` turned back into an
//! input, so one session makes a frame at any phase between the two. It
//! takes:
//!
//! - `input`: `[1, 6, H, W]`, the earlier frame's planar RGB in channels
//!   0–2 and the later frame's in 3–5, both in 0..1. Any `H × W`: the graph
//!   pads to a multiple of 32 and crops back.
//! - `timestep`: a scalar, the phase, `0 < t < 1` (the endpoints are not
//!   reproduced exactly, so they are never asked for: they are the source
//!   frames themselves).
//!
//! and answers `output`, `[1, 3, H, W]`, planar RGB in about 0..1.

/// The `input` tensor of two RGBA8 frames of the same size: both planar,
/// 0..1, the earlier first. Alpha is ignored (video has none).
pub fn input(first: &[u8], second: &[u8], width: usize, height: usize) -> Vec<f32> {
    let plane = width * height;
    let mut out = vec![0.0f32; 6 * plane];
    for (frame, base) in [(first, 0usize), (second, 3 * plane)] {
        for (i, px) in frame.as_chunks::<4>().0.iter().take(plane).enumerate() {
            out[base + i] = px[0] as f32 / 255.0;
            out[base + plane + i] = px[1] as f32 / 255.0;
            out[base + 2 * plane + i] = px[2] as f32 / 255.0;
        }
    }
    out
}

/// The `output` tensor as an opaque RGBA8 frame, rounded and clamped.
pub fn frame_bytes(output: &[f32], width: usize, height: usize) -> Vec<u8> {
    let plane = width * height;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = Vec::with_capacity(plane * 4);
    for i in 0..plane {
        out.extend_from_slice(&[
            byte(output[i]),
            byte(output[plane + i]),
            byte(output[2 * plane + i]),
            255,
        ]);
    }
    out
}

/// Whether `phase` is one the network is asked for: strictly between the
/// two frames.
pub fn valid_phase(phase: f32) -> bool {
    phase.is_finite() && phase > 0.0 && phase < 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_frames_go_in_planar_and_the_earlier_first() {
        // 2×1 frames: the first red then green, the second blue then white.
        let a = [255, 0, 0, 255, 0, 255, 0, 255];
        let b = [0, 0, 255, 255, 255, 255, 255, 255];
        let t = input(&a, &b, 2, 1);
        assert_eq!(t.len(), 12);
        assert_eq!(&t[0..2], &[1.0, 0.0], "first R");
        assert_eq!(&t[2..4], &[0.0, 1.0], "first G");
        assert_eq!(&t[4..6], &[0.0, 0.0], "first B");
        assert_eq!(&t[6..8], &[0.0, 1.0], "second R");
        assert_eq!(&t[10..12], &[1.0, 1.0], "second B");
    }

    #[test]
    fn the_output_comes_back_opaque_and_clamped() {
        let out = [1.2, 0.5, -0.1, 0.0, 0.25, 1.0];
        assert_eq!(
            frame_bytes(&out, 2, 1),
            vec![255, 0, 64, 255, 128, 0, 255, 255]
        );
        assert!(valid_phase(0.25));
        assert!(!valid_phase(0.0));
        assert!(!valid_phase(1.0));
        assert!(!valid_phase(f32::NAN));
    }
}
