//! The edits analysis results drive: split at scene changes, cut to the beat,
//! snap cuts to beats, reframe. Each builds **one** `EditCommand` — one undo
//! step — out of the ordinary primitives, against a scratch copy of the
//! document so every part is built on the state the part before it leaves.

use super::reframe::{Axis, PathPoint};
use super::store::{self, timeline_time_of, Beats, SceneCuts};
use crate::modules::project::document::{
    AnimatableProperty, Easing, Id, Keyframe, Micros, Project, Segment, TimeRange, TrackKind,
};
use crate::modules::render::layout::{crop_extent, crop_uv, fit_size};
use crate::modules::timeline::ops::{mirror_linked_edits, split_at, EditCommand};

/// How close to a clip's edge a cut may land, in frames: a split one frame
/// from the edge leaves a sliver nobody wants.
const EDGE_FRAMES: f64 = 2.0;

fn frame(project: &Project) -> Micros {
    let fps = if project.fps.is_finite() && project.fps > 0.0 {
        project.fps
    } else {
        30.0
    };
    (1e6 / fps).round() as Micros
}

/// Split `segment_id` at each of `times` (timeline), as one command. Times
/// too close to an edge or to each other are dropped; none left is an error.
pub fn split_at_times(
    project: &Project,
    segment_id: &str,
    times: &[Micros],
    label: &str,
) -> Result<EditCommand, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let margin = (frame(project) as f64 * EDGE_FRAMES) as Micros;
    let range = segment.target_range;
    let mut cuts: Vec<Micros> = times
        .iter()
        .copied()
        .filter(|&t| t >= range.start + margin && t <= range.end() - margin)
        .collect();
    cuts.sort_unstable();
    cuts.dedup_by(|b, a| *b - *a < margin);
    if cuts.is_empty() {
        return Err("there is nowhere inside the clip to cut".into());
    }
    // Latest first: each split leaves the original id on the left half, so
    // every cut is aimed at the same clip.
    let mut scratch = project.clone();
    let mut commands = Vec::with_capacity(cuts.len());
    for &t in cuts.iter().rev() {
        let command = split_at(&scratch, segment_id, t)?;
        command.apply(&mut scratch)?;
        commands.push(command);
    }
    Ok(EditCommand::Composite {
        label: label.into(),
        commands,
    })
}

/// The scene cuts `segment` shows, in timeline time on the project's frame
/// grid.
pub fn scene_cut_times(project: &Project, segment: &Segment) -> Vec<Micros> {
    let Some((_, cuts)) = store::entry_of::<SceneCuts>(project, segment) else {
        return Vec::new();
    };
    let map = project.materials.time_map(segment);
    cuts.cuts
        .iter()
        .filter_map(|&t| timeline_time_of(&map, t))
        .map(|t| store::snap_to_frame(t, project.fps))
        .collect()
}

/// The beats `segment` carries, in timeline time, inside the clip.
pub fn segment_beats(project: &Project, segment: &Segment) -> Vec<Micros> {
    let Some((_, beats)) = store::entry_of::<Beats>(project, segment) else {
        return Vec::new();
    };
    let map = project.materials.time_map(segment);
    beats
        .beats
        .iter()
        .filter_map(|&t| timeline_time_of(&map, t))
        .collect()
}

/// Every beat on the timeline, from every clip that has beats, ascending.
pub fn timeline_beats(project: &Project) -> Vec<Micros> {
    let mut all: Vec<Micros> = project
        .tracks
        .iter()
        .filter(|t| !t.muted)
        .flat_map(|t| t.segments.iter())
        .flat_map(|s| segment_beats(project, s))
        .collect();
    all.sort_unstable();
    all.dedup();
    all
}

/// Split each of `segment_ids` on the beats, keeping every `every`-th beat,
/// as one undo step. Sound clips are left to their pictures: a linked sound
/// is split with its picture, and a music clip is what the beats came from.
pub fn auto_cut_to_beat(
    project: &Project,
    segment_ids: &[Id],
    every: usize,
) -> Result<EditCommand, String> {
    let beats = timeline_beats(project);
    if beats.is_empty() {
        return Err("detect the beats of a music clip first".into());
    }
    let every = every.max(1);
    let grid: Vec<Micros> = beats
        .iter()
        .step_by(every)
        .map(|&t| store::snap_to_frame(t, project.fps))
        .collect();
    let mut scratch = project.clone();
    let mut commands = Vec::new();
    for id in segment_ids {
        let Some((track, _)) = scratch.segment(id) else {
            continue;
        };
        if track.kind != TrackKind::Video || track.locked {
            continue;
        }
        let Ok(command) = split_at_times(&scratch, id, &grid, "Auto-cut to beat") else {
            continue;
        };
        command.apply(&mut scratch)?;
        commands.push(command);
    }
    if commands.is_empty() {
        return Err("no beat falls inside the selected video clips".into());
    }
    Ok(EditCommand::Composite {
        label: "Auto-cut to beat".into(),
        commands,
    })
}

/// How long `material_id` is, or `None` for a still that lasts forever.
fn material_length(project: &Project, material_id: &str) -> Option<Micros> {
    let pool = &project.materials;
    pool.video(material_id)
        .map(|v| v.duration)
        .or_else(|| pool.audio(material_id).map(|a| a.duration))
}

/// The trim that moves `segment`'s head (`head == true`) or tail to `to`,
/// keeping the other edge, or `None` when the file has no frames to show
/// there or the clip would vanish.
fn edge_trim(project: &Project, segment: &Segment, head: bool, to: Micros) -> Option<EditCommand> {
    let (target, source) = (segment.target_range, segment.source_range);
    let min = frame(project);
    let after_target = if head {
        TimeRange::new(to, target.end() - to)
    } else {
        TimeRange::new(target.start, to - target.start)
    };
    // Through the time map, as every trim does: at constant speed this is
    // `source_duration_for` on the moved edge, on a speed curve it is the part
    // of the file the curve carries the new edges to.
    let after_source = project
        .materials
        .time_map(segment)
        .retimed_source(after_target);
    if after_target.duration < min || after_source.start < 0 || after_target.start < 0 {
        return None;
    }
    if let Some(length) = material_length(project, &segment.material_id) {
        if after_source.end() > length {
            return None;
        }
    }
    Some(EditCommand::TrimSegment {
        segment_id: segment.id.clone(),
        before_target: target,
        before_source: source,
        after_target,
        after_source,
    })
}

/// Move every cut between two of `segment_ids` — or between one of them and
/// the clip it butts against — onto the nearest beat within `tolerance`, by
/// rolling the cut: the clip before it gets longer by what the clip after it
/// loses, so nothing else on the timeline moves. One undo step.
pub fn snap_cuts_to_beats(
    project: &Project,
    segment_ids: &[Id],
    tolerance: Micros,
) -> Result<EditCommand, String> {
    let beats = timeline_beats(project);
    if beats.is_empty() {
        return Err("detect the beats of a music clip first".into());
    }
    // Every cut a selected clip touches, once: (track, time, left, right).
    let mut cuts: Vec<(Id, Micros, Id, Id)> = Vec::new();
    for id in segment_ids {
        let Some((track, segment)) = project.segment(id) else {
            continue;
        };
        if track.locked || track.kind == TrackKind::Audio {
            continue;
        }
        for other in &track.segments {
            let (left, right) = if other.target_range.end() == segment.target_range.start {
                (other, segment)
            } else if segment.target_range.end() == other.target_range.start {
                (segment, other)
            } else {
                continue;
            };
            let at = right.target_range.start;
            if !cuts.iter().any(|c| c.0 == track.id && c.1 == at) {
                cuts.push((track.id.clone(), at, left.id.clone(), right.id.clone()));
            }
        }
    }
    if cuts.is_empty() {
        return Err("the selected clips have no cuts between them".into());
    }
    let mut scratch = project.clone();
    let mut commands = Vec::new();
    let mut moved = 0usize;
    for (_, at, left_id, right_id) in cuts {
        let nearest = beats
            .iter()
            .copied()
            .min_by_key(|b| (b - at).abs())
            .map(|b| store::snap_to_frame(b, project.fps));
        let Some(beat) = nearest.filter(|b| (b - at).abs() <= tolerance && *b != at) else {
            continue;
        };
        let (Some((_, left)), Some((_, right))) =
            (scratch.segment(&left_id), scratch.segment(&right_id))
        else {
            continue;
        };
        let (Some(tail), Some(head)) = (
            edge_trim(&scratch, left, false, beat),
            edge_trim(&scratch, right, true, beat),
        ) else {
            continue;
        };
        // Shrink before growing: the two clips may not overlap even between
        // the halves of one roll.
        let order = if beat > at {
            [head, tail]
        } else {
            [tail, head]
        };
        let mut part = Vec::new();
        let mut trial = scratch.clone();
        let mut ok = true;
        for command in order {
            let command = mirror_linked_edits(&trial, command);
            if command.apply(&mut trial).is_err() {
                ok = false;
                break;
            }
            part.push(command);
        }
        if ok {
            scratch = trial;
            commands.extend(part);
            moved += 1;
        }
    }
    if moved == 0 {
        return Err("no cut is close enough to a beat to move".into());
    }
    Ok(EditCommand::Composite {
        label: "Snap cuts to beats".into(),
        commands,
    })
}

/// Displayed size of the picture `segment` shows, after its crop.
fn cropped_size(project: &Project, segment: &Segment) -> Option<(u32, u32)> {
    let pool = &project.materials;
    let (w, h) = if let Some(video) = pool.video(&segment.material_id) {
        if video.rotation.rem_euclid(180) == 90 {
            (video.height, video.width)
        } else {
            (video.width, video.height)
        }
    } else if pool.sequence(&segment.material_id).is_some() {
        sequence_content(project, &segment.material_id, 0)
            .unwrap_or((project.canvas.width, project.canvas.height))
    } else {
        let image = pool.image(&segment.material_id)?;
        (image.width, image.height)
    };
    let (cw, ch) = crop_extent(crop_uv(segment.crop)?);
    Some((
        ((w.max(1) as f32 * cw).max(1.0)) as u32,
        ((h.max(1) as f32 * ch).max(1.0)) as u32,
    ))
}

/// The shape a compound clip's contents have: the largest picture inside
/// that fills its frame the plain way, through compound clips inside. A
/// compound clip is drawn as its sequence rendered on the whole canvas, so
/// the canvas says nothing about its contents; a 16:9 shot inside it stays a
/// 16:9 picture, letterboxed, on a 9:16 canvas. `None` when nothing inside
/// fills its frame (only picture-in-picture, titles).
pub fn content_size(project: &Project, segment: &Segment) -> Option<(u32, u32)> {
    project.materials.sequence(&segment.material_id)?;
    sequence_content(project, &segment.material_id, 0)
}

fn sequence_content(project: &Project, id: &str, depth: usize) -> Option<(u32, u32)> {
    if depth >= crate::modules::sequence::MAX_DEPTH {
        return None;
    }
    let tracks = crate::modules::sequence::tracks_of(project, id)?;
    let area = |(w, h): (u32, u32)| w as u64 * h as u64;
    tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Video && !t.hidden)
        .flat_map(|t| t.segments.iter())
        .filter(|s| is_full_frame(s) && !project.materials.is_effect_clip(s))
        .filter_map(|s| {
            if project.materials.sequence(&s.material_id).is_some() {
                sequence_content(project, &s.material_id, depth + 1)
            } else {
                cropped_size(project, s)
            }
        })
        .max_by_key(|&size| area(size))
}

/// Width over height of the picture `segment` shows, after its crop.
pub fn picture_aspect(project: &Project, segment: &Segment) -> Option<f32> {
    let (w, h) = cropped_size(project, segment)?;
    Some(w as f32 / h.max(1) as f32)
}

/// The edit that fills the canvas with `segment` and moves it along `path`
/// (source time, window centres as fractions of the *uncropped* frame along
/// `axis`), replacing any position and scale animation it had. With an empty
/// path the clip is filled and centred. `project` must already have the
/// canvas the clip is reframed for.
pub fn reframe_command(
    project: &Project,
    segment_id: &str,
    axis: Axis,
    path: &[PathPoint],
) -> Result<EditCommand, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let canvas = (project.canvas.width, project.canvas.height);
    let picture = cropped_size(project, segment).ok_or("only a picture can be reframed")?;
    let (fw, fh) = fit_size(canvas, picture);
    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
    let cover = (cw / fw).max(ch / fh);
    let (qw, qh) = (fw * cover, fh * cover);
    let crop = crop_uv(segment.crop).unwrap_or([0.0, 0.0, 1.0, 1.0]);

    let before = segment.transform;
    let mut after = before;
    after.scale = [cover, cover];
    after.position = [0.0, 0.0];

    // Window centre (uncropped fraction) → position in canvas half-units.
    let position = |centre: f32| -> f32 {
        match axis {
            Axis::Horizontal => {
                let mut u = (centre - crop[0]) / (crop[2] - crop[0]).max(1e-6);
                if before.flip_h {
                    u = 1.0 - u;
                }
                let px =
                    ((0.5 - u) * qw).clamp(-(qw - cw).max(0.0) * 0.5, (qw - cw).max(0.0) * 0.5);
                px / (cw * 0.5)
            }
            Axis::Vertical => {
                let mut v = (centre - crop[1]) / (crop[3] - crop[1]).max(1e-6);
                if before.flip_v {
                    v = 1.0 - v;
                }
                let py =
                    ((v - 0.5) * qh).clamp(-(qh - ch).max(0.0) * 0.5, (qh - ch).max(0.0) * 0.5);
                py / (ch * 0.5)
            }
        }
    };
    let property = match axis {
        Axis::Horizontal => AnimatableProperty::PositionX,
        Axis::Vertical => AnimatableProperty::PositionY,
    };

    let map = project.materials.time_map(segment);
    let mut keys: Vec<Keyframe> = Vec::new();
    for (i, point) in path.iter().enumerate() {
        let Some(at) = timeline_time_of(&map, point.t) else {
            continue;
        };
        let time = at - segment.target_range.start;
        // The window holds until the next shot starts.
        let next_is_cut = path.get(i + 1).is_some_and(|p| p.cut);
        let key = Keyframe {
            time,
            value: (position(point.centre) * 10_000.0).round() / 10_000.0,
            easing: if next_is_cut {
                Easing::Hold
            } else {
                Easing::EaseInOut
            },
        };
        if keys.last().is_some_and(|k| k.time == key.time) {
            keys.pop();
        }
        keys.push(key);
    }
    match keys.len() {
        0 => {}
        // One key is a still window: no animation, just the position.
        1 => {
            match axis {
                Axis::Horizontal => after.position[0] = keys[0].value,
                Axis::Vertical => after.position[1] = keys[0].value,
            }
            keys.clear();
        }
        _ => match axis {
            Axis::Horizontal => after.position[0] = keys[0].value,
            Axis::Vertical => after.position[1] = keys[0].value,
        },
    }

    let mut commands = Vec::new();
    for track in &segment.keyframes {
        if matches!(
            track.property,
            AnimatableProperty::PositionX
                | AnimatableProperty::PositionY
                | AnimatableProperty::ScaleX
                | AnimatableProperty::ScaleY
        ) {
            for keyframe in &track.keyframes {
                commands.push(EditCommand::RemoveKeyframe {
                    segment_id: segment.id.clone(),
                    property: track.property,
                    keyframe: *keyframe,
                });
            }
        }
    }
    commands.push(EditCommand::SetTransform {
        segment_id: segment.id.clone(),
        before,
        after,
    });
    for keyframe in keys {
        commands.push(EditCommand::AddKeyframe {
            segment_id: segment.id.clone(),
            property,
            keyframe,
        });
    }
    Ok(EditCommand::Composite {
        label: "Auto reframe".into(),
        commands,
    })
}

/// A canvas of the shape `ratio` (width, height) with the long edge of
/// `canvas`, both sides even.
pub fn canvas_for_ratio(canvas: (u32, u32), ratio: (u32, u32)) -> (u32, u32) {
    let long = canvas.0.max(canvas.1).max(2) as f64;
    let (rw, rh) = (ratio.0.max(1) as f64, ratio.1.max(1) as f64);
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    if rw >= rh {
        (even(long), even(long * rh / rw))
    } else {
        (even(long * rw / rh), even(long))
    }
}

/// Whether `segment` fills its frame the plain way — no scale, no offset, no
/// turn — so reframing a whole project may take it over. A picture-in-picture
/// someone placed by hand is left alone.
pub fn is_full_frame(segment: &Segment) -> bool {
    let t = segment.transform;
    (t.scale[0] - 1.0).abs() < 1e-3
        && (t.scale[1] - 1.0).abs() < 1e-3
        && t.position[0].abs() < 1e-3
        && t.position[1].abs() < 1e-3
        && t.rotation.abs() < 1e-3
        && !segment.keyframes.iter().any(|k| {
            matches!(
                k.property,
                AnimatableProperty::PositionX
                    | AnimatableProperty::PositionY
                    | AnimatableProperty::ScaleX
                    | AnimatableProperty::ScaleY
            )
        })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Track, Transform, VideoMaterial};

    pub(crate) fn clip(id: &str, start: Micros, duration: Micros, source: Micros) -> Segment {
        Segment {
            id: id.into(),
            material_id: "v".into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(source, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    pub(crate) fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.canvas.width = 1920;
        p.canvas.height = 1080;
        p.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/nonexistent.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 20_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V");
        track.segments.push(clip("a", 0, 4_000_000, 1_000_000));
        track
            .segments
            .push(clip("b", 4_000_000, 4_000_000, 8_000_000));
        p.tracks.push(track);
        p
    }

    fn with_beats(p: &mut Project, beats: Vec<Micros>) {
        let mut music = Track::new(TrackKind::Audio, "M");
        let mut s = clip("m", 0, 10_000_000, 0);
        let (id, value) = store::new_entry(&Beats {
            media_id: "v".into(),
            analysed: TimeRange::new(0, 10_000_000),
            beats,
            bpm: 120.0,
        });
        p.materials.extras.insert(id.clone(), value);
        s.extras.push(id);
        music.segments.push(s);
        p.tracks.push(music);
    }

    #[test]
    fn split_at_times_is_one_step_that_undoes() {
        let p = project();
        let command = split_at_times(&p, "a", &[1_000_000, 2_000_000, 3_990_000], "Split").unwrap();
        let mut q = p.clone();
        command.apply(&mut q).unwrap();
        // The cut near the edge is dropped: three pieces plus "b".
        assert_eq!(q.tracks[0].segments.len(), 4);
        let starts: Vec<Micros> = q.tracks[0]
            .segments
            .iter()
            .map(|s| s.target_range.start)
            .collect();
        assert_eq!(starts, vec![0, 1_000_000, 2_000_000, 4_000_000]);
        // The second piece shows the file from one second further in.
        assert_eq!(q.tracks[0].segments[1].source_range.start, 2_000_000);
        command.invert().apply(&mut q).unwrap();
        assert_eq!(q.tracks[0].segments.len(), 2);
        assert_eq!(q.tracks[0].segments[0].target_range.duration, 4_000_000);
    }

    #[test]
    fn auto_cut_splits_the_video_on_every_other_beat() {
        let mut p = project();
        with_beats(&mut p, (1..16).map(|i| i * 500_000).collect());
        let command = auto_cut_to_beat(&p, &["a".into()], 2).unwrap();
        let mut q = p.clone();
        command.apply(&mut q).unwrap();
        let starts: Vec<Micros> = q.tracks[0]
            .segments
            .iter()
            .map(|s| s.target_range.start)
            .collect();
        assert_eq!(
            starts,
            vec![0, 500_000, 1_500_000, 2_500_000, 3_500_000, 4_000_000]
        );
        // The music is not cut.
        assert_eq!(q.tracks[1].segments.len(), 1);
    }

    #[test]
    fn snapping_rolls_the_cut_onto_the_beat() {
        let mut p = project();
        with_beats(&mut p, vec![3_800_000, 6_000_000]);
        let command = snap_cuts_to_beats(&p, &["a".into(), "b".into()], 400_000).unwrap();
        let mut q = p.clone();
        command.apply(&mut q).unwrap();
        let (a, b) = (&q.tracks[0].segments[0], &q.tracks[0].segments[1]);
        let beat = store::snap_to_frame(3_800_000, 30.0);
        assert_eq!(a.target_range.end(), beat);
        assert_eq!(b.target_range.start, beat);
        // The roll keeps the rest of the timeline where it was.
        assert_eq!(b.target_range.end(), 8_000_000);
        assert_eq!(b.source_range.start, 8_000_000 - (4_000_000 - beat));
        command.invert().apply(&mut q).unwrap();
        assert_eq!(q.tracks[0].segments[0].target_range.end(), 4_000_000);
    }

    /// Clip "a" played through a speed curve, its `speed` field left at 1 —
    /// what the legacy mapping read — with scene cuts and beats on it.
    fn curved(points: Vec<crate::modules::project::SpeedPoint>, length: Micros) -> Project {
        use crate::modules::project::speed::{curve_target_duration, SpeedCurveMaterial};
        let mut p = project();
        let source = TimeRange::new(1_000_000, length);
        p.materials.speed_curves.push(SpeedCurveMaterial {
            id: "curve".into(),
            preset: None,
            points: points.clone(),
        });
        let duration = curve_target_duration(&points, source);
        let cuts = store::new_entry(&SceneCuts {
            media_id: "v".into(),
            analysed: source,
            cuts: vec![3_000_000, 5_000_000, 7_000_000],
            sensitivity: 0.5,
        });
        let beats = store::new_entry(&Beats {
            media_id: "v".into(),
            analysed: source,
            beats: vec![2_000_000, 4_000_000, 8_000_000],
            bpm: 120.0,
        });
        let a = &mut p.tracks[0].segments[0];
        a.source_range = source;
        a.target_range = TimeRange::new(0, duration);
        a.extras = vec!["curve".into(), cuts.0.clone(), beats.0.clone()];
        p.materials.extras.insert(cuts.0, cuts.1);
        p.materials.extras.insert(beats.0, beats.1);
        // Clip "b" butts against the curved clip's new end.
        p.tracks[0].segments[1].target_range.start = duration;
        p
    }

    #[test]
    fn scene_cuts_and_beats_follow_a_speed_curve() {
        use crate::modules::project::SpeedPoint;
        // Twice real time all the way: source 1 s..9 s plays in 4 s, so
        // source instant s is on screen at (s - 1 s) / 2. The constant
        // `speed` of 1 would have said s - 1 s.
        let p = curved(
            vec![SpeedPoint {
                source: 0,
                speed: 2.0,
            }],
            8_000_000,
        );
        let a = p.tracks[0].segments[0].clone();
        assert_eq!(a.target_range.duration, 4_000_000);
        assert_eq!(
            scene_cut_times(&p, &a),
            vec![1_000_000, 2_000_000, 3_000_000]
        );
        assert_eq!(segment_beats(&p, &a), vec![500_000, 1_500_000, 3_500_000]);
        // Auto-cut puts the splits on those frames.
        let command = auto_cut_to_beat(&p, &["a".into()], 1).unwrap();
        let mut q = p.clone();
        command.apply(&mut q).unwrap();
        let starts: Vec<Micros> = q.tracks[0]
            .segments
            .iter()
            .map(|s| s.target_range.start)
            .collect();
        assert_eq!(starts, vec![0, 500_000, 1_500_000, 3_500_000, 4_000_000]);
    }

    #[test]
    fn markers_on_a_ramp_land_where_the_ramp_plays_them() {
        use crate::modules::project::speed::elapsed;
        use crate::modules::project::SpeedPoint;
        // Slow at the start, fast at the end.
        let points = vec![
            SpeedPoint {
                source: 1_000_000,
                speed: 0.5,
            },
            SpeedPoint {
                source: 9_000_000,
                speed: 3.0,
            },
        ];
        let p = curved(points.clone(), 8_000_000);
        let a = p.tracks[0].segments[0].clone();
        let on_screen = |source: Micros| elapsed(&points, 1_000_000.0, source as f64);
        let frame = 1e6 / 30.0;
        for (got, source) in scene_cut_times(&p, &a)
            .into_iter()
            .zip([3_000_000, 5_000_000, 7_000_000])
        {
            // On the frame grid, within half a frame of where it plays.
            assert!(
                (got as f64 - on_screen(source)).abs() <= frame / 2.0 + 1.0,
                "cut at {source}: {got} against {}",
                on_screen(source)
            );
        }
        for (got, source) in segment_beats(&p, &a)
            .into_iter()
            .zip([2_000_000, 4_000_000, 8_000_000])
        {
            assert!(
                (got as f64 - on_screen(source)).abs() <= 1.0,
                "beat at {source}: {got} against {}",
                on_screen(source)
            );
        }
    }

    #[test]
    fn a_cut_too_far_from_a_beat_stays() {
        let mut p = project();
        with_beats(&mut p, vec![2_000_000]);
        assert!(snap_cuts_to_beats(&p, &["a".into()], 400_000).is_err());
    }

    #[test]
    fn reframing_fills_the_canvas_and_follows_the_path() {
        let mut p = project();
        p.canvas.width = 1080;
        p.canvas.height = 1920;
        let path = vec![
            PathPoint {
                t: 1_000_000,
                centre: 0.5,
                cut: false,
            },
            PathPoint {
                t: 3_000_000,
                centre: 0.8,
                cut: false,
            },
        ];
        let command = reframe_command(&p, "a", Axis::Horizontal, &path).unwrap();
        let mut q = p.clone();
        command.apply(&mut q).unwrap();
        let (_, s) = q.segment("a").unwrap();
        // 1920×1080 fitted into 1080×1920 is 1080×607.5; filling the height
        // scales it by 1920 / 607.5.
        assert!((s.transform.scale[0] - 1920.0 / 607.5).abs() < 1e-3);
        let track = &s.keyframes[0];
        assert_eq!(track.property, AnimatableProperty::PositionX);
        assert_eq!(track.keyframes[0].time, 0);
        assert_eq!(track.keyframes[0].value, 0.0);
        // A subject right of centre moves the picture left.
        let last = track.keyframes[1].value;
        let qw = 1080.0 * 1920.0 / 607.5;
        let expected = ((0.5 - 0.8) * qw / 540.0 * 10_000.0f32).round() / 10_000.0;
        assert!((last - expected).abs() < 1e-3, "{last} vs {expected}");
        command.invert().apply(&mut q).unwrap();
        assert!(q.segment("a").unwrap().1.keyframes.is_empty());
    }

    #[test]
    fn a_ratio_keeps_the_long_edge() {
        assert_eq!(canvas_for_ratio((1920, 1080), (9, 16)), (1080, 1920));
        assert_eq!(canvas_for_ratio((1920, 1080), (1, 1)), (1920, 1920));
        assert_eq!(canvas_for_ratio((1080, 1920), (4, 5)), (1536, 1920));
    }
}
