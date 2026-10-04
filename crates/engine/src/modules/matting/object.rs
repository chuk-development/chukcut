//! "Select object": a SAM mask on the clicked frame, propagated over the
//! clip.
//!
//! MobileSAM segments one frame from a prompt; it knows nothing of the frame
//! before. SAM 2's video predictor would carry a memory from frame to frame,
//! but its memory encoder and attention have no published ONNX export that
//! runs on ONNX Runtime's CPU and CUDA providers (the research note,
//! `docs/research/ml-features.md` §2.3). So the propagation is the classic
//! one: **track, then prompt.**
//!
//! 1. The clicks segment the clicked frame. The mask's bounding box is the
//!    object.
//! 2. VitTrack (tracker T2, already in the worker) follows that box frame
//!    by frame, forwards and backwards from the clicked frame.
//! 3. On every frame, SAM gets the tracked box (grown by a tenth, so a
//!    part that grew past it is not clipped) and the clicked points carried
//!    along with the box, and draws the object's mask inside it.
//!
//! Without VitTrack (it failed to load) the last mask's box is the next
//! frame's prompt, which follows slow motion and loses fast motion.
//!
//! Frames already baked are only tracked, not segmented: the tracker needs
//! every frame, the cache already has the mask.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use chukcut_ml_worker::protocol::SegmentPoint;
use chukcut_ml_worker::sam;

use super::bake::{BakeJob, BakeOutcome, BakeProgress};
use super::cache;
use crate::modules::ml::segment;
use crate::modules::ml::tracker::VitTracker;
use crate::modules::project::document::{Micros, SAMPLE_SLACK};

/// How much the tracked box grows on each side before it becomes SAM's
/// prompt, as a fraction of its size.
const BOX_MARGIN: f32 = 0.1;

/// Frames decoded at once for the backward walk: decoders only go forward
/// cheaply, so the walk decodes a chunk forward and segments it in reverse.
const CHUNK: usize = 24;

/// The clicked points of `job` in pixels of a `w × h` frame.
fn clicks(job: &BakeJob, w: u32, h: u32) -> Vec<SegmentPoint> {
    job.setting
        .prompt
        .as_ref()
        .map(|p| {
            p.points
                .iter()
                .map(|q| SegmentPoint {
                    x: q.x * w as f32,
                    y: q.y * h as f32,
                    keep: q.keep,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `b` grown by [`BOX_MARGIN`] on each side and kept inside the frame.
pub fn grow(b: [f32; 4], w: u32, h: u32) -> [f32; 4] {
    let (mx, my) = (b[2] * BOX_MARGIN, b[3] * BOX_MARGIN);
    let x0 = (b[0] - mx).max(0.0);
    let y0 = (b[1] - my).max(0.0);
    let x1 = (b[0] + b[2] + mx).min(w as f32);
    let y1 = (b[1] + b[3] + my).min(h as f32);
    [x0, y0, (x1 - x0).max(1.0), (y1 - y0).max(1.0)]
}

/// `points` (inside box `from`) carried to where box `to` is: the same
/// place relative to the box, so a point on the object's left stays on its
/// left as it moves and grows. Points that fall outside the frame are
/// dropped.
pub fn carry(
    points: &[SegmentPoint],
    from: [f32; 4],
    to: [f32; 4],
    w: u32,
    h: u32,
) -> Vec<SegmentPoint> {
    points
        .iter()
        .filter(|p| p.keep)
        .filter_map(|p| {
            let rx = (p.x - from[0]) / from[2].max(1.0);
            let ry = (p.y - from[1]) / from[3].max(1.0);
            let x = to[0] + rx * to[2];
            let y = to[1] + ry * to[3];
            ((0.0..w as f32).contains(&x) && (0.0..h as f32).contains(&y)).then_some(SegmentPoint {
                x,
                y,
                keep: true,
            })
        })
        .collect()
}

/// One direction of the walk from the clicked frame.
struct Walk {
    tracker: Option<VitTracker>,
    /// The object's box on the clicked frame, and the clicks there.
    origin: [f32; 4],
    points: Vec<SegmentPoint>,
    /// Where the object was last seen.
    last: [f32; 4],
    size: (u32, u32),
}

impl Walk {
    fn start(first: &[u8], size: (u32, u32), origin: [f32; 4], points: Vec<SegmentPoint>) -> Walk {
        let tracker = match VitTracker::start(first, size.0 as usize, size.1 as usize, origin) {
            Ok(tracker) => Some(tracker),
            Err(error) => {
                tracing::info!(%error, "no VitTrack for the propagation");
                None
            }
        };
        Walk {
            tracker,
            origin,
            points,
            last: origin,
            size,
        }
    }

    /// Follow the object into `frame`; with `segment`, also make its mask.
    fn step(&mut self, frame: &[u8], segment: bool) -> Result<Option<Vec<u8>>, String> {
        let (w, h) = self.size;
        let tracked = match &mut self.tracker {
            // Whole-frame re-detection, so an object that was hidden for a
            // moment is found again rather than prompted at where it was.
            Some(tracker) => {
                tracker
                    .update(frame, w as usize, h as usize, true)
                    .map_err(|e| e.to_string())?
                    .0
            }
            None => None,
        };
        let at = tracked.unwrap_or(self.last);
        if !segment {
            self.last = at;
            return Ok(None);
        }
        let points = carry(&self.points, self.origin, at, w, h);
        let mask = segment::segment(frame, w as usize, h as usize, &points, Some(grow(at, w, h)))
            .map_err(|e| e.to_string())?;
        // With a tracker its box leads; without one the mask's own box is
        // the best guess of where the object is next.
        self.last = tracked
            .or_else(|| sam::mask_box(&mask.alpha, w as usize, h as usize))
            .unwrap_or(at);
        Ok(Some(mask.alpha))
    }
}

/// Bake the missing masks of a "Select object" job into `dir`.
pub(super) fn run(
    job: &BakeJob,
    dir: &Path,
    existing: &[Micros],
    missing: &[Micros],
    cancel: &AtomicBool,
    report: &mut dyn FnMut(&BakeProgress),
) -> Result<BakeOutcome, String> {
    let prompt = job
        .setting
        .prompt
        .as_ref()
        .ok_or("Select object has no clicks on this clip yet")?;
    let mut decoder = job.open_decoder()?;
    let period = job.period();
    let first = decoder
        .seek_and_decode(prompt.time.max(0) + SAMPLE_SLACK)
        .map_err(|e| format!("decode failed at {} µs: {e}", prompt.time))?;
    let size = (first.width, first.height);
    let points = clicks(job, size.0, size.1);
    let mask = segment::segment(&first.data, size.0 as usize, size.1 as usize, &points, None)
        .map_err(|e| e.to_string())?;
    let Some(origin) = sam::mask_box(&mask.alpha, size.0 as usize, size.1 as usize) else {
        return Err("nothing was selected where you clicked; click inside the object".into());
    };
    let mut outcome = BakeOutcome {
        provider: Some(mask.provider.clone()),
        frames: 1,
        ..BakeOutcome::default()
    };
    let needed = |pts: Micros| existing.binary_search(&pts).is_err();
    if needed(first.pts) {
        cache::write(dir, first.pts, size.0, size.1, mask.alpha)?;
        outcome.written += 1;
    }
    let lo = missing.first().copied().unwrap_or(first.pts);
    let hi = missing.last().copied().unwrap_or(first.pts);
    let forward = job.frame_times(first.pts + period, hi);
    let backward = if lo < first.pts {
        job.frame_times(lo, first.pts - 1)
    } else {
        Vec::new()
    };
    let mut progress = BakeProgress {
        done: 1,
        total: (1 + forward.len() + backward.len()) as u32,
    };
    report(&progress);

    if !forward.is_empty() {
        let mut walk = Walk::start(&first.data, size, origin, points.clone());
        let mut last_pts = first.pts;
        for &t in &forward {
            if cancel.load(Ordering::Relaxed) {
                outcome.cancelled = true;
                return Ok(outcome);
            }
            let frame = decoder
                .seek_and_decode(t + SAMPLE_SLACK)
                .map_err(|e| format!("decode failed at {t} µs: {e}"))?;
            progress.done += 1;
            if frame.pts <= last_pts {
                continue;
            }
            last_pts = frame.pts;
            let want = needed(frame.pts);
            if let Some(alpha) = walk.step(&frame.data, want)? {
                cache::write(dir, frame.pts, frame.width, frame.height, alpha)?;
                outcome.written += 1;
            }
            outcome.frames += 1;
            report(&progress);
        }
    }

    if !backward.is_empty() {
        let mut walk = Walk::start(&first.data, size, origin, points);
        // Chunks from the clicked frame back to the start of the range.
        for chunk in backward.rchunks(CHUNK) {
            let mut frames = Vec::with_capacity(chunk.len());
            for &t in chunk {
                let frame = decoder
                    .seek_and_decode(t + SAMPLE_SLACK)
                    .map_err(|e| format!("decode failed at {t} µs: {e}"))?;
                if frames
                    .last()
                    .is_some_and(|f: &crate::modules::media::decoder::DecodedFrame| {
                        f.pts >= frame.pts
                    })
                    || frame.pts >= first.pts
                {
                    continue;
                }
                frames.push(frame);
            }
            for frame in frames.into_iter().rev() {
                if cancel.load(Ordering::Relaxed) {
                    outcome.cancelled = true;
                    return Ok(outcome);
                }
                progress.done += 1;
                let want = needed(frame.pts);
                if let Some(alpha) = walk.step(&frame.data, want)? {
                    cache::write(dir, frame.pts, frame.width, frame.height, alpha)?;
                    outcome.written += 1;
                }
                outcome.frames += 1;
                report(&progress);
            }
        }
    }
    progress.done = progress.total;
    report(&progress);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_ride_along_with_the_box() {
        let points = [
            SegmentPoint {
                x: 15.0,
                y: 15.0,
                keep: true,
            },
            SegmentPoint {
                x: 1.0,
                y: 1.0,
                keep: false,
            },
        ];
        // The box moved right by 50 and doubled.
        let carried = carry(
            &points,
            [10.0, 10.0, 10.0, 10.0],
            [60.0, 10.0, 20.0, 20.0],
            200,
            100,
        );
        assert_eq!(carried.len(), 1, "exclusions stay on the clicked frame");
        assert_eq!((carried[0].x, carried[0].y), (70.0, 20.0));
        // Carried out of the frame: dropped.
        assert!(carry(
            &points,
            [10.0, 10.0, 10.0, 10.0],
            [195.0, 10.0, 10.0, 10.0],
            200,
            100
        )
        .is_empty());
    }

    #[test]
    fn the_prompt_box_grows_and_stays_in_the_frame() {
        assert_eq!(
            grow([10.0, 10.0, 100.0, 50.0], 640, 360),
            [0.0, 5.0, 120.0, 60.0]
        );
        let edge = grow([600.0, 300.0, 40.0, 60.0], 640, 360);
        assert_eq!(edge[0] + edge[2], 640.0);
        assert_eq!(edge[1] + edge[3], 360.0);
    }
}
