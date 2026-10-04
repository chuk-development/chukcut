//! Finding the faces of a stretch of video, frame by frame, through the ML
//! worker, into the clip's landmark track.

use std::path::{Path, PathBuf};

use super::track::{self, Face, FaceFrame};
use crate::modules::analysis::frames::{self, Walk};
use crate::modules::analysis::jobs::JobContext;
use crate::modules::ml;
use crate::modules::project::document::{Micros, Project, TimeRange};

/// The long side frames are analysed at. The face mesh reads a 256-pixel
/// crop around each face; at 1280 a face a fifth of a 9:16 frame wide is
/// still 144 px, enough for the mesh to see eyelids and lip edges.
pub const ANALYSIS_LONG_SIDE: u32 = 1280;
/// Frames are written to the track file every this many, so the preview
/// starts using the first seconds while the rest is analysed.
const FLUSH_EVERY: usize = 60;

/// What to analyse: a video file's stretch.
#[derive(Debug, Clone)]
pub struct Job {
    pub media: PathBuf,
    pub fps: f64,
    pub range: TimeRange,
    /// The displayed frame's size, for the analysis size.
    pub size: (u32, u32),
    /// The track file.
    pub track: PathBuf,
}

impl Job {
    /// The job for `segment_id`'s source range. Only a video clip has faces
    /// to follow over time.
    pub fn for_segment(project: &Project, segment_id: &str) -> Result<Job, String> {
        let (_, segment) = project
            .segment(segment_id)
            .ok_or("the clip is no longer on the timeline")?;
        let video = project
            .materials
            .video(&segment.material_id)
            .ok_or("only a video clip has faces to find")?;
        let range = segment
            .source_range
            .intersect(&TimeRange::new(0, video.duration.max(1)))
            .ok_or("the clip shows none of its file")?;
        Ok(Job {
            media: PathBuf::from(&video.path),
            fps: video.fps,
            range,
            size: (video.width.max(1), video.height.max(1)),
            track: track::path_for(Path::new(&video.path), ml::landmarks::model_version())?,
        })
    }

    /// One frame's length.
    pub fn period(&self) -> Micros {
        let fps = if self.fps.is_finite() && self.fps > 1.0 {
            self.fps
        } else {
            30.0
        };
        (1_000_000.0 / fps).round() as Micros
    }

    /// `(analysed, total)` frames of the range.
    pub fn coverage(&self) -> (usize, usize) {
        let frames = track::read(&self.track).unwrap_or_default();
        track::coverage(&frames, self.range.start, self.range.end(), self.period())
    }

    fn analysis_height(&self) -> u32 {
        let (w, h) = self.size;
        let long = w.max(h);
        if long <= ANALYSIS_LONG_SIDE {
            h
        } else {
            ((h as u64 * ANALYSIS_LONG_SIDE as u64) / long as u64) as u32
        }
    }
}

/// Analyse the job's range (all of it: a re-run replaces what was there),
/// reporting through `ctx`. Returns the number of frames with a face.
pub fn run(job: &Job, ctx: Option<&JobContext>) -> Result<usize, String> {
    let never = std::sync::atomic::AtomicBool::new(false);
    let cancel = ctx.map_or(&never, |c| c.cancel_flag());
    ml::landmarks::prepare(&|_, _| {}, cancel).map_err(|e| e.to_string())?;
    let walk = Walk {
        path: job.media.to_string_lossy().into_owned(),
        fps: job.fps,
        range: job.range,
        height: job.analysis_height(),
        max_rate: None,
        sequence: None,
    };
    let mut hints: Vec<[f32; 4]> = Vec::new();
    let mut pending: Vec<FaceFrame> = Vec::new();
    let mut with_face = 0usize;
    frames::walk(&walk, ctx, (0.0, 1.0), |frame| {
        let meshes =
            ml::landmarks::landmarks(&frame.rgba, frame.width, frame.height, &hints, Some(cancel))
                .map_err(|e| e.to_string())?;
        hints = meshes.iter().map(|m| m.roi).collect();
        let (w, h) = (frame.width as f32, frame.height as f32);
        let faces: Vec<Face> = meshes
            .iter()
            .map(|m| Face {
                score: m.score,
                points: m.points.iter().map(|p| [p[0] / w, p[1] / h]).collect(),
            })
            .collect();
        if !faces.is_empty() {
            with_face += 1;
        }
        pending.push(FaceFrame {
            t: frame.pts,
            faces,
        });
        if pending.len() >= FLUSH_EVERY {
            track::merge_into(&job.track, std::mem::take(&mut pending))?;
        }
        Ok(())
    })?;
    track::merge_into(&job.track, pending)?;
    Ok(with_face)
}
