//! Reading a clip's pixels for the colour tools: a few frames of a video
//! (or the one frame under a given instant), or a photo, small.
//!
//! Split in two so the project lock is never held across a decode: [`plan`]
//! reads what it needs from the document, [`read`] does the IO.

use super::pixels::Samples;
use crate::modules::inspector::edit::GradeEdit;
use crate::modules::media::decoder::VideoDecoder;
use crate::modules::project::document::{Crop, Micros, Project, TimeRange, SAMPLE_SLACK};
use crate::modules::render::lut::Cube;

/// Frames read from a whole clip. Eight spread over its length see most of
/// what a shot holds without decoding all of it.
const FRAMES_PER_CLIP: usize = 8;
/// Height the frames are decoded at. Statistics need no detail.
const HEIGHT: u32 = 180;
/// Roughly how many pixels are kept per frame.
const PIXELS_PER_FRAME: usize = 6000;

/// What to read for one clip.
#[derive(Debug, Clone)]
pub struct Plan {
    source: Source,
    crop: Option<Crop>,
    /// The clip's grade now.
    pub grade: GradeEdit,
}

#[derive(Debug, Clone)]
enum Source {
    Video { path: String, times: Vec<Micros> },
    Image { path: String },
}

/// A clip's pixels and the grade (with its look parsed) they are drawn with.
pub struct Picture {
    pub samples: Samples,
    pub grade: GradeEdit,
    pub lut: Option<Cube>,
}

/// What [`read`] needs to sample `segment_id`: the whole clip, or with `at`
/// (a timeline instant) only the frame shown then.
pub fn plan(project: &Project, segment_id: &str, at: Option<Micros>) -> Result<Plan, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let grade = GradeEdit::of(project.materials.color_adjust_of(segment));
    let id = &segment.material_id;
    let source = if let Some(video) = project.materials.video(id) {
        let range = segment
            .source_range
            .intersect(&TimeRange::new(0, video.duration.max(1)))
            .ok_or("the clip shows none of its file")?;
        let times = match at {
            Some(t) => vec![project.materials.time_map(segment).clamped_source_time(t)],
            None => (0..FRAMES_PER_CLIP)
                .map(|i| {
                    range.start
                        + ((i as f64 + 0.5) / FRAMES_PER_CLIP as f64 * range.duration as f64)
                            as Micros
                })
                .collect(),
        };
        Source::Video {
            path: video.path.clone(),
            times,
        }
    } else if let Some(image) = project.materials.image(id) {
        Source::Image {
            path: image.path.clone(),
        }
    } else {
        return Err("only a video or a photo clip has colours to measure".into());
    };
    Ok(Plan {
        source,
        // A keyframed crop is measured as the clip starts.
        crop: segment.crop_at(segment.target_range.start),
        grade,
    })
}

/// Decode what `plan` names. Blocking: call it from a job thread.
pub fn read(plan: &Plan) -> Result<Picture, String> {
    let mut samples = Samples::default();
    let mut add = |rgba: &[u8], w: usize, h: usize| {
        let (cropped, cw, ch) = crop_rgba(rgba, w, h, plan.crop);
        let step = (cw * ch / PIXELS_PER_FRAME).max(1);
        samples.add_rgba(&cropped, step);
    };
    match &plan.source {
        Source::Video { path, times } => {
            let mut decoder = VideoDecoder::open_scaled(path, HEIGHT)
                .map_err(|e| format!("could not open {path}: {e}"))?;
            for &t in times {
                let frame = decoder
                    .seek_and_decode(t + SAMPLE_SLACK)
                    .map_err(|e| format!("could not decode {path} at {t} µs: {e}"))?;
                add(&frame.data, frame.width as usize, frame.height as usize);
            }
        }
        Source::Image { path } => {
            let image = image::open(path)
                .map_err(|e| format!("could not read {path}: {e}"))?
                .thumbnail(HEIGHT * 2, HEIGHT * 2)
                .to_rgba8();
            let (w, h) = (image.width() as usize, image.height() as usize);
            add(image.as_raw(), w, h);
        }
    }
    if samples.is_empty() {
        return Err("the clip has no pixels to measure".into());
    }
    // A look whose file is gone is drawn without it, so it is measured
    // without it too.
    let lut = plan.grade.lut.as_ref().and_then(|l| {
        std::fs::read_to_string(&l.path)
            .ok()
            .and_then(|text| crate::modules::render::lut::parse(&text).ok())
    });
    Ok(Picture {
        samples,
        grade: plan.grade.clone(),
        lut,
    })
}

/// The part of a frame a crop leaves visible. The colour of what the viewer
/// never sees must not steer the balance.
fn crop_rgba(rgba: &[u8], w: usize, h: usize, crop: Option<Crop>) -> (Vec<u8>, usize, usize) {
    let Some(c) = crop else {
        return (rgba.to_vec(), w, h);
    };
    let x0 = ((c.left.clamp(0.0, 1.0) * w as f32) as usize).min(w.saturating_sub(1));
    let y0 = ((c.top.clamp(0.0, 1.0) * h as f32) as usize).min(h.saturating_sub(1));
    let x1 = ((c.right.clamp(0.0, 1.0) * w as f32).ceil() as usize).clamp(x0 + 1, w);
    let y1 = ((c.bottom.clamp(0.0, 1.0) * h as f32).ceil() as usize).clamp(y0 + 1, h);
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        out.extend_from_slice(&rgba[(y * w + x0) * 4..(y * w + x1) * 4]);
    }
    (out, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crop_keeps_only_the_visible_part() {
        // 4x2: left half red, right half blue.
        let mut rgba = Vec::new();
        for _ in 0..2 {
            for x in 0..4 {
                rgba.extend_from_slice(if x < 2 {
                    &[255, 0, 0, 255]
                } else {
                    &[0, 0, 255, 255]
                });
            }
        }
        let crop = Crop {
            left: 0.5,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        };
        let (out, w, h) = crop_rgba(&rgba, 4, 2, Some(crop));
        assert_eq!((w, h), (2, 2));
        assert!(out.chunks(4).all(|p| p == [0, 0, 255, 255]));
    }
}
