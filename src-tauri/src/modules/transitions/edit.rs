//! What the three transition `EditCommand` variants do.
//!
//! Every mutation still goes through `EditCommand` — there is no second path,
//! and `timeline/ops.rs` owns the enum. What lives here are the bodies, so the
//! rules about what makes a transition placeable are next to the rules about
//! what makes it render, and the command table stays a table.
//!
//! The three commands are the usual invertible triple:
//!
//! | command | undone by |
//! |---|---|
//! | `AddTransition { segment_id, transition }` | `RemoveTransition` with the same payload |
//! | `RemoveTransition { segment_id, transition }` | `AddTransition` with the same payload |
//! | `SetTransition { segment_id, before, after }` | itself, with the two swapped |
//!
//! Both add and remove carry the **whole** material rather than its id, for the
//! reason `RemoveSegment` carries the whole segment: undo has to put back
//! exactly what was there, and an id is not enough to rebuild a colour, an
//! easing and a direction.
//!
//! ## The garbage question
//!
//! Deleting a clip through `RemoveSegment` takes its transition id with it —
//! the id lives in `Segment::extras` — but leaves the `TransitionMaterial` in
//! the pool with nothing referring to it. That is deliberate. Collecting it
//! would mean `RemoveSegment` carrying the material too, which would either
//! change that command's payload for every caller or make undo lossy. An
//! unreferenced transition is inert: resolution only ever goes segment →
//! material, so nothing reads it, and it costs about a hundred bytes.
//! [`detach_around`] is what a ripple-delete composite uses when it wants the
//! removal to be tidy as well as correct.

use crate::modules::project::document::{
    Micros, Project, Segment, Track, TransitionKind, TransitionMaterial,
    DEFAULT_TRANSITION_DURATION,
};
use crate::modules::timeline::ops::EditCommand;

use super::resolve::max_duration;

/// The clip immediately before `segment_id` on its own track, when the two
/// touch.
///
/// "Touch" is exact equality of the outgoing end and the incoming start, not
/// approximate: times are microseconds and the editing model docks clips
/// exactly, so a gap of one microsecond is a gap and a transition across it
/// would have to invent a frame.
pub fn predecessor<'a>(project: &'a Project, segment_id: &str) -> Option<(&'a Track, &'a Segment)> {
    let (track, segment) = project.segment(segment_id)?;
    let index = track.segments.iter().position(|s| s.id == segment.id)?;
    let previous = track.segments.get(index.checked_sub(1)?)?;
    (previous.target_range.end() == segment.target_range.start).then_some((track, previous))
}

/// Whether a transition of `duration` may sit at the head of `segment_id`, and
/// why not when it may not.
///
/// The message is user-facing prose because it goes straight back through the
/// command result to the UI.
pub fn check_placement(project: &Project, segment_id: &str, duration: Micros) -> Result<(), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let (_, previous) = predecessor(project, segment_id)
        .ok_or("a transition needs a clip immediately before this one")?;

    if duration <= 0 {
        return Err("a transition must be longer than zero".into());
    }
    let limit = max_duration(previous, segment);
    if duration > limit {
        return Err(format!(
            "a transition here can be at most {:.2} s; the clips it joins are not long enough",
            limit as f64 / 1_000_000.0
        ));
    }
    Ok(())
}

/// The longest transition that may sit at the head of `segment_id`.
///
/// `0` when there is no clip before it, which is also the answer to "may the
/// user drag a transition onto this edge".
pub fn allowed_duration(project: &Project, segment_id: &str) -> Micros {
    match (project.segment(segment_id), predecessor(project, segment_id)) {
        (Some((_, segment)), Some((_, previous))) => max_duration(previous, segment),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Command bodies
// ---------------------------------------------------------------------------

/// `EditCommand::AddTransition`.
pub fn add(
    project: &mut Project,
    segment_id: &str,
    material: &TransitionMaterial,
) -> Result<(), String> {
    check_placement(project, segment_id, material.duration)?;

    if project.materials.transition(&material.id).is_some() {
        return Err("that transition is already in the project".into());
    }
    let existing = project
        .segment(segment_id)
        .map(|(_, s)| project.materials.transition_of(s).is_some())
        .unwrap_or(false);
    if existing {
        return Err("this clip already has a transition; change it instead".into());
    }

    // Kept in a canonical order rather than in the order they were added.
    // Nothing reads the pool positionally — a transition is found by id — and
    // pinning the order is what makes `remove` and `add` exact inverses:
    // pushing meant that removing the first of two transitions and undoing it
    // put it back second, and the fuzzer's byte-exact undo check found it
    // (`tests/edit_fuzz.rs::transitions_survive_the_edits_that_move_the_clips_they_join`).
    let at = project
        .materials
        .transitions
        .partition_point(|t| t.id < material.id);
    project.materials.transitions.insert(at, material.clone());
    project
        .segment_mut(segment_id)
        .expect("checked above")
        .extras
        .push(material.id.clone());
    Ok(())
}

/// `EditCommand::RemoveTransition`.
pub fn remove(
    project: &mut Project,
    segment_id: &str,
    material: &TransitionMaterial,
) -> Result<(), String> {
    let segment = project
        .segment_mut(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let before = segment.extras.len();
    segment.extras.retain(|id| id != &material.id);
    if segment.extras.len() == before {
        return Err("this clip has no such transition".into());
    }
    project.materials.transitions.retain(|m| m.id != material.id);
    Ok(())
}

/// `EditCommand::SetTransition` — retime, restyle, redirect.
///
/// The id is not allowed to change: this edits a transition in place, and
/// swapping its identity would be an add and a remove wearing one label, which
/// is how an undo stack ends up pointing at a material that was never there.
pub fn set(
    project: &mut Project,
    segment_id: &str,
    before: &TransitionMaterial,
    after: &TransitionMaterial,
) -> Result<(), String> {
    if before.id != after.id {
        return Err("a transition cannot change its identity; remove and add instead".into());
    }
    check_placement(project, segment_id, after.duration)?;

    let attached = project
        .segment(segment_id)
        .map(|(_, s)| s.extras.iter().any(|id| id == &after.id))
        .unwrap_or(false);
    if !attached {
        return Err("this clip has no such transition".into());
    }

    let slot = project
        .materials
        .transition_mut(&after.id)
        .ok_or("this project has no such transition")?;
    *slot = after.clone();
    Ok(())
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

/// The command that puts a `kind` transition at the head of `segment_id`.
///
/// `duration` of `None` asks for the default, shortened to fit when the clips
/// are too short for it — a user dropping a transition onto a half-second clip
/// wants a shorter transition, not an error.
pub fn add_command(
    project: &Project,
    segment_id: &str,
    kind: TransitionKind,
    duration: Option<Micros>,
) -> Result<EditCommand, String> {
    let limit = allowed_duration(project, segment_id);
    if limit <= 0 {
        return Err("a transition needs a clip immediately before this one".into());
    }
    let duration = match duration {
        Some(explicit) => explicit,
        None => DEFAULT_TRANSITION_DURATION.min(limit),
    };
    let material = TransitionMaterial::new(kind, duration);
    check_placement(project, segment_id, material.duration)?;
    Ok(EditCommand::AddTransition {
        segment_id: segment_id.to_string(),
        transition: material,
    })
}

/// The command that takes the transition off the head of `segment_id`.
pub fn remove_command(project: &Project, segment_id: &str) -> Result<EditCommand, String> {
    let material = current(project, segment_id)?;
    Ok(EditCommand::RemoveTransition {
        segment_id: segment_id.to_string(),
        transition: material,
    })
}

/// The command that changes only the length of an existing transition.
pub fn retime_command(
    project: &Project,
    segment_id: &str,
    duration: Micros,
) -> Result<EditCommand, String> {
    let before = current(project, segment_id)?;
    let mut after = before.clone();
    after.duration = duration;
    check_placement(project, segment_id, duration)?;
    Ok(EditCommand::SetTransition {
        segment_id: segment_id.to_string(),
        before,
        after,
    })
}

/// The command that replaces an existing transition's parameters wholesale,
/// keeping its identity. What a parameter panel emits when the user lets go of
/// a control.
pub fn set_command(
    project: &Project,
    segment_id: &str,
    after: TransitionMaterial,
) -> Result<EditCommand, String> {
    let before = current(project, segment_id)?;
    if before.id != after.id {
        return Err("a transition cannot change its identity; remove and add instead".into());
    }
    check_placement(project, segment_id, after.duration)?;
    Ok(EditCommand::SetTransition {
        segment_id: segment_id.to_string(),
        before,
        after,
    })
}

/// The transition currently at the head of `segment_id`.
pub fn current(project: &Project, segment_id: &str) -> Result<TransitionMaterial, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    project
        .materials
        .transition_of(segment)
        .cloned()
        .ok_or_else(|| "this clip has no transition".into())
}

/// The removals a structural edit should prepend so that deleting or moving
/// `segment_id` does not leave a transition describing a join that is about to
/// stop existing.
///
/// Both ends matter: the clip's own incoming transition, and the one at the
/// head of whatever follows it, which was joining *to* this clip.
///
/// Returns commands rather than mutating, so the caller folds them into its own
/// `Composite` and the whole thing stays one undo step.
pub fn detach_around(project: &Project, segment_id: &str) -> Vec<EditCommand> {
    let mut out = Vec::new();
    if let Ok(command) = remove_command(project, segment_id) {
        out.push(command);
    }
    if let Some((track, segment)) = project.segment(segment_id) {
        if let Some(index) = track.segments.iter().position(|s| s.id == segment.id) {
            if let Some(next) = track.segments.get(index + 1) {
                if let Ok(command) = remove_command(project, &next.id) {
                    out.push(command);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Easing, TimeRange, TrackKind, TransitionDirection, VideoMaterial,
    };
    use crate::modules::project::{new_id, Severity, Transform};
    use crate::modules::timeline::ops::split_at;
    use crate::modules::timeline::History;

    fn cut_project() -> (Project, String, String, String) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        project.materials.videos.push(VideoMaterial {
            id: "m".into(),
            path: "/nonexistent.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 20_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });

        let mut track = Track::new(TrackKind::Video, "V1");
        let make = |start: Micros, source_start: Micros| Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, 4_000_000),
            source_range: TimeRange::new(source_start, 4_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        let left = make(0, 0);
        let right = make(4_000_000, 8_000_000);
        let (left_id, right_id) = (left.id.clone(), right.id.clone());
        let track_id = track.id.clone();
        track.segments.push(left);
        track.segments.push(right);
        project.tracks.push(track);
        (project, track_id, left_id, right_id)
    }

    fn with_transition() -> (Project, String, String, String, String) {
        let (mut project, track_id, left_id, right_id) = cut_project();
        let command =
            add_command(&project, &right_id, TransitionKind::Dissolve, Some(1_000_000)).unwrap();
        let id = match &command {
            EditCommand::AddTransition { transition, .. } => transition.id.clone(),
            _ => unreachable!(),
        };
        command.apply(&mut project).unwrap();
        (project, track_id, left_id, right_id, id)
    }

    fn no_errors(project: &Project) {
        let errors: Vec<_> = project
            .validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn add_puts_the_material_in_the_pool_and_the_id_on_the_incoming_clip() {
        let (project, _, _, right_id, id) = with_transition();
        assert_eq!(project.materials.transitions.len(), 1);
        let (_, segment) = project.segment(&right_id).unwrap();
        assert!(segment.extras.contains(&id));
        assert!(project.materials.transition_of(segment).is_some());
        no_errors(&project);
    }

    #[test]
    fn a_transition_needs_a_clip_before_it() {
        let (project, _, left_id, _) = cut_project();
        let error = add_command(&project, &left_id, TransitionKind::Dissolve, None).unwrap_err();
        assert!(error.contains("immediately before"), "{error}");
    }

    #[test]
    fn a_transition_cannot_be_longer_than_the_clips_it_joins() {
        let (project, _, _, right_id) = cut_project();
        let error = add_command(
            &project,
            &right_id,
            TransitionKind::Dissolve,
            Some(20_000_000),
        )
        .unwrap_err();
        assert!(error.contains("at most"), "{error}");
        // The default is shortened rather than rejected when the clips are
        // short, which is what a drop onto a tiny clip has to do.
        assert_eq!(allowed_duration(&project, &right_id), 8_000_000);
    }

    #[test]
    fn a_second_transition_on_the_same_edge_is_refused() {
        let (project, _, _, right_id, _) = with_transition();
        let again =
            add_command(&project, &right_id, TransitionKind::Wipe, Some(500_000)).unwrap();
        let mut project = project;
        let error = again.apply(&mut project).unwrap_err();
        assert!(error.contains("already has a transition"), "{error}");
    }

    #[test]
    fn add_and_remove_are_exact_inverses() {
        let (mut project, _, _, right_id, id) = with_transition();
        let remove = remove_command(&project, &right_id).unwrap();

        remove.apply(&mut project).unwrap();
        assert!(project.materials.transitions.is_empty());
        assert!(!project.segment(&right_id).unwrap().1.extras.contains(&id));

        remove.invert().apply(&mut project).unwrap();
        assert_eq!(project.materials.transitions.len(), 1);
        assert_eq!(project.materials.transitions[0].id, id);
        assert!(project.segment(&right_id).unwrap().1.extras.contains(&id));
        no_errors(&project);
    }

    #[test]
    fn retime_round_trips_through_undo_and_redo() {
        let (mut project, _, _, right_id, _) = with_transition();
        let mut history = History::new();

        let command = retime_command(&project, &right_id, 2_000_000).unwrap();
        history.apply(&mut project, command).unwrap();
        assert_eq!(project.materials.transitions[0].duration, 2_000_000);

        history.undo(&mut project).unwrap();
        assert_eq!(project.materials.transitions[0].duration, 1_000_000);

        history.redo(&mut project).unwrap();
        assert_eq!(project.materials.transitions[0].duration, 2_000_000);
        no_errors(&project);
    }

    #[test]
    fn the_whole_add_remove_cycle_round_trips_through_history() {
        let (mut project, _, _, right_id) = cut_project();
        let mut history = History::new();

        let add = add_command(&project, &right_id, TransitionKind::Slide, Some(800_000)).unwrap();
        history.apply(&mut project, add).unwrap();
        assert_eq!(history.undo_label().as_deref(), Some("Add transition"));

        let remove = remove_command(&project, &right_id).unwrap();
        history.apply(&mut project, remove).unwrap();
        assert!(project.materials.transitions.is_empty());

        // Undo the removal, then the addition, then put both back.
        history.undo(&mut project).unwrap();
        assert_eq!(project.materials.transitions.len(), 1);
        history.undo(&mut project).unwrap();
        assert!(project.materials.transitions.is_empty());
        assert!(project.segment(&right_id).unwrap().1.extras.is_empty());

        history.redo(&mut project).unwrap();
        assert_eq!(project.materials.transitions.len(), 1);
        history.redo(&mut project).unwrap();
        assert!(project.materials.transitions.is_empty());
        no_errors(&project);
    }

    #[test]
    fn set_preserves_identity_and_carries_every_parameter() {
        let (mut project, _, _, right_id, id) = with_transition();
        let mut after = current(&project, &right_id).unwrap();
        after.kind = TransitionKind::Wipe;
        after.direction = TransitionDirection::Up;
        after.easing = Easing::EaseOut;
        after.softness = 0.2;

        let command = set_command(&project, &right_id, after).unwrap();
        command.apply(&mut project).unwrap();
        let now = current(&project, &right_id).unwrap();
        assert_eq!(now.id, id);
        assert_eq!(now.kind, TransitionKind::Wipe);
        assert_eq!(now.direction, TransitionDirection::Up);
        assert_eq!(now.softness, 0.2);

        command.invert().apply(&mut project).unwrap();
        let back = current(&project, &right_id).unwrap();
        assert_eq!(back.kind, TransitionKind::Dissolve);
        assert_eq!(back.softness, 0.04);
    }

    #[test]
    fn a_transition_survives_moving_the_pair_it_joins() {
        let (mut project, track_id, left_id, right_id, id) = with_transition();
        // Slide both clips two seconds later, keeping them docked. Order
        // matters — the right one first, or it would land on the left one.
        for (segment_id, to) in [(&right_id, 6_000_000), (&left_id, 2_000_000)] {
            EditCommand::MoveSegment {
                segment_id: segment_id.clone(),
                from_track: track_id.clone(),
                to_track: track_id.clone(),
                from_start: 0,
                to_start: to,
            }
            .apply(&mut project)
            .unwrap();
        }

        let track = project.track(&track_id).unwrap();
        let spans = super::super::resolve::spans(track, &project.materials);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].cut, 6_000_000);
        assert_eq!(spans[0].material.id, id);
        no_errors(&project);
    }

    #[test]
    fn a_transition_survives_trimming_the_clips_it_joins() {
        let (mut project, track_id, left_id, right_id, _) = with_transition();
        // Trim a second off the outgoing clip's tail and dock the incoming one
        // back against it: the cut moves and the transition follows, because
        // the window is derived from the cut rather than stored.
        EditCommand::TrimSegment {
            segment_id: left_id.clone(),
            before_target: TimeRange::new(0, 4_000_000),
            before_source: TimeRange::new(0, 4_000_000),
            after_target: TimeRange::new(0, 3_000_000),
            after_source: TimeRange::new(0, 3_000_000),
        }
        .apply(&mut project)
        .unwrap();
        EditCommand::MoveSegment {
            segment_id: right_id.clone(),
            from_track: track_id.clone(),
            to_track: track_id.clone(),
            from_start: 4_000_000,
            to_start: 3_000_000,
        }
        .apply(&mut project)
        .unwrap();

        let track = project.track(&track_id).unwrap();
        let spans = super::super::resolve::spans(track, &project.materials);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].window, TimeRange::new(2_500_000, 1_000_000));
        no_errors(&project);
    }

    #[test]
    fn splitting_the_incoming_clip_leaves_the_transition_on_the_half_that_kept_the_cut() {
        let (mut project, track_id, _, right_id, id) = with_transition();
        let command = split_at(&project, &right_id, 6_000_000).unwrap();
        command.apply(&mut project).unwrap();

        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 3);
        // The half that still starts at the original cut keeps it…
        assert!(track.segments[1].extras.contains(&id));
        // …and the fresh half, whose left edge is a brand new cut, does not.
        assert!(track.segments[2].extras.is_empty());
        assert_eq!(project.materials.transitions.len(), 1);
        no_errors(&project);

        // And undoing the split puts the clip back with its transition intact.
        command.invert().apply(&mut project).unwrap();
        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 2);
        assert!(track.segments[1].extras.contains(&id));
        no_errors(&project);
    }

    #[test]
    fn splitting_the_outgoing_clip_does_not_touch_the_transition() {
        let (mut project, track_id, left_id, _, id) = with_transition();
        split_at(&project, &left_id, 2_000_000)
            .unwrap()
            .apply(&mut project)
            .unwrap();

        let track = project.track(&track_id).unwrap();
        assert_eq!(track.segments.len(), 3);
        assert!(track.segments[2].extras.contains(&id));
        // The transition now joins the second half of the split to the clip
        // after it, and its window is still centred on the same cut.
        let spans = super::super::resolve::spans(track, &project.materials);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].cut, 4_000_000);
        no_errors(&project);
    }

    #[test]
    fn detach_around_finds_both_ends() {
        let (mut project, _, left_id, right_id, _) = with_transition();
        // The transition is on the incoming clip, so deleting the *outgoing*
        // one orphans it. `detach_around` is what a delete composite prepends.
        let commands = detach_around(&project, &left_id);
        assert_eq!(commands.len(), 1);
        for command in &commands {
            command.apply(&mut project).unwrap();
        }
        assert!(project.materials.transitions.is_empty());
        assert!(project.segment(&right_id).unwrap().1.extras.is_empty());
    }
}
