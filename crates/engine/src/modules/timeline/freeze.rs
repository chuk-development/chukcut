//! Freeze frame: hold the picture at the playhead for a while.
//!
//! The edit has two halves with different natures, and they are kept apart on
//! purpose:
//!
//! - [`extract_frame`] is the impure half. It decodes the clip's frame at the
//!   playhead from the original file and writes it as a PNG under
//!   `workspace::paths::freeze_frames_dir`. It is slow (a seek and a decode)
//!   and runs without the project lock.
//! - [`freeze_frame_edit`] is the pure half. Given the document and the still,
//!   it builds **one** `Composite`: add the still to the pool, split the clip
//!   at the playhead, move everything after the cut right by the still's
//!   length, and put the still in the gap. One undo step takes all of it back.
//!
//! The edit is built the way `silence::cut` builds its edit: every step is
//! applied to a copy as it is recorded, so each step is computed against the
//! document as it will be when the real apply reaches it. `split_at` is reused
//! rather than restated, so a linked picture-and-sound pair is cut together and
//! keyframes and transitions follow the split rules.
//!
//! ## Which lanes ripple
//!
//! The clip's lane and every lane linked to it, closed over links — the rule
//! `silence::cut::rippled_lanes` documents. The sound of a linked clip moves
//! with its picture and leaves a gap under the still; music on another lane
//! stays where the user put it.

use std::path::{Path, PathBuf};

use super::ops::{split_at, EditCommand, PoolMaterial};
use crate::modules::project::document::{
    new_id, ImageMaterial, Micros, Project, Segment, TimeRange, TrackKind,
};
use crate::modules::render::layout::animated_transform;

/// How long a freeze frame holds when the caller does not say: three seconds.
pub const DEFAULT_FREEZE: Micros = 3_000_000;

/// What the decode step needs from the document, read under the lock and then
/// released.
#[derive(Debug, Clone)]
pub struct FreezeSource {
    /// The original file, never a proxy: the still is shown at full size.
    pub path: String,
    /// Source time of the frame on screen at the playhead.
    pub source_time: Micros,
}

/// Check that `segment_id` can be frozen at `at` and say which frame to decode.
pub fn freeze_source(
    project: &Project,
    segment_id: &str,
    at: Micros,
) -> Result<FreezeSource, String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    if track.kind != TrackKind::Video {
        return Err("only a video clip can be frozen".into());
    }
    if track.locked {
        return Err("the clip's track is locked".into());
    }
    let video = project
        .materials
        .video(&segment.material_id)
        .ok_or("only a video clip can be frozen")?;
    let source_time = segment
        .source_time_at(at)
        .ok_or("the playhead is not over the clip")?;
    Ok(FreezeSource {
        path: video.path.clone(),
        source_time,
    })
}

/// Decode the frame of `source` and write it to `output` as a PNG.
///
/// Software decode on purpose: this is one frame, and a hardware decoder
/// would have to be opened, warmed and downloaded from for it. The frame comes
/// out with the file's rotation applied, which is the size the compositor
/// fits a video by, so the still placed with the clip's transform covers
/// exactly what the clip did.
pub fn extract_frame(source: &FreezeSource, output: &Path) -> Result<ImageMaterial, String> {
    let mut decoder = crate::modules::media::VideoDecoder::open(&source.path)
        .map_err(|e| format!("cannot open {}: {e}", source.path))?;
    let frame = decoder
        .seek_and_decode(source.source_time)
        .map_err(|e| format!("cannot decode the frame: {e}"))?;
    let png = crate::modules::export::snapshot::encode_png(frame.data, frame.width, frame.height)?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(output, png).map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    Ok(ImageMaterial {
        id: new_id(),
        path: output.to_string_lossy().into_owned(),
        width: frame.width,
        height: frame.height,
    })
}

/// Where a new freeze frame's PNG goes. A fresh name every time: undo leaves
/// the file behind so that redo still has it, and two freezes of one frame
/// are two materials anyway.
pub fn freeze_output_path() -> PathBuf {
    crate::modules::workspace::paths::freeze_frames_dir().join(format!("{}.png", new_id()))
}

/// The one edit that freezes `segment_id` at `at` for `duration`, showing
/// `image` in the gap.
///
/// Pure: reads the document, returns the command. `image` is the still the
/// caller decoded (or, in a test, made up); it goes into the pool as part of
/// the same composite, so undo takes it out again.
pub fn freeze_frame_edit(
    project: &Project,
    segment_id: &str,
    at: Micros,
    duration: Micros,
    image: ImageMaterial,
) -> Result<EditCommand, String> {
    if duration <= 0 {
        return Err("a freeze frame needs a length".into());
    }
    freeze_source(project, segment_id, at)?;
    let (track, segment) = project.segment(segment_id).expect("checked above");
    let lane = track.id.clone();
    let still = still_segment(project, segment, at, duration, &image);

    let mut sim = project.clone();
    let mut commands = Vec::new();
    let mut record = |sim: &mut Project, command: EditCommand| -> Result<(), String> {
        command.apply(sim)?;
        commands.push(command);
        Ok(())
    };

    // 1. The still into the pool, at the end of the images.
    let pool_index = sim.materials.images.len();
    record(
        &mut sim,
        EditCommand::AddMaterial {
            index: pool_index,
            material: PoolMaterial::Image(image),
        },
    )?;

    // 2. Cut the clip (and its linked partners) at the playhead. At the clip's
    //    first instant there is nothing to cut: the still goes before it.
    if at > segment.target_range.start {
        let split = split_at(&sim, segment_id, at)?;
        record(&mut sim, split)?;
    }

    // 3. Everything from the cut on, on every lane that has to stay in step
    //    with the clip, moves right by the still's length. Right to left, so
    //    no clip ever lands on a neighbour that has not moved yet.
    let lanes = crate::modules::silence::cut::rippled_lanes(&sim, segment_id, at);
    let mut moves: Vec<(Micros, String, String)> = sim
        .tracks
        .iter()
        .filter(|t| lanes.contains(&t.id))
        .flat_map(|t| {
            t.segments
                .iter()
                .filter(|s| s.target_range.start >= at)
                .map(|s| (s.target_range.start, t.id.clone(), s.id.clone()))
        })
        .collect();
    moves.sort_by(|a, b| b.cmp(a));
    for (from_start, track_id, id) in moves {
        let to_start = from_start
            .checked_add(duration)
            .ok_or("the freeze frame would push clips past the end of time")?;
        record(
            &mut sim,
            EditCommand::MoveSegment {
                segment_id: id,
                from_track: track_id.clone(),
                to_track: track_id,
                from_start,
                to_start,
            },
        )?;
    }

    // 4. The still into the gap.
    let index = sim
        .track(&lane)
        .map(|t| t.segments.partition_point(|s| s.target_range.start < at))
        .unwrap_or(0);
    record(
        &mut sim,
        EditCommand::InsertSegment {
            track_id: lane,
            segment: still,
            index,
        },
    )?;

    Ok(EditCommand::Composite {
        label: "Freeze frame".into(),
        commands,
    })
}

/// The still's segment: the clip's placement as it is at `at`, frozen.
///
/// The transform is the *animated* one at the playhead and the still carries
/// no keyframes: a freeze frame holds the picture, so a zoom running through
/// the cut stops where it was rather than replaying from the clip's start.
/// The crop is the clip's. Colour and effect extras are shared, as a split
/// shares them; transitions, link groups and animations are not, for the
/// reasons `split_one` gives for the right half.
fn still_segment(
    project: &Project,
    clip: &Segment,
    at: Micros,
    duration: Micros,
    image: &ImageMaterial,
) -> Segment {
    let mut transform = animated_transform(clip, at);
    transform.opacity = transform.opacity.clamp(0.0, 1.0);
    let mut extras = clip.extras.clone();
    extras.retain(|id| {
        project.materials.transition(id).is_none()
            && !project.materials.links.contains(id)
            && project.materials.animation(id).is_none()
            && project.materials.follow(id).is_none()
    });
    Segment {
        id: new_id(),
        material_id: image.id.clone(),
        target_range: TimeRange::new(at, duration),
        source_range: TimeRange::new(0, duration),
        render_index: clip.render_index,
        speed: 1.0,
        volume: 1.0,
        transform,
        crop: clip.crop,
        extras,
        keyframes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AnimatableProperty, AudioMaterial, CanvasConfig, Crop, Easing, Keyframe, KeyframeTrack,
        Severity, Track, Transform, VideoMaterial, MICROS_PER_SECOND,
    };
    use crate::modules::timeline::History;

    const S: Micros = MICROS_PER_SECOND;

    fn seg(id: &str, material: &str, start: Micros, duration: Micros) -> Segment {
        Segment {
            id: id.into(),
            material_id: material.into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// A take with its sound linked (V1 + A1, 0–10 s), a second linked take
    /// after it (10–15 s), and music on A2 that must not move.
    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "take".into(),
            path: "/nonexistent/take.mp4".into(),
            width: 1080,
            height: 1920,
            duration: 60 * S,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        p.materials.audios.push(AudioMaterial {
            id: "music".into(),
            path: "/nonexistent/music.mp3".into(),
            duration: 60 * S,
            sample_rate: 48_000,
            channels: 2,
        });
        p.materials.links.insert("g1".into());
        p.materials.links.insert("g2".into());

        let mut v = Track::new(TrackKind::Video, "V1");
        let mut a = Track::new(TrackKind::Audio, "A1");
        let mut take = seg("take-v", "take", 0, 10 * S);
        take.extras.push("g1".into());
        take.transform.scale = [1.5, 1.5];
        take.transform.position = [0.1, -0.2];
        take.crop = Some(Crop {
            left: 0.1,
            top: 0.0,
            right: 0.9,
            bottom: 1.0,
        });
        take.keyframes.push(KeyframeTrack {
            property: AnimatableProperty::Rotation,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: 10 * S,
                    value: 10.0,
                    easing: Easing::Linear,
                },
            ],
        });
        let mut take_a = seg("take-a", "take", 0, 10 * S);
        take_a.extras.push("g1".into());
        let mut next = seg("next-v", "take", 10 * S, 5 * S);
        next.source_range = TimeRange::new(20 * S, 5 * S);
        next.extras.push("g2".into());
        let mut next_a = next.clone();
        next_a.id = "next-a".into();
        v.segments = vec![take, next];
        a.segments = vec![take_a, next_a];
        let mut m = Track::new(TrackKind::Audio, "A2");
        m.segments = vec![seg("music", "music", 0, 20 * S)];
        p.tracks = vec![v, a, m];
        // The render order every edit leaves behind (one index per lane), so
        // that "undo is exact" compares like with like.
        for (index, track) in p.tracks.iter_mut().enumerate() {
            for segment in &mut track.segments {
                segment.render_index = index as i32;
            }
        }
        p
    }

    fn still() -> ImageMaterial {
        ImageMaterial {
            id: "still".into(),
            path: "/nonexistent/still.png".into(),
            width: 1080,
            height: 1920,
        }
    }

    fn json(p: &Project) -> String {
        serde_json::to_string(p).unwrap()
    }

    fn starts(p: &Project, lane: usize) -> Vec<(String, Micros, Micros)> {
        p.tracks[lane]
            .segments
            .iter()
            .map(|s| {
                (
                    s.material_id.clone(),
                    s.target_range.start,
                    s.target_range.duration,
                )
            })
            .collect()
    }

    #[test]
    fn freezing_holds_the_frame_and_pushes_the_rest_right_in_one_undo_step() {
        let mut p = project();
        let before = json(&p);
        let command = freeze_frame_edit(&p, "take-v", 4 * S, DEFAULT_FREEZE, still()).unwrap();
        let mut history = History::new();
        history.apply(&mut p, command).unwrap();

        let errors: Vec<_> = p
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");

        // Picture: left half, the still, right half and the next take, 3 s on.
        assert_eq!(
            starts(&p, 0),
            vec![
                ("take".into(), 0, 4 * S),
                ("still".into(), 4 * S, 3 * S),
                ("take".into(), 7 * S, 6 * S),
                ("take".into(), 13 * S, 5 * S),
            ]
        );
        // Sound: cut with the picture and moved with it, a gap under the still.
        assert_eq!(
            starts(&p, 1),
            vec![
                ("take".into(), 0, 4 * S),
                ("take".into(), 7 * S, 6 * S),
                ("take".into(), 13 * S, 5 * S),
            ]
        );
        // Music is on no linked lane and stays put.
        assert_eq!(starts(&p, 2), vec![("music".into(), 0, 20 * S)]);
        // The right halves keep reading the source where the cut was.
        assert_eq!(p.tracks[0].segments[2].source_range.start, 4 * S);
        assert_eq!(p.materials.images.len(), 1);

        // The still looks like the clip did at the playhead.
        let held = &p.tracks[0].segments[1];
        assert_eq!(held.transform.scale, [1.5, 1.5]);
        assert_eq!(held.transform.position, [0.1, -0.2]);
        assert!((held.transform.rotation - 4.0).abs() < 1e-3);
        assert!(held.keyframes.is_empty());
        let crop = held.crop.unwrap();
        assert_eq!((crop.left, crop.right), (0.1, 0.9));
        assert!(p.link_group_of(&held.id).is_none());

        history.undo(&mut p).unwrap();
        assert_eq!(json(&p), before);
    }

    #[test]
    fn freezing_at_the_first_instant_puts_the_still_before_the_clip() {
        let mut p = project();
        let before = json(&p);
        let command = freeze_frame_edit(&p, "next-v", 10 * S, S, still()).unwrap();
        let mut history = History::new();
        history.apply(&mut p, command).unwrap();
        assert_eq!(
            starts(&p, 0),
            vec![
                ("take".into(), 0, 10 * S),
                ("still".into(), 10 * S, S),
                ("take".into(), 11 * S, 5 * S),
            ]
        );
        assert_eq!(starts(&p, 1)[1], ("take".into(), 11 * S, 5 * S));
        history.undo(&mut p).unwrap();
        assert_eq!(json(&p), before);
    }

    #[test]
    fn only_a_video_clip_under_the_playhead_freezes() {
        let p = project();
        assert!(freeze_frame_edit(&p, "take-a", 4 * S, S, still()).is_err());
        assert!(freeze_frame_edit(&p, "music", 4 * S, S, still()).is_err());
        assert!(freeze_frame_edit(&p, "take-v", 12 * S, S, still()).is_err());
        assert!(freeze_frame_edit(&p, "take-v", 4 * S, 0, still()).is_err());
    }
}
