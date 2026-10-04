//! The voice out of a recording, for "Isolate voice" (`voice::isolate`):
//! HTDemucs in the ML worker (`chukcut_ml_worker::demucs`).
//!
//! The model takes a fixed 7.8 s segment, so a recording is cut into
//! segments that overlap by a quarter and the answers are cross-faded
//! linearly across each overlap ([`Separator`]). Demucs is weakest at a
//! segment's edges, where it hears half a context; the cross-fade gives
//! each output sample mostly to the segment that heard it in the middle.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_ml_worker::protocol::{Outcome, RequestBody};
use chukcut_ml_worker::registry::{SEPARATION_RATE, SEPARATION_SEGMENT};

use super::{worker, MlError};

/// The registry id of the separation model.
pub const MODEL: &str = "htdemucs-vocals";
/// The rate sound is separated at.
pub const RATE: u32 = SEPARATION_RATE;
/// Frames per model segment.
pub const SEGMENT: usize = SEPARATION_SEGMENT;
/// Frames two neighbouring segments share.
pub const OVERLAP: usize = SEGMENT / 4;
/// Frames from one segment's start to the next.
pub const HOP: usize = SEGMENT - OVERLAP;

/// The version of [`MODEL`] this build runs, which the cache key records.
pub fn model_version() -> &'static str {
    super::matte::version_of(MODEL).expect("the registry lists the separation model")
}

/// Make the model ready; returns the provider it runs on.
pub fn prepare(
    progress: &dyn Fn(&str, Option<f32>),
    cancel: &AtomicBool,
) -> Result<String, MlError> {
    super::prepare(MODEL, "the voice separation model", progress, cancel)
}

/// The vocals of one stretch of planar stereo (`left`, `right`, at most
/// [`SEGMENT`] frames each), and the provider it ran on.
pub fn separate_segment(
    left: &[f32],
    right: &[f32],
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<f32>, Vec<f32>, String), MlError> {
    let frames = left.len();
    if frames == 0 || frames > SEGMENT || right.len() != frames {
        return Err(MlError::Failed(format!(
            "a segment is 1 to {SEGMENT} frames per channel, got {} and {}",
            left.len(),
            right.len()
        )));
    }
    let mut payload = Vec::with_capacity(frames * 8);
    for v in left.iter().chain(right) {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    let body = RequestBody::Separate {
        model: MODEL.into(),
        frames: frames as u32,
    };
    // A segment is ~4 s on eight CPU threads, well under a second on CUDA;
    // a minute means the worker is wedged.
    let (outcome, bytes) =
        worker::request_payload(body, &payload, &|_, _| {}, cancel, Duration::from_secs(120))?;
    match outcome {
        Outcome::Separated { provider, .. } if bytes.len() == frames * 8 => {
            let values: Vec<f32> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect();
            let (l, r) = values.split_at(frames);
            Ok((l.to_vec(), r.to_vec(), provider))
        }
        Outcome::Separated { .. } => Err(MlError::Failed(format!(
            "the worker sent {} bytes for {frames} frames",
            bytes.len()
        ))),
        other => Err(MlError::Failed(format!("unexpected answer {other:?}"))),
    }
}

/// Overlap-add of segment answers into one continuous signal, streaming:
/// [`Self::push`] takes each segment's output (segments start [`HOP`] frames
/// apart) and hands back the frames that are final; [`Self::finish`] the
/// rest. Planar stereo.
#[derive(Default)]
pub struct Separator {
    /// The previous segment's last [`OVERLAP`] frames, waiting to be faded
    /// into the next one's first.
    tail: Option<[Vec<f32>; 2]>,
}

impl Separator {
    pub fn new() -> Self {
        Self::default()
    }

    /// One segment's output (`SEGMENT` frames per channel, or fewer for the
    /// last). Returns the frames now final: up to the next segment's start.
    pub fn push(&mut self, segment: [Vec<f32>; 2]) -> [Vec<f32>; 2] {
        let frames = segment[0].len();
        let mut out = segment.clone();
        if let Some(tail) = self.tail.take() {
            let n = OVERLAP.min(frames);
            for (c, channel) in out.iter_mut().enumerate() {
                for i in 0..n {
                    // Linear: the old segment fades out as the new fades in.
                    let w = (i as f32 + 0.5) / OVERLAP as f32;
                    channel[i] = tail[c][i] * (1.0 - w) + channel[i] * w;
                }
            }
        }
        if frames > HOP {
            let keep = [out[0].split_off(HOP), out[1].split_off(HOP)];
            self.tail = Some(keep);
        }
        out
    }

    /// Whatever the last segment left beyond the hop.
    pub fn finish(&mut self) -> [Vec<f32>; 2] {
        self.tail.take().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A "model" that returns its input: overlap-add of segment outputs
    /// must give back the original signal exactly, at every length.
    #[test]
    fn overlap_add_of_an_identity_gives_the_signal_back() {
        for total in [100, SEGMENT, SEGMENT + 1, HOP * 3 + 17, SEGMENT * 3] {
            let signal: Vec<f32> = (0..total).map(|i| ((i % 977) as f32) / 977.0).collect();
            let mut sep = Separator::new();
            let mut out: Vec<f32> = Vec::new();
            let mut start = 0;
            loop {
                let end = (start + SEGMENT).min(total);
                let seg = signal[start..end].to_vec();
                let [l, _] = sep.push([seg.clone(), seg]);
                out.extend(l);
                if end == total {
                    break;
                }
                start += HOP;
            }
            out.extend(sep.finish()[0].clone());
            assert_eq!(out.len(), total, "length at {total}");
            for (i, (a, b)) in signal.iter().zip(&out).enumerate() {
                assert!((a - b).abs() < 1e-5, "frame {i} of {total}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn the_overlap_is_a_cross_fade() {
        // Segment answers of all ones, then all zeros: across the overlap
        // the output falls linearly from one to zero.
        let mut sep = Separator::new();
        let first = sep.push([vec![1.0; SEGMENT], vec![1.0; SEGMENT]]);
        assert_eq!(first[0].len(), HOP);
        let second = sep.push([vec![0.0; SEGMENT], vec![0.0; SEGMENT]]);
        let fade = &second[0][..OVERLAP];
        assert!(fade[0] > 0.99 && fade[OVERLAP - 1] < 0.01);
        assert!((fade[OVERLAP / 2] - 0.5).abs() < 0.01);
        assert!(fade.windows(2).all(|w| w[1] <= w[0]));
    }
}
