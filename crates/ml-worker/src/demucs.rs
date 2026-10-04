//! Source separation with HTDemucs: the voice out of a stretch of sound,
//! without a runtime.
//!
//! The export (`registry`, id `htdemucs-vocals`) takes `mix`,
//! `[1, 2, SEPARATION_SEGMENT]`, stereo at 44.1 kHz in -1..1, and answers
//! `stems`, `[1, 4, 2, SEPARATION_SEGMENT]`: drums, bass, other, vocals.
//! Only the vocals are used. The engine cuts long sound into overlapping
//! segments and cross-fades the answers (`modules::ml::separate`).

use crate::registry::SEPARATION_SEGMENT;

/// Which of the four stems is the voice.
pub const VOCALS: usize = 3;
pub const STEMS: usize = 4;

/// Planar `f32` little-endian bytes (`frames` per channel, two channels) as
/// the model's input: zero-padded to a whole segment. `None` when the byte
/// count does not match.
pub fn input(payload: &[u8], frames: usize) -> Option<Vec<f32>> {
    if frames == 0 || frames > SEPARATION_SEGMENT || payload.len() != frames * 2 * 4 {
        return None;
    }
    let mut out = vec![0.0f32; 2 * SEPARATION_SEGMENT];
    for (i, b) in payload.as_chunks::<4>().0.iter().enumerate() {
        let (channel, frame) = (i / frames, i % frames);
        out[channel * SEPARATION_SEGMENT + frame] = f32::from_le_bytes(*b);
    }
    Some(out)
}

/// The vocals of the model's output, cut back to `frames`, as planar `f32`
/// little-endian bytes.
pub fn vocals_bytes(stems: &[f32], frames: usize) -> Option<Vec<u8>> {
    if stems.len() != STEMS * 2 * SEPARATION_SEGMENT {
        return None;
    }
    let base = VOCALS * 2 * SEPARATION_SEGMENT;
    let mut out = Vec::with_capacity(frames * 2 * 4);
    for channel in 0..2 {
        let start = base + channel * SEPARATION_SEGMENT;
        for v in &stems[start..start + frames] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_go_in_padded_and_the_vocals_come_out_cut() {
        let frames = 3;
        let samples = [0.1f32, 0.2, 0.3, -0.1, -0.2, -0.3];
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_le_bytes()).collect();
        let t = input(&bytes, frames).unwrap();
        assert_eq!(t.len(), 2 * SEPARATION_SEGMENT);
        assert_eq!(&t[..3], &[0.1, 0.2, 0.3]);
        assert_eq!(t[3], 0.0);
        assert_eq!(
            &t[SEPARATION_SEGMENT..SEPARATION_SEGMENT + 3],
            &[-0.1, -0.2, -0.3]
        );
        assert!(input(&bytes, 4).is_none());

        let mut stems = vec![0.0f32; STEMS * 2 * SEPARATION_SEGMENT];
        let base = VOCALS * 2 * SEPARATION_SEGMENT;
        stems[base] = 0.5;
        stems[base + SEPARATION_SEGMENT + 2] = -0.5;
        let out = vocals_bytes(&stems, frames).unwrap();
        let back: Vec<f32> = out
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert_eq!(back, vec![0.5, 0.0, 0.0, 0.0, 0.0, -0.5]);
    }
}
