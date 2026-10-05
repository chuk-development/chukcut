//! Putting media into a slot: the time and shape arithmetic, and the edit.
//!
//! The slot decides; the media adapts. A slot keeps its place and length on
//! the timeline, so a template's cuts stay on its music. A longer clip is
//! trimmed (from its start, or from `source_start`); a shorter clip is slowed
//! down until it fills the slot, because a gap would break every cut after
//! it. A picture of another shape is cropped, centred, to the slot's shape:
//! the cropped picture then fits the canvas exactly where the placeholder
//! did, under the slot's own transform (`fx::edit::fill_cell` relies on the
//! same property of the compositor).

use crate::modules::project::document::{
    source_duration_for, Crop, Id, Micros, Project, TimeRange,
};
use crate::modules::timeline::ops::EditCommand;

use super::slot::{self, SlotMarker, SlotMedia};

/// The slowest a short clip is played to fill its slot — the editor's own
/// floor for a speed (`timeline::ops`).
const MIN_SPEED: f32 = 0.01;

/// A picture within this much of the slot's shape is not cropped: a
/// 1078×1920 phone clip in a 9:16 slot loses nothing.
const SAME_SHAPE: f64 = 0.005;

/// What is being put into a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillKind {
    Video,
    Image,
}

/// A material already in the pool, as the fill needs to know it.
#[derive(Debug, Clone, PartialEq)]
pub struct FillMedia {
    pub material_id: String,
    pub kind: FillKind,
    /// Zero for a still.
    pub duration: Micros,
    /// Display size: after the file's rotation.
    pub width: u32,
    pub height: u32,
}

/// How a clip sits in a slot.
#[derive(Debug, Clone, Copy)]
pub struct FillPlan {
    pub source: TimeRange,
    pub speed: f32,
    pub crop: Option<Crop>,
    /// The clip was shorter than the slot and plays slowed down.
    pub slowed: bool,
}

/// The centred crop that gives a `width`×`height` picture the shape
/// `aspect`, or `None` when it has that shape already.
pub fn cover_crop(width: u32, height: u32, aspect: [u32; 2]) -> Option<Crop> {
    if width == 0 || height == 0 {
        return None;
    }
    let source = width as f64 / height as f64;
    let target = slot::ratio(aspect);
    if (source / target - 1.0).abs() < SAME_SHAPE {
        return None;
    }
    if source > target {
        // Too wide: keep the middle of the width.
        let keep = (target / source) as f32;
        let side = (1.0 - keep) * 0.5;
        Some(Crop {
            left: side,
            top: 0.0,
            right: 1.0 - side,
            bottom: 1.0,
        })
    } else {
        let keep = (source / target) as f32;
        let side = (1.0 - keep) * 0.5;
        Some(Crop {
            left: 0.0,
            top: side,
            right: 1.0,
            bottom: 1.0 - side,
        })
    }
}

/// How `media` fills a slot `slot_duration` long with shape `aspect`.
///
/// `source_start` picks where in a longer clip the slot starts reading; it
/// is clamped so the slot never reads past the clip's end. A still reads
/// `[0, slot)`, as every still on the timeline does.
pub fn plan(
    slot_duration: Micros,
    aspect: [u32; 2],
    media: &FillMedia,
    source_start: Option<Micros>,
) -> Result<FillPlan, String> {
    if slot_duration <= 0 {
        return Err("the slot has no length".into());
    }
    let crop = cover_crop(media.width, media.height, aspect);
    if media.kind == FillKind::Image {
        return Ok(FillPlan {
            source: TimeRange::new(0, slot_duration),
            speed: 1.0,
            crop,
            slowed: false,
        });
    }
    let length = media.duration;
    if length <= 0 {
        return Err("the video has no length".into());
    }
    if length >= slot_duration {
        let start = source_start.unwrap_or(0).clamp(0, length - slot_duration);
        return Ok(FillPlan {
            source: TimeRange::new(start, slot_duration),
            speed: 1.0,
            crop,
            slowed: false,
        });
    }

    // Shorter than the slot: slow it down so the whole clip spans the slot.
    // The speed is an f32 and the source length is derived from it the way
    // the document derives it (`source_duration_for`); step the speed down
    // until that length fits inside the file, so rounding never reads a
    // microsecond past the end.
    let mut speed = (length as f64 / slot_duration as f64) as f32;
    if speed < MIN_SPEED {
        return Err(format!(
            "the clip is {:.2} s long and the slot {:.2} s; it would have to play slower than {MIN_SPEED}x",
            length as f64 / 1e6,
            slot_duration as f64 / 1e6
        ));
    }
    while source_duration_for(slot_duration, speed) > length {
        speed = f32::from_bits(speed.to_bits() - 1);
    }
    Ok(FillPlan {
        source: TimeRange::new(0, source_duration_for(slot_duration, speed)),
        speed,
        crop,
        slowed: true,
    })
}

/// The edit that puts `media` into the slot clip `segment_id`, plus the new
/// marker entry the caller must insert into `materials.extras` before
/// applying it (the analysis store's contract: pool first, then the edit).
///
/// The clip keeps its id, place, length, transform, animations, effects,
/// transitions and keyframes; its material, source range, speed and crop are
/// the media's. A speed ramp is dropped, because it was timed to the
/// placeholder. Any picture clip may be filled, slot or not: a clip without
/// a marker gets one, which is how "replace media" works on a plain clip.
///
/// The clip may be on any sequence: a slot that was moved into a compound
/// clip, or one on another timeline, is filled where it is
/// (`sequence::build::inside`), without opening it.
pub fn replace_command(
    project: &Project,
    segment_id: &str,
    media: &FillMedia,
    source_start: Option<Micros>,
) -> Result<((Id, serde_json::Value), FillPlan, EditCommand), String> {
    let (sequence_id, track, before) = crate::modules::sequence::find_segment(project, segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let pool = &project.materials;
    if pool.video(&before.material_id).is_none() && pool.image(&before.material_id).is_none() {
        return Err("only a video or photo clip can take new media".into());
    }
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .ok_or("the clip is no longer on the timeline")?;

    let current = slot::marker_of(project, before);
    let marker = match &current {
        Some((_, marker)) => SlotMarker {
            filled: true,
            ..marker.clone()
        },
        None => SlotMarker {
            index: 1,
            label: None,
            aspect: displayed_aspect(project, before),
            accepts: SlotMedia::Any,
            filled: true,
        },
    };
    match (marker.accepts, media.kind) {
        (SlotMedia::Video, FillKind::Image) => {
            return Err("this slot takes a video, not a photo".into())
        }
        (SlotMedia::Image, FillKind::Video) => {
            return Err("this slot takes a photo, not a video".into())
        }
        _ => {}
    }
    let plan = plan(
        before.target_range.duration,
        marker.aspect,
        media,
        source_start,
    )?;

    let entry = marker.new_entry();
    let mut after = before.clone();
    after.material_id = media.material_id.clone();
    after.source_range = plan.source;
    after.speed = plan.speed;
    after.crop = plan.crop;
    // The slot's fitted crop replaces whatever crop animation the placeholder
    // had; its keyframes were measured against another picture.
    after.keyframes.retain(|t| !t.property.is_crop());
    after.extras.retain(|id| {
        pool.speed_curve(id).is_none() && current.as_ref().is_none_or(|(old, _)| old != id)
    });
    after.extras.push(entry.0.clone());

    let commands = vec![
        EditCommand::RemoveSegment {
            track_id: track.id.clone(),
            segment: before.clone(),
            index,
        },
        EditCommand::InsertSegment {
            track_id: track.id.clone(),
            segment: after,
            index,
        },
    ];
    let command = EditCommand::Composite {
        label: "Replace media".into(),
        commands: crate::modules::sequence::build::inside(project, sequence_id, commands),
    };
    Ok((entry, plan, command))
}

/// The shape a clip's picture has on the canvas: its material's display
/// size, then its crop.
pub fn displayed_aspect(
    project: &Project,
    segment: &crate::modules::project::document::Segment,
) -> [u32; 2] {
    let pool = &project.materials;
    let size = pool
        .video(&segment.material_id)
        .map(|v| {
            // A file rotated a quarter turn is displayed on its side.
            if v.rotation.rem_euclid(180) == 90 {
                (v.height, v.width)
            } else {
                (v.width, v.height)
            }
        })
        .or_else(|| {
            pool.image(&segment.material_id)
                .map(|i| (i.width, i.height))
        })
        .unwrap_or((project.canvas.width, project.canvas.height));
    let (mut w, mut h) = (size.0.max(1) as f64, size.1.max(1) as f64);
    if let Some(crop) = segment.crop {
        w *= (crop.right - crop.left).clamp(0.01, 1.0) as f64;
        h *= (crop.bottom - crop.top).clamp(0.01, 1.0) as f64;
    }
    slot::reduce_aspect(w.round() as u32, h.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(duration: Micros, width: u32, height: u32) -> FillMedia {
        FillMedia {
            material_id: "v".into(),
            kind: FillKind::Video,
            duration,
            width,
            height,
        }
    }

    #[test]
    fn a_longer_clip_is_trimmed_to_the_slot_from_its_start() {
        let p = plan(2_000_000, [9, 16], &video(10_000_000, 1080, 1920), None).unwrap();
        assert_eq!(p.source, TimeRange::new(0, 2_000_000));
        assert_eq!(p.speed, 1.0);
        assert!(p.crop.is_none());
        assert!(!p.slowed);
    }

    #[test]
    fn a_chosen_start_is_kept_but_never_reads_past_the_end() {
        let media = video(5_000_000, 1080, 1920);
        let p = plan(2_000_000, [9, 16], &media, Some(1_500_000)).unwrap();
        assert_eq!(p.source, TimeRange::new(1_500_000, 2_000_000));
        let p = plan(2_000_000, [9, 16], &media, Some(4_900_000)).unwrap();
        assert_eq!(p.source, TimeRange::new(3_000_000, 2_000_000));
        let p = plan(2_000_000, [9, 16], &media, Some(-7)).unwrap();
        assert_eq!(p.source.start, 0);
    }

    #[test]
    fn a_shorter_clip_is_slowed_to_span_the_slot_exactly() {
        // Awkward numbers on purpose: the f32 speed must not round the source
        // past the end of the file.
        for (length, slot) in [
            (1_000_000, 3_000_000),
            (999_999, 1_000_000),
            (1_234_567, 7_654_321),
            (33_366, 2_000_000),
        ] {
            let p = plan(slot, [16, 9], &video(length, 1920, 1080), None).unwrap();
            assert!(p.slowed);
            assert!(p.speed < 1.0);
            assert!(p.source.duration <= length, "{length} in {slot}");
            assert_eq!(p.source.duration, source_duration_for(slot, p.speed));
            // Within a frame of the whole clip.
            assert!(length - p.source.duration < 34_000, "{length} in {slot}");
        }
    }

    #[test]
    fn a_clip_too_short_for_any_speed_is_refused_by_name() {
        let error = plan(60_000_000, [9, 16], &video(100_000, 1080, 1920), None).unwrap_err();
        assert!(error.contains("slower than"), "{error}");
    }

    #[test]
    fn a_still_reads_the_slot_length() {
        let still = FillMedia {
            kind: FillKind::Image,
            duration: 0,
            ..video(0, 4000, 3000)
        };
        let p = plan(1_500_000, [4, 3], &still, None).unwrap();
        assert_eq!(p.source, TimeRange::new(0, 1_500_000));
        assert!(p.crop.is_none());
    }

    #[test]
    fn landscape_into_portrait_keeps_the_middle_of_the_width() {
        let crop = cover_crop(1920, 1080, [9, 16]).unwrap();
        let kept = crop.right - crop.left;
        // 9:16 out of 16:9 is (9/16)/(16/9) of the width.
        assert!((kept - 0.316_406).abs() < 1e-4, "{kept}");
        assert!((crop.left - (1.0 - kept) / 2.0).abs() < 1e-6);
        assert_eq!((crop.top, crop.bottom), (0.0, 1.0));
    }

    #[test]
    fn portrait_into_square_keeps_the_middle_of_the_height() {
        let crop = cover_crop(1080, 1920, [1, 1]).unwrap();
        assert_eq!((crop.left, crop.right), (0.0, 1.0));
        assert!((crop.bottom - crop.top - 0.5625).abs() < 1e-4);
    }

    #[test]
    fn the_same_shape_within_half_a_percent_is_not_cropped() {
        assert!(cover_crop(1078, 1920, [9, 16]).is_none());
        assert!(cover_crop(1000, 1920, [9, 16]).is_some());
    }
}
