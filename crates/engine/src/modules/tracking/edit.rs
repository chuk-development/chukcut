//! Undoable tracking edits.
//!
//! ## Why a command type of its own
//!
//! A track is a pool material and `EditCommand` has no variant that writes the
//! pool's tracking categories; the enum belongs to `timeline/ops.rs`, which
//! other work changes all the time. `state::DocumentHistory` already carries a
//! second kind of command for exactly this reason (`ConfigureCommand`), so
//! tracking is a third: [`TrackingCommand`] sets or clears one tracking or
//! follow material, wraps an ordinary `EditCommand` for the segment half of an
//! edit (a follower's `extras`, its keyframes), and composes, so "track, then
//! attach" is one undo step and Ctrl+Z walks it in order with everything else.
//!
//! Every builder here reads the document and returns the command; nothing
//! here mutates. The commands are exact inverses of each other because the
//! two `Set*` variants carry both sides whole, the way `RemoveSegment` carries
//! the whole segment.

use serde::{Deserialize, Serialize};

use super::follow::{self, object_through, target_segment};
use super::model::{FollowMaterial, FollowMode, TrackingMaterial};
use crate::modules::project::document::{
    new_id, AnimatableProperty, Easing, Id, Keyframe, KeyframeTrack, Micros, Project, Segment,
};
use crate::modules::render::layout::animated_transform;
use crate::modules::timeline::ops::EditCommand;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackingCommand {
    /// Put, replace or remove the tracking material `id`. `None` is "absent".
    SetTrack {
        id: Id,
        before: Option<Box<TrackingMaterial>>,
        after: Option<Box<TrackingMaterial>>,
    },
    /// Put, replace or remove the follow link `id`.
    SetFollow {
        id: Id,
        before: Option<FollowMaterial>,
        after: Option<FollowMaterial>,
    },
    /// A segment edit that belongs to a tracking edit. Boxed because an
    /// `EditCommand` is several hundred bytes and the other variants are not.
    Edit(Box<EditCommand>),
    Composite {
        label: String,
        commands: Vec<TrackingCommand>,
    },
}

impl TrackingCommand {
    fn edit(command: EditCommand) -> Self {
        Self::Edit(Box::new(command))
    }

    pub fn label(&self) -> String {
        match self {
            Self::SetTrack { before, after, .. } => match (before, after) {
                (None, Some(_)) => "Track object".into(),
                (Some(_), None) => "Remove track".into(),
                _ => "Change track".into(),
            },
            Self::SetFollow { before, after, .. } => match (before, after) {
                (None, Some(_)) => "Follow track".into(),
                (Some(_), None) => "Stop following".into(),
                _ => "Change tracking".into(),
            },
            Self::Edit(command) => command.label(),
            Self::Composite { label, .. } => label.clone(),
        }
    }

    pub fn invert(&self) -> Self {
        match self {
            Self::SetTrack { id, before, after } => Self::SetTrack {
                id: id.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            Self::SetFollow { id, before, after } => Self::SetFollow {
                id: id.clone(),
                before: after.clone(),
                after: before.clone(),
            },
            Self::Edit(command) => Self::edit(command.invert()),
            Self::Composite { label, commands } => Self::Composite {
                label: label.clone(),
                commands: commands.iter().rev().map(Self::invert).collect(),
            },
        }
    }

    /// Apply, all or nothing: a composite that fails half-way is rolled back.
    pub fn apply(&self, project: &mut Project) -> Result<(), String> {
        match self {
            Self::SetTrack { id, before, after } => {
                if let Some(after) = after {
                    if &after.id != id {
                        return Err("a track cannot change its id".into());
                    }
                    if let Some(field) = after.non_finite_field() {
                        return Err(format!("{field} is not a finite number"));
                    }
                    if !after.is_sorted() {
                        return Err("track samples must be in time order".into());
                    }
                }
                set(
                    &mut project.materials.trackings,
                    id,
                    before.as_deref(),
                    after.as_deref(),
                    |m| &m.id,
                    "track",
                )
            }
            Self::SetFollow { id, before, after } => {
                if let Some(after) = after {
                    if &after.id != id {
                        return Err("a follow link cannot change its id".into());
                    }
                    if !(after.offset[0].is_finite() && after.offset[1].is_finite()) {
                        return Err("the follow offset is not a finite number".into());
                    }
                }
                set(
                    &mut project.materials.follows,
                    id,
                    before.as_ref(),
                    after.as_ref(),
                    |m| &m.id,
                    "follow link",
                )
            }
            Self::Edit(command) => command.apply(project),
            Self::Composite { commands, .. } => {
                for (done, command) in commands.iter().enumerate() {
                    if let Err(error) = command.apply(project) {
                        for applied in commands[..done].iter().rev() {
                            // Undoing what just succeeded cannot fail.
                            let _ = applied.invert().apply(project);
                        }
                        return Err(error);
                    }
                }
                Ok(())
            }
        }
    }
}

/// Swap `before` for `after` in a category kept sorted by id — sorted so that
/// a remove and its undo leave the file byte-identical, without carrying an
/// index the way the media pool's commands must.
fn set<T: Clone + PartialEq>(
    list: &mut Vec<T>,
    id: &str,
    before: Option<&T>,
    after: Option<&T>,
    id_of: fn(&T) -> &Id,
    what: &str,
) -> Result<(), String> {
    // Found by scanning rather than bisecting, so a hand-edited file whose
    // list is out of order still resolves; new entries go in sorted.
    let found = list
        .iter()
        .position(|m| id_of(m) == id)
        .ok_or_else(|| list.partition_point(|m| id_of(m).as_str() < id));
    match (found, before) {
        (Ok(i), Some(expected)) if &list[i] == expected => {}
        (Err(_), None) => {}
        _ => {
            return Err(format!(
                "that {what} is not what this edit expected; the project changed underneath it"
            ))
        }
    }
    match (found, after) {
        (Ok(i), Some(after)) => list[i] = after.clone(),
        (Ok(i), None) => {
            list.remove(i);
        }
        (Err(i), Some(after)) => list.insert(i, after.clone()),
        (Err(_), None) => {}
    }
    Ok(())
}

/// The remove + insert pair `inspector::edit` uses for a whole-segment change:
/// exact undo, every entry check re-run.
pub fn replace_segment(
    project: &Project,
    segment_id: &str,
    label: &str,
    mutate: impl FnOnce(&mut Segment),
) -> Result<EditCommand, String> {
    let (track, before) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("segment found on this track");
    let mut after = before.clone();
    mutate(&mut after);
    Ok(EditCommand::Composite {
        label: label.into(),
        commands: vec![
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
        ],
    })
}

// --- builders ---------------------------------------------------------------------

/// Put a new track into the pool.
pub fn add_track(track: TrackingMaterial) -> TrackingCommand {
    TrackingCommand::SetTrack {
        id: track.id.clone(),
        before: None,
        after: Some(Box::new(track)),
    }
}

/// Replace a track's samples or settings in place; every follower sees it.
pub fn replace_track(
    project: &Project,
    after: TrackingMaterial,
) -> Result<TrackingCommand, String> {
    let before = project
        .materials
        .tracking(&after.id)
        .ok_or("that track is no longer in the project")?
        .clone();
    Ok(TrackingCommand::SetTrack {
        id: after.id.clone(),
        before: Some(Box::new(before)),
        after: Some(Box::new(after)),
    })
}

/// Make `overlay_id` follow `track_id` as seen through `target_id`, without
/// moving it at `at` (timeline). Replaces any follow it had.
pub fn attach(
    project: &Project,
    overlay_id: &str,
    track_id: &str,
    target_id: &str,
    mode: FollowMode,
    at: Micros,
) -> Result<TrackingCommand, String> {
    let track = project
        .materials
        .tracking(track_id)
        .ok_or("that track is no longer in the project")?;
    let (_, target) = project
        .segment(target_id)
        .ok_or("the tracked clip is no longer on the timeline")?;
    let (_, overlay) = project
        .segment(overlay_id)
        .ok_or("the clip is no longer on the timeline")?;
    if overlay_id == target_id {
        return Err("a clip cannot follow itself".into());
    }
    let object = object_through(project, track, target, at)
        .ok_or("the track has no position to follow at the playhead")?;

    let link = FollowMaterial {
        id: new_id(),
        track_id: track_id.into(),
        target_segment_id: target_id.into(),
        mode,
        offset: [-object.position[0], -object.position[1]],
        reference: object.source_time,
    };

    let mut commands = detach_parts(project, overlay)?;
    commands.push(TrackingCommand::SetFollow {
        id: link.id.clone(),
        before: None,
        after: Some(link.clone()),
    });
    let follow_id = link.id.clone();
    let project_after_detach = {
        // The segment edit must be built against the segment as the detach
        // left it, so build both halves against a scratch copy.
        let mut copy = project.clone();
        for command in &commands {
            command.apply(&mut copy)?;
        }
        copy
    };
    commands.push(TrackingCommand::edit(replace_segment(
        &project_after_detach,
        overlay_id,
        "Follow track",
        |segment| segment.extras.push(follow_id),
    )?));
    Ok(TrackingCommand::Composite {
        label: "Follow track".into(),
        commands,
    })
}

/// The commands that take `overlay`'s follow link off it, if it has one: the
/// id out of its extras, and the link itself out of the pool unless another
/// clip (a split half, a copy) still uses it.
fn detach_parts(project: &Project, overlay: &Segment) -> Result<Vec<TrackingCommand>, String> {
    let Some(link) = project.materials.follow_of(overlay) else {
        return Ok(Vec::new());
    };
    let link = link.clone();
    let mut commands = vec![TrackingCommand::edit(replace_segment(
        project,
        &overlay.id,
        "Stop following",
        |segment| segment.extras.retain(|id| id != &link.id),
    )?)];
    let shared = project
        .materials
        .followers(project, &link.id)
        .iter()
        .any(|s| s.id != overlay.id);
    if !shared {
        commands.push(TrackingCommand::SetFollow {
            id: link.id.clone(),
            before: Some(link),
            after: None,
        });
    }
    Ok(commands)
}

/// Stop `overlay_id` following, leaving it where it is shown at `at`: the
/// followed position, scale and rotation become its own, for every property
/// it does not animate with keyframes.
pub fn detach(project: &Project, overlay_id: &str, at: Micros) -> Result<TrackingCommand, String> {
    let (_, overlay) = project
        .segment(overlay_id)
        .ok_or("the clip is no longer on the timeline")?;
    if project.materials.follow_of(overlay).is_none() {
        return Err("the clip does not follow a track".into());
    }
    let shown = follow::followed_transform(project, overlay, at);
    let mut commands = detach_parts(project, overlay)?;
    if let Some(shown) = shown {
        let mut copy = project.clone();
        for command in &commands {
            command.apply(&mut copy)?;
        }
        let animated = |p: AnimatableProperty| overlay.keyframes.iter().any(|t| t.property == p);
        commands.push(TrackingCommand::edit(replace_segment(
            &copy,
            overlay_id,
            "Stop following",
            |segment| {
                let t = &mut segment.transform;
                if !animated(AnimatableProperty::PositionX) {
                    t.position[0] = shown.position[0];
                }
                if !animated(AnimatableProperty::PositionY) {
                    t.position[1] = shown.position[1];
                }
                if !animated(AnimatableProperty::ScaleX) {
                    t.scale[0] = shown.scale[0];
                }
                if !animated(AnimatableProperty::ScaleY) {
                    t.scale[1] = shown.scale[1];
                }
                if !animated(AnimatableProperty::Rotation) {
                    t.rotation = shown.rotation;
                }
            },
        )?));
    }
    Ok(TrackingCommand::Composite {
        label: "Stop following".into(),
        commands,
    })
}

/// Remove a track and stop everything following it, each follower keeping its
/// place at `at`.
pub fn remove_track(
    project: &Project,
    track_id: &str,
    at: Micros,
) -> Result<TrackingCommand, String> {
    let track = project
        .materials
        .tracking(track_id)
        .ok_or("that track is no longer in the project")?
        .clone();
    let mut commands = Vec::new();
    let mut scratch = project.clone();
    let links: Vec<FollowMaterial> = project
        .materials
        .follows
        .iter()
        .filter(|f| f.track_id == track_id)
        .cloned()
        .collect();
    for link in &links {
        let followers: Vec<String> = scratch
            .materials
            .followers(&scratch, &link.id)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        for follower in followers {
            let command = detach(&scratch, &follower, at)?;
            command.apply(&mut scratch)?;
            commands.push(command);
        }
        // A link nobody carries (its clips were deleted) goes with the track.
        if let Some(orphan) = scratch.materials.follow(&link.id).cloned() {
            let command = TrackingCommand::SetFollow {
                id: orphan.id.clone(),
                before: Some(orphan),
                after: None,
            };
            command.apply(&mut scratch)?;
            commands.push(command);
        }
    }
    commands.push(TrackingCommand::SetTrack {
        id: track.id.clone(),
        before: Some(Box::new(track)),
        after: None,
    });
    Ok(TrackingCommand::Composite {
        label: "Remove track".into(),
        commands,
    })
}

/// Write the follow as keyframes on the overlay, one per frame (fewer where a
/// straight line between two reproduces the ones in between), and remove the
/// link. The overlay then plays exactly as it did, and the motion can be
/// edited by hand.
pub fn bake(project: &Project, overlay_id: &str) -> Result<TrackingCommand, String> {
    let (_, overlay) = project
        .segment(overlay_id)
        .ok_or("the clip is no longer on the timeline")?;
    let link = project
        .materials
        .follow_of(overlay)
        .ok_or("the clip does not follow a track")?
        .clone();
    let fps = if project.fps.is_finite() && project.fps > 0.0 {
        project.fps
    } else {
        30.0
    };
    let start = overlay.target_range.start;
    let duration = overlay.target_range.duration;
    let frames = ((duration as f64 * fps / 1_000_000.0).ceil() as i64).max(1);

    let mut series: Vec<(AnimatableProperty, Vec<(Micros, f32)>)> = vec![
        (AnimatableProperty::PositionX, Vec::new()),
        (AnimatableProperty::PositionY, Vec::new()),
    ];
    if link.mode.scales() {
        series.push((AnimatableProperty::ScaleX, Vec::new()));
        series.push((AnimatableProperty::ScaleY, Vec::new()));
    }
    if link.mode.rotates() {
        series.push((AnimatableProperty::Rotation, Vec::new()));
    }
    for frame in 0..=frames {
        let rel = ((frame as f64 * 1_000_000.0 / fps).round() as Micros).min(duration);
        let at = start + rel;
        let shown = follow::followed_transform(project, overlay, at)
            .unwrap_or_else(|| animated_transform(overlay, at));
        for (property, values) in series.iter_mut() {
            let value = match property {
                AnimatableProperty::PositionX => shown.position[0],
                AnimatableProperty::PositionY => shown.position[1],
                AnimatableProperty::ScaleX => shown.scale[0],
                AnimatableProperty::ScaleY => shown.scale[1],
                _ => shown.rotation,
            };
            if values.last().is_none_or(|(t, _)| *t != rel) {
                values.push((rel, value));
            }
        }
    }

    let mut commands = detach_parts(project, overlay)?;
    let mut copy = project.clone();
    for command in &commands {
        command.apply(&mut copy)?;
    }
    commands.push(TrackingCommand::edit(replace_segment(
        &copy,
        overlay_id,
        "Bake track to keyframes",
        |segment| {
            for (property, values) in series {
                let tolerance = match property {
                    AnimatableProperty::Rotation => 0.05,
                    AnimatableProperty::ScaleX | AnimatableProperty::ScaleY => 0.0005,
                    _ => 0.0002,
                };
                let keyframes = simplify(&values, tolerance)
                    .into_iter()
                    .map(|(time, value)| Keyframe {
                        time,
                        value,
                        easing: Easing::Linear,
                    })
                    .collect();
                segment.keyframes.retain(|t| t.property != property);
                segment.keyframes.push(KeyframeTrack {
                    property,
                    keyframes,
                });
            }
            segment.keyframes.sort_by_key(|t| t.property);
        },
    )?));
    Ok(TrackingCommand::Composite {
        label: "Bake track to keyframes".into(),
        commands,
    })
}

/// Bake `followers` to keyframes, then run the timeline edit `then` builds —
/// all as one undo step.
///
/// The bakes have to come first: a follow is evaluated through the tracked
/// clip, so once that clip is gone there is no motion left to bake. `then` is
/// built against the document as the bakes leave it, and gets the same two
/// expansions `DocumentHistory::apply` gives every timeline edit (link
/// partners, then transitions that lose their cut), because it does not pass
/// through there.
pub fn bake_then(
    project: &Project,
    followers: &[Id],
    label: &str,
    then: impl FnOnce(&Project) -> Result<EditCommand, String>,
) -> Result<TrackingCommand, String> {
    let mut scratch = project.clone();
    let mut commands = Vec::new();
    for overlay_id in followers {
        let command = bake(&scratch, overlay_id)?;
        command.apply(&mut scratch)?;
        commands.push(command);
    }
    let edit = then(&scratch)?;
    let edit = crate::modules::timeline::ops::mirror_linked_edits(&scratch, edit);
    let edit = crate::modules::timeline::ops::detach_broken_transitions(&scratch, edit);
    commands.push(TrackingCommand::edit(edit));
    Ok(TrackingCommand::Composite {
        label: label.into(),
        commands,
    })
}

/// Delete clips with `delete` (the primitives a delete gesture produced, as
/// `timeline_apply_many` takes them), baking every overlay that follows one of
/// `deleted` first, so the overlays keep their motion as keyframes. One undo
/// step. See `validate::dependent_followers` for which overlays those are.
pub fn bake_and_delete(
    project: &Project,
    deleted: &[Id],
    delete: Vec<EditCommand>,
    label: &str,
) -> Result<TrackingCommand, String> {
    let followers = super::validate::dependent_followers(project, deleted);
    bake_then(project, &followers, label, |scratch| {
        crate::modules::timeline::ops::compose_edits(scratch, label, delete)
    })
}

/// Drop points a straight line between their neighbours reproduces within
/// `tolerance` (greedy; keeps both ends).
fn simplify(points: &[(Micros, f32)], tolerance: f32) -> Vec<(Micros, f32)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    let mut anchor = 0;
    let mut i = 2;
    while i < points.len() {
        let (t0, v0) = points[anchor];
        let (t1, v1) = points[i];
        let fits = points[anchor + 1..i].iter().all(|&(t, v)| {
            let f = (t - t0) as f32 / (t1 - t0).max(1) as f32;
            (v0 + (v1 - v0) * f - v).abs() <= tolerance
        });
        if !fits {
            anchor = i - 1;
            out.push(points[anchor]);
        }
        i += 1;
    }
    out.push(*points.last().expect("at least three points"));
    out
}

/// The default target for an overlay: the topmost visible video clip under
/// it at `at` (or at its start, when the playhead is elsewhere).
pub fn default_target(project: &Project, overlay_id: &str, at: Micros) -> Option<Id> {
    let (_, overlay) = project.segment(overlay_id)?;
    let at = if overlay.target_range.contains(at) {
        at
    } else {
        overlay.target_range.start
    };
    project
        .segments_at(at)
        .into_iter()
        .filter(|(track, s)| {
            crate::modules::render::layout::track_is_visible(track)
                && s.id != overlay_id
                && project.materials.video(&s.material_id).is_some()
        })
        .map(|(_, s)| s.id.clone())
        .next_back()
}

/// The track an overlay follows, if any, and the clip it is seen through at
/// `at`.
pub fn followed_track<'a>(
    project: &'a Project,
    overlay: &Segment,
    at: Micros,
) -> Option<(
    &'a TrackingMaterial,
    &'a FollowMaterial,
    Option<&'a Segment>,
)> {
    let link = project.materials.follow_of(overlay)?;
    let track = project.materials.tracking(&link.track_id)?;
    Some((track, link, target_segment(project, link, track, at)))
}

#[cfg(test)]
mod tests {
    use super::super::follow::followed_transform;
    use super::*;
    use crate::modules::project::document::TimeRange;

    fn project() -> Project {
        let mut p = super::super::follow::tests::project();
        // Start unlinked: the fixture's overlay carries a follow.
        p.materials.follows.clear();
        p.tracks[1].segments[0].extras.clear();
        p
    }

    fn json(p: &Project) -> String {
        serde_json::to_string(p).unwrap()
    }

    #[test]
    fn attaching_does_not_move_the_overlay_and_undo_is_exact() {
        let mut p = project();
        p.tracks[1].segments[0].transform.position = [0.3, 0.2];
        let original = json(&p);
        let command = attach(&p, "o", "track", "v", FollowMode::Position, 2_000_000).unwrap();
        command.apply(&mut p).unwrap();
        let overlay = p.segment("o").unwrap().1.clone();
        let at_attach = followed_transform(&p, &overlay, 2_000_000).unwrap();
        assert!((at_attach.position[0] - 0.3).abs() < 1e-4);
        assert!((at_attach.position[1] - 0.2).abs() < 1e-4);
        // One second later the object moved by 0.1 of the frame = 0.2 canvas.
        let later = followed_transform(&p, &overlay, 3_000_000).unwrap();
        assert!((later.position[0] - 0.5).abs() < 1e-4);
        command.invert().apply(&mut p).unwrap();
        assert!(json(&p) == original, "undo was not exact");
    }

    #[test]
    fn retrack_replaces_samples_for_every_follower() {
        let mut p = project();
        attach(&p, "o", "track", "v", FollowMode::Position, 0)
            .unwrap()
            .apply(&mut p)
            .unwrap();
        let mut changed = p.materials.tracking("track").unwrap().clone();
        for s in &mut changed.samples {
            s.y = 0.25;
        }
        let command = replace_track(&p, changed).unwrap();
        command.apply(&mut p).unwrap();
        let overlay = p.segment("o").unwrap().1.clone();
        // The object moved up by a quarter frame = half a canvas, and the
        // overlay with it.
        let t = followed_transform(&p, &overlay, 0).unwrap();
        assert!((t.position[1] - 0.5).abs() < 1e-4);
        command.invert().apply(&mut p).unwrap();
        let t = followed_transform(&p, &overlay, 0).unwrap();
        assert!(t.position[1].abs() < 1e-4);
    }

    #[test]
    fn detach_keeps_the_overlay_where_it_was_shown() {
        let mut p = project();
        attach(&p, "o", "track", "v", FollowMode::Position, 0)
            .unwrap()
            .apply(&mut p)
            .unwrap();
        let overlay = p.segment("o").unwrap().1.clone();
        let shown = followed_transform(&p, &overlay, 6_000_000).unwrap();
        let before = json(&p);
        let command = detach(&p, "o", 6_000_000).unwrap();
        command.apply(&mut p).unwrap();
        let overlay = p.segment("o").unwrap().1.clone();
        assert!(p.materials.follow_of(&overlay).is_none());
        assert!(p.materials.follows.is_empty());
        assert_eq!(overlay.transform.position, shown.position);
        command.invert().apply(&mut p).unwrap();
        assert!(json(&p) == before, "the document changed");
    }

    #[test]
    fn baking_reproduces_the_follow_at_every_frame() {
        let mut p = project();
        attach(
            &p,
            "o",
            "track",
            "v",
            FollowMode::PositionScaleRotation,
            1_000_000,
        )
        .unwrap()
        .apply(&mut p)
        .unwrap();
        let overlay = p.segment("o").unwrap().1.clone();
        let followed: Vec<_> = (0..300)
            .map(|f| followed_transform(&p, &overlay, f * 33_333).unwrap())
            .collect();
        let command = bake(&p, "o").unwrap();
        command.apply(&mut p).unwrap();
        let baked = p.segment("o").unwrap().1.clone();
        assert!(p.materials.follow_of(&baked).is_none());
        // Linear motion: the bake collapses to a handful of keyframes.
        let x = baked
            .keyframes
            .iter()
            .find(|t| t.property == AnimatableProperty::PositionX)
            .unwrap();
        assert!(x.keyframes.len() < 10, "{} keyframes", x.keyframes.len());
        for (f, want) in followed.iter().enumerate() {
            let got = animated_transform(&baked, f as Micros * 33_333);
            assert!(
                (got.position[0] - want.position[0]).abs() < 1e-3,
                "frame {f}"
            );
            assert!(
                (got.position[1] - want.position[1]).abs() < 1e-3,
                "frame {f}"
            );
        }
    }

    #[test]
    fn removing_a_track_detaches_its_followers_in_one_step() {
        let mut p = project();
        attach(&p, "o", "track", "v", FollowMode::Position, 0)
            .unwrap()
            .apply(&mut p)
            .unwrap();
        let before = json(&p);
        let command = remove_track(&p, "track", 0).unwrap();
        command.apply(&mut p).unwrap();
        assert!(p.materials.trackings.is_empty());
        assert!(p.materials.follows.is_empty());
        assert!(p.segment("o").unwrap().1.extras.is_empty());
        command.invert().apply(&mut p).unwrap();
        assert!(json(&p) == before, "the document changed");
    }

    #[test]
    fn a_stale_edit_is_refused_and_leaves_nothing_behind() {
        let mut p = project();
        let command = attach(&p, "o", "track", "v", FollowMode::Position, 0).unwrap();
        // Somebody else removed the track meanwhile.
        p.materials.trackings.clear();
        let fresh = TrackingCommand::Composite {
            label: "x".into(),
            commands: vec![
                command,
                TrackingCommand::SetTrack {
                    id: "track".into(),
                    before: None,
                    after: None,
                },
                // Fails: there is no "missing" track to replace.
                TrackingCommand::SetFollow {
                    id: "missing".into(),
                    before: Some(FollowMaterial {
                        id: "missing".into(),
                        track_id: "t".into(),
                        target_segment_id: "v".into(),
                        mode: FollowMode::Position,
                        offset: [0.0; 2],
                        reference: 0,
                    }),
                    after: None,
                },
            ],
        };
        let before = json(&p);
        assert!(fresh.apply(&mut p).is_err());
        assert!(json(&p) == before, "the document changed");
    }

    #[test]
    fn the_default_target_is_the_video_under_the_overlay() {
        let mut p = project();
        assert_eq!(default_target(&p, "o", 1_000_000).as_deref(), Some("v"));
        p.tracks[0].segments[0].target_range = TimeRange::new(20_000_000, 1_000_000);
        assert_eq!(default_target(&p, "o", 1_000_000), None);
    }

    #[test]
    fn bake_then_delete_keeps_the_motion_and_undoes_in_one_step() {
        // The fixture as it ships: the overlay follows the track through "v".
        let mut p = super::super::follow::tests::project();
        let original = json(&p);
        let (track, video) = p.segment("v").unwrap();
        let delete = vec![EditCommand::RemoveSegment {
            track_id: track.id.clone(),
            segment: video.clone(),
            index: 0,
        }];
        let command = bake_and_delete(&p, &["v".into()], delete, "Delete clip").unwrap();
        let mut history = crate::state::DocumentHistory::new();
        history.apply_tracking(&mut p, command).unwrap();

        assert!(p.segment("v").is_none());
        let overlay = p.segment("o").unwrap().1;
        assert!(p.materials.follow_of(overlay).is_none());
        assert!(p.materials.follows.is_empty());
        let x = overlay
            .keyframes
            .iter()
            .find(|t| t.property == AnimatableProperty::PositionX)
            .expect("the motion is baked");
        assert!(x.keyframes.len() >= 2);
        // Nothing left that follows a missing target.
        assert!(super::super::validate::issues(&p).is_empty());

        history.undo(&mut p).unwrap();
        assert!(json(&p) == original, "undo was not exact");
    }

    #[test]
    fn simplify_keeps_corners_and_drops_lines() {
        let points: Vec<_> = (0..20)
            .map(|i| (i as Micros, if i < 10 { i as f32 } else { 10.0 }))
            .collect();
        let s = simplify(&points, 1e-4);
        assert_eq!(s.len(), 3, "{s:?}");
    }
}
