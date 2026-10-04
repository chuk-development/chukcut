//! Running the tracker over a stretch of a video file.
//!
//! Frames are decoded at the analysis size (long side 640 px by default) —
//! swscale does the scaling during the colour conversion it does anyway, so a
//! small frame is close to free and decode stays the bottleneck rather than a
//! full-size RGBA buffer per frame. No lock is held while this runs; the
//! caller passes in what it needs and gets samples back.
//!
//! Backwards tracking decodes a chunk forwards and walks it in reverse, then
//! seeks back one chunk: decoders only go forwards cheaply.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::model::{TrackSample, TrackerKind, FLAG_ANCHOR, FLAG_LOST};
use super::tracker::{BoxState, Frame, StepResult, Tracker, TrackerParams};
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::ml::tracker::VitTracker;
use crate::modules::project::document::{Micros, SAMPLE_SLACK};

/// Which way from the start frame to track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Backward,
    #[default]
    Both,
}

/// Everything a tracking run needs, owned, so the run holds no lock.
#[derive(Debug, Clone)]
pub struct TrackJob {
    pub path: String,
    /// Frame rate of the file, for stepping.
    pub fps: f64,
    /// Displayed size of the file (rotation applied), as the pool records it.
    pub source_size: (u32, u32),
    /// Source time of the frame the box was drawn on.
    pub start: Micros,
    /// The box there: centre and size as fractions of the frame.
    pub init: [f32; 4],
    /// The box's rotation, degrees: zero for a new track, the track's own
    /// angle for a re-track so the angles stay continuous.
    pub init_angle: f32,
    /// Source times to stay within, inclusive.
    pub range: (Micros, Micros),
    pub direction: Direction,
    /// Long side of the analysis frames, pixels.
    pub analysis_size: u32,
    /// Which tracker runs. The caller has made sure it can (for VitTrack:
    /// `VitTracker::prepare`), or chose KLT.
    pub tracker: TrackerKind,
}

/// How far a run is.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub done: u32,
    pub total: u32,
    /// The newest sample, for drawing the box live.
    pub latest: Option<TrackSample>,
}

/// What a run produced. `cancelled` runs keep what they had.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub samples: Vec<TrackSample>,
    pub cancelled: bool,
    /// Analysis frame size actually used.
    pub frame_size: (u32, u32),
}

struct Source {
    decoder: VideoDecoder,
    period: Micros,
    size: (usize, usize),
}

impl Source {
    fn open(job: &TrackJob) -> Result<Self, String> {
        let (w, h) = job.source_size;
        let long = w.max(h).max(1) as f32;
        let scale = (job.analysis_size.max(64) as f32 / long).min(1.0);
        let target_height = ((h as f32 * scale).round() as u32).max(16) & !1;
        let decoder = VideoDecoder::open_scaled(&job.path, target_height)
            .map_err(|e| format!("could not open {}: {e}", job.path))?;
        let (ow, oh) = decoder.output_size();
        let fps = if job.fps.is_finite() && job.fps > 1.0 {
            job.fps
        } else {
            30.0
        };
        Ok(Self {
            decoder,
            period: ((1_000_000.0 / fps).round() as Micros).max(1),
            size: (ow as usize, oh as usize),
        })
    }

    /// The frame shown at `time` as RGBA at the analysis size, with its own
    /// timestamp.
    fn frame(&mut self, time: Micros) -> Result<(Micros, Vec<u8>), String> {
        let frame = self
            .decoder
            .seek_and_decode(time + SAMPLE_SLACK)
            .map_err(|e| format!("decode failed at {time} µs: {e}"))?;
        Ok((frame.pts, frame.data))
    }
}

/// The tracker a run uses, behind one `step`.
enum Backend {
    Klt(Box<Tracker>),
    Vit {
        tracker: VitTracker,
        last: BoxState,
        /// Frames lost in a row, for [`scan_now`].
        lost_run: u32,
    },
}

/// Whether the `lost_run`-th lost frame in a row (1 = the first) gets a
/// whole-frame scan. A scan costs about a hundred model runs (~0.2 s on a
/// CPU at 640 px), so every lost frame of a long absence would make a clip
/// where the object leaves for ten seconds take a minute. The first three
/// are scanned, because an object that went behind something usually comes
/// out within a few frames; after that every third, which finds an object
/// that came back within a tenth of a second at 30 fps.
fn scan_now(lost_run: u32) -> bool {
    lost_run <= 3 || lost_run.is_multiple_of(3)
}

/// VitTrack's score after its window is about 0.6–0.75 on a held object and
/// falls towards the worker's 0.25 threshold as it slips; this maps that
/// onto the 0..1 confidence the player draws, with the worker's `CONFIDENT`
/// (0.45) landing on `LOW_CONFIDENCE` (0.5).
fn vit_confidence(score: f32) -> f32 {
    ((score - 0.25) / 0.4).clamp(0.0, 1.0)
}

impl Backend {
    fn start(
        kind: TrackerKind,
        rgba: &[u8],
        size: (usize, usize),
        init: BoxState,
    ) -> Result<Backend, String> {
        Ok(match kind {
            TrackerKind::Klt => {
                let frame = klt_frame(rgba, size);
                Backend::Klt(Box::new(Tracker::new(
                    frame,
                    init,
                    TrackerParams::default(),
                )))
            }
            TrackerKind::VitTrack => {
                let bbox = [
                    init.cx + 0.5 - init.w * 0.5,
                    init.cy + 0.5 - init.h * 0.5,
                    init.w,
                    init.h,
                ];
                Backend::Vit {
                    tracker: VitTracker::start(rgba, size.0, size.1, bbox)?,
                    last: init,
                    lost_run: 0,
                }
            }
        })
    }

    fn step(&mut self, rgba: &[u8], size: (usize, usize)) -> Result<StepResult, String> {
        match self {
            Backend::Klt(tracker) => Ok(tracker.step(klt_frame(rgba, size))),
            Backend::Vit {
                tracker,
                last,
                lost_run,
            } => {
                // `lost_run + 1` is this frame's place in the run of lost
                // frames if the local search misses here too.
                let redetect = scan_now(*lost_run + 1);
                let (bbox, score) = tracker.update(rgba, size.0, size.1, redetect)?;
                *lost_run = if bbox.is_some() { 0 } else { *lost_run + 1 };
                Ok(match bbox {
                    Some(b) => {
                        // The box only: VitTrack has no rotation, so the
                        // angle the track started with is kept.
                        *last = BoxState {
                            cx: b[0] + b[2] * 0.5 - 0.5,
                            cy: b[1] + b[3] * 0.5 - 0.5,
                            w: b[2].max(4.0),
                            h: b[3].max(4.0),
                            angle: last.angle,
                        };
                        StepResult {
                            pose: *last,
                            confidence: vit_confidence(score),
                            lost: false,
                        }
                    }
                    None => StepResult {
                        pose: *last,
                        confidence: vit_confidence(score),
                        lost: true,
                    },
                })
            }
        }
    }
}

fn klt_frame(rgba: &[u8], size: (usize, usize)) -> Frame {
    Frame::from_rgba(
        rgba,
        size.0,
        size.1,
        TrackerParams::default().pyramid_levels,
    )
}

fn to_box(init: [f32; 4], angle: f32, size: (usize, usize)) -> BoxState {
    let (w, h) = (size.0 as f32, size.1 as f32);
    // Fractions measure from the frame's edge; pixel coordinates from the
    // first pixel's centre, half a pixel in.
    BoxState {
        cx: init[0] * w - 0.5,
        cy: init[1] * h - 0.5,
        w: (init[2] * w).max(4.0),
        h: (init[3] * h).max(4.0),
        angle,
    }
}

fn to_sample(
    t: Micros,
    b: &BoxState,
    size: (usize, usize),
    confidence: f32,
    flags: u8,
) -> TrackSample {
    let (w, h) = (size.0 as f32, size.1 as f32);
    TrackSample {
        t,
        x: (b.cx + 0.5) / w,
        y: (b.cy + 0.5) / h,
        w: b.w / w,
        h: b.h / h,
        a: b.angle,
        c: confidence,
        f: flags,
    }
}

/// Track `job`, reporting every frame. Returns the samples in time order,
/// the start frame first marked as an anchor.
pub fn run(
    job: &TrackJob,
    cancel: &AtomicBool,
    mut report: impl FnMut(&Progress),
) -> Result<Outcome, String> {
    let mut source = Source::open(job)?;
    let size = source.size;
    let period = source.period;
    let (lo, hi) = (job.range.0.min(job.range.1), job.range.0.max(job.range.1));
    let start = job.start.clamp(lo, hi);

    let forward = matches!(job.direction, Direction::Forward | Direction::Both);
    let backward = matches!(job.direction, Direction::Backward | Direction::Both);
    let span_frames = |a: Micros, b: Micros| ((b - a).max(0) / period) as u32;
    let total = 1
        + if forward { span_frames(start, hi) } else { 0 }
        + if backward { span_frames(lo, start) } else { 0 };
    let mut progress = Progress {
        done: 0,
        total: total.max(1),
        latest: None,
    };

    let (start_pts, first) = source.frame(start)?;
    let init = to_box(job.init, job.init_angle, size);
    let anchor = to_sample(start_pts, &init, size, 1.0, FLAG_ANCHOR);
    let mut samples = vec![anchor];
    progress.done = 1;
    progress.latest = Some(anchor);
    report(&progress);

    let mut cancelled = false;

    if forward {
        let mut tracker = Backend::start(job.tracker, &first, size, init)?;
        let mut last_pts = start_pts;
        let mut t = start_pts + period;
        while t <= hi {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            let (pts, frame) = source.frame(t)?;
            t += period;
            if pts <= last_pts {
                // A variable-rate file repeated a frame for this step.
                continue;
            }
            last_pts = pts;
            let step = tracker.step(&frame, size)?;
            let flags = if step.lost { FLAG_LOST } else { 0 };
            let sample = to_sample(pts, &step.pose, size, step.confidence, flags);
            samples.push(sample);
            progress.done = (progress.done + 1).min(progress.total);
            progress.latest = Some(sample);
            report(&progress);
        }
    }

    if backward && !cancelled {
        let mut tracker = Backend::start(job.tracker, &first, size, init)?;
        let chunk = 30 * period;
        let mut chunk_end = start_pts;
        'chunks: while chunk_end > lo {
            let chunk_start = (chunk_end - chunk).max(lo);
            let mut frames: Vec<(Micros, Vec<u8>)> = Vec::new();
            let mut t = chunk_start;
            while t < chunk_end {
                let (pts, frame) = source.frame(t)?;
                t += period;
                if pts >= chunk_end || frames.last().is_some_and(|(p, _)| *p >= pts) {
                    continue;
                }
                frames.push((pts, frame));
            }
            if frames.is_empty() {
                break;
            }
            let earliest = frames[0].0;
            for (pts, frame) in frames.into_iter().rev() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break 'chunks;
                }
                let step = tracker.step(&frame, size)?;
                let flags = if step.lost { FLAG_LOST } else { 0 };
                let sample = to_sample(pts, &step.pose, size, step.confidence, flags);
                samples.push(sample);
                progress.done = (progress.done + 1).min(progress.total);
                progress.latest = Some(sample);
                report(&progress);
            }
            if earliest >= chunk_end {
                break;
            }
            chunk_end = earliest;
            if chunk_start <= lo {
                break;
            }
        }
    }

    samples.sort_by_key(|s| s.t);
    samples.dedup_by_key(|s| s.t);
    Ok(Outcome {
        samples,
        cancelled,
        frame_size: (size.0 as u32, size.1 as u32),
    })
}
