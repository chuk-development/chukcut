//! What `EditCommand::SetAnimation` does, and the commands the UI builds.
//!
//! `timeline/ops.rs` owns the enum; the body lives here so the rules about
//! what an animation may be sit next to the code that renders it — the
//! arrangement `transitions::edit` set up.
//!
//! ## The one command
//!
//! `SetAnimation { segment_id, before, after }` swaps the segment's
//! animation material for another, or adds or removes it. Both sides carry
//! the whole material, so undo needs nothing from anywhere else, and the
//! inverse is the same command with the two swapped.
//!
//! The pool is kept exactly in step with the references, which is what
//! makes the inverse exact down to the bytes of the saved file:
//!
//! - `after` goes into the pool if it is not there, at its sorted place.
//! - `before` leaves the pool once no segment refers to it any more. A
//!   material two clips share (a duplicated clip) stays while the other
//!   clip still uses it.
//!
//! Every builder here mints a fresh id for `after`, so a shared material is
//! never edited in place under the other clip.

use crate::modules::project::animation::{
    AnimationMaterial, AnimationSlot, ClipAnimation, PunchZoom, TextAnimator, TextSlot,
};
use crate::modules::project::document::{new_id, Micros, Project, Segment};
use crate::modules::timeline::ops::EditCommand;

/// The body of `EditCommand::SetAnimation`.
pub fn set(
    project: &mut Project,
    segment_id: &str,
    before: Option<&AnimationMaterial>,
    after: Option<&AnimationMaterial>,
    slot: Option<usize>,
) -> Result<(), String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let current = project
        .materials
        .animation_of(segment)
        .map(|m| m.id.clone());
    if current.as_deref() != before.map(|m| m.id.as_str()) {
        return Err("the clip's animation changed since this edit was made".into());
    }
    if let Some(after) = after {
        if after.is_empty() {
            return Err("an empty animation is removed, not stored".into());
        }
        if let Some(problem) = after.problem() {
            return Err(problem);
        }
        if let Some(existing) = project.materials.animation(&after.id) {
            // Reusing an id with other parameters is only legitimate as an
            // in-place replacement of this segment's own material.
            let in_place = before.is_some_and(|b| b.id == after.id);
            if existing != after && !in_place {
                return Err(format!("animation {} already exists", after.id));
            }
        }
    }

    // Pool first, so the document never references a material it lacks.
    if let Some(after) = after {
        let pool = &mut project.materials.animations;
        match pool.binary_search_by(|m| m.id.as_str().cmp(after.id.as_str())) {
            Ok(i) => pool[i] = after.clone(),
            Err(i) => pool.insert(i, after.clone()),
        }
    }

    let segment = project
        .segment_mut(segment_id)
        .expect("the segment was found above");
    let position = before.and_then(|b| segment.extras.iter().position(|id| *id == b.id));
    match (position, after) {
        (Some(i), Some(after)) => segment.extras[i] = after.id.clone(),
        (Some(i), None) => {
            segment.extras.remove(i);
        }
        (None, Some(after)) => {
            let at = slot.unwrap_or(usize::MAX).min(segment.extras.len());
            segment.extras.insert(at, after.id.clone());
        }
        (None, None) => {}
    }

    if let Some(before) = before {
        let replaced = after.is_none_or(|a| a.id != before.id);
        if replaced && !referenced(project, &before.id) {
            project.materials.animations.retain(|m| m.id != before.id);
        }
    }
    Ok(())
}

fn referenced(project: &Project, id: &str) -> bool {
    project
        .tracks
        .iter()
        .flat_map(|t| &t.segments)
        .any(|s| s.extras.iter().any(|e| e == id))
}

/// The command that changes `segment_id`'s animation with `change`.
///
/// `change` gets the current parameters (or an empty material) to edit; the
/// result is stored under a fresh id, or the reference is removed when
/// nothing is left.
pub fn edit_command(
    project: &Project,
    segment_id: &str,
    change: impl FnOnce(&mut AnimationMaterial),
) -> Result<EditCommand, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let before = project.materials.animation_of(segment).cloned();
    let mut next = before.clone().unwrap_or_default();
    change(&mut next);
    next.id = new_id();
    if let Some(problem) = next.problem() {
        return Err(problem);
    }
    let unchanged = match &before {
        Some(b) => b.same_parameters(&next),
        None => next.is_empty(),
    };
    if unchanged {
        return Err("nothing to change".into());
    }
    let after = (!next.is_empty()).then_some(next);
    let slot = slot_of(segment, before.as_ref());
    Ok(EditCommand::SetAnimation {
        segment_id: segment_id.to_string(),
        before,
        after,
        slot,
    })
}

/// Set or clear one of a clip's In, Out and Combo animations.
pub fn set_slot_command(
    project: &Project,
    segment_id: &str,
    slot: AnimationSlot,
    animation: Option<ClipAnimation>,
) -> Result<EditCommand, String> {
    edit_command(project, segment_id, |m| match slot {
        AnimationSlot::In => m.intro = animation,
        AnimationSlot::Out => m.outro = animation,
        AnimationSlot::Combo => m.combo = animation,
    })
}

/// Set or clear a text clip's per-unit In or Out animation.
pub fn set_text_command(
    project: &Project,
    segment_id: &str,
    slot: TextSlot,
    animator: Option<TextAnimator>,
) -> Result<EditCommand, String> {
    let (_, segment) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    if animator.is_some() && project.materials.text(&segment.material_id).is_none() {
        return Err("only a text clip can be animated by letter, word or line".into());
    }
    edit_command(project, segment_id, |m| match slot {
        TextSlot::In => m.text_in = animator,
        TextSlot::Out => m.text_out = animator,
    })
}

/// Set or clear a clip's punch-in zoom.
pub fn set_zoom_command(
    project: &Project,
    segment_id: &str,
    zoom: Option<PunchZoom>,
) -> Result<EditCommand, String> {
    edit_command(project, segment_id, |m| m.zoom = zoom)
}

/// What a split does to the animation of the clip it cuts.
///
/// The left half keeps the entrance and the right half takes the exit; the
/// loop and the zoom go to both. The right half's zoom is already pushed in
/// when it starts — it is the same shot a frame later — so its ramp is
/// dropped. The right half is inserted *without* the original's reference
/// (`split_one` strips it), and these commands give each half its own.
///
/// Two seams are accepted rather than modelled: a Combo restarts its loop at
/// the cut, and a cut in the middle of a zoom ramp jumps to the full zoom.
pub fn split_commands(project: &Project, original: &Segment, right_id: &str) -> Vec<EditCommand> {
    let Some(material) = project.materials.animation_of(original) else {
        return Vec::new();
    };
    let mut commands = Vec::new();

    let mut left = material.clone();
    left.outro = None;
    left.text_out = None;
    if !left.same_parameters(material) {
        left.id = new_id();
        commands.push(EditCommand::SetAnimation {
            segment_id: original.id.clone(),
            before: Some(material.clone()),
            after: (!left.is_empty()).then_some(left),
            slot: slot_of(original, Some(material)),
        });
    }

    let mut right = material.clone();
    right.id = new_id();
    right.intro = None;
    right.text_in = None;
    if let Some(zoom) = &mut right.zoom {
        zoom.duration = 0;
    }
    if !right.is_empty() {
        commands.push(EditCommand::SetAnimation {
            segment_id: right_id.to_string(),
            before: None,
            after: Some(right),
            slot: None,
        });
    }
    commands
}

/// Where `material`'s id sits in `segment`'s extras, for
/// `EditCommand::SetAnimation::slot`.
fn slot_of(segment: &Segment, material: Option<&AnimationMaterial>) -> Option<usize> {
    let id = &material?.id;
    segment.extras.iter().position(|e| e == id)
}

/// The clips cut from one take that sit end to end with `segment_id`: same
/// material, same lane, each starting where the last one ends.
pub fn jump_cut_run(project: &Project, segment_id: &str) -> Option<Vec<String>> {
    let (track, segment) = project.segment(segment_id)?;
    let index = track.segments.iter().position(|s| s.id == segment.id)?;
    let joins = |a: &Segment, b: &Segment| {
        a.material_id == b.material_id && a.target_range.end() == b.target_range.start
    };
    let mut first = index;
    while first > 0 && joins(&track.segments[first - 1], &track.segments[first]) {
        first -= 1;
    }
    let mut last = index;
    while last + 1 < track.segments.len() && joins(&track.segments[last], &track.segments[last + 1])
    {
        last += 1;
    }
    Some(
        track.segments[first..=last]
            .iter()
            .map(|s| s.id.clone())
            .collect(),
    )
}

/// Alternate 100 % and `amount` across the jump cuts around `segment_id`,
/// as one undo step: the first clip of the run stays wide, the second is
/// punched in, the third wide again, and so on.
///
/// The pivot is the selected clip's own, when it has a zoom, so a user who
/// put it on the speaker's face gets every punch on the face.
pub fn auto_zoom_command(
    project: &Project,
    segment_id: &str,
    amount: f32,
) -> Result<EditCommand, String> {
    if !amount.is_finite() || !(1.0..=4.0).contains(&amount) {
        return Err("the auto zoom must be between 100% and 400%".into());
    }
    let run = jump_cut_run(project, segment_id).ok_or("the clip is no longer on the timeline")?;
    if run.len() < 2 {
        return Err("auto zoom needs jump cuts: split the clip, or cut its silences, first".into());
    }
    let template = project
        .segment(segment_id)
        .and_then(|(_, s)| project.materials.animation_of(s))
        .and_then(|m| m.zoom)
        .unwrap_or_default();
    let punch = PunchZoom {
        amount,
        duration: 0,
        ..template
    };

    let mut commands = Vec::new();
    for (i, id) in run.iter().enumerate() {
        let wanted = (i % 2 == 1).then_some(punch);
        match set_zoom_command(project, id, wanted) {
            Ok(command) => commands.push(command),
            Err(e) if e == "nothing to change" => {}
            Err(e) => return Err(e),
        }
    }
    if commands.is_empty() {
        return Err("the jump cuts already alternate".into());
    }
    Ok(EditCommand::Composite {
        label: "Auto zoom".into(),
        commands,
    })
}

/// How long a clip's longest In/Out pair may be, for a duration slider: the
/// whole clip.
pub fn max_window(segment: &Segment) -> Micros {
    segment.target_range.duration
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::motion::pose::material_pose;
    use crate::modules::project::animation::{AnimationPreset as P, Ease};
    use crate::modules::project::{
        CanvasConfig, TextMaterial, TimeRange, Track, TrackKind, Transform,
    };
    use crate::modules::timeline::ops::split_at;
    use crate::modules::timeline::History;

    fn segment(material: &str, start: Micros, duration: Micros) -> Segment {
        Segment {
            id: new_id(),
            material_id: material.into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(start, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// One lane holding `count` 2-second clips cut from one take, end to end.
    fn project(count: usize) -> (Project, Vec<String>) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        let mut ids = Vec::new();
        for i in 0..count {
            let s = segment("take", i as Micros * 2_000_000, 2_000_000);
            ids.push(s.id.clone());
            track.segments.push(s);
        }
        project.tracks.push(track);
        (project, ids)
    }

    fn anim(preset: P, duration: Micros) -> ClipAnimation {
        ClipAnimation {
            preset,
            duration,
            easing: Ease::Linear,
            strength: 1.0,
        }
    }

    fn json(project: &Project) -> String {
        serde_json::to_string(project).unwrap()
    }

    fn material<'a>(project: &'a Project, id: &str) -> Option<&'a AnimationMaterial> {
        let (_, s) = project.segment(id)?;
        project.materials.animation_of(s)
    }

    #[test]
    fn setting_an_animation_undoes_to_the_identical_document() {
        let (mut project, ids) = project(1);
        let original = json(&project);
        let mut history = History::new();

        let add = set_slot_command(
            &project,
            &ids[0],
            AnimationSlot::In,
            Some(anim(P::Pop, 400_000)),
        )
        .unwrap();
        history.apply(&mut project, add).unwrap();
        assert_eq!(project.materials.animations.len(), 1);
        let first = json(&project);

        let change = set_slot_command(
            &project,
            &ids[0],
            AnimationSlot::Out,
            Some(anim(P::Fade, 300_000)),
        )
        .unwrap();
        history.apply(&mut project, change).unwrap();
        // The old material is gone, not left behind: nothing refers to it.
        assert_eq!(project.materials.animations.len(), 1);
        let m = material(&project, &ids[0]).unwrap();
        assert_eq!(m.intro.unwrap().preset, P::Pop);
        assert_eq!(m.outro.unwrap().preset, P::Fade);

        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), first);
        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), original);
        // And a project that never had an animation saves without the key.
        assert!(!original.contains("animations"));
    }

    #[test]
    fn clearing_the_last_slot_removes_the_reference_and_the_material() {
        let (mut project, ids) = project(1);
        set_slot_command(
            &project,
            &ids[0],
            AnimationSlot::Combo,
            Some(anim(P::Pulse, 1_000_000)),
        )
        .unwrap()
        .apply(&mut project)
        .unwrap();
        set_slot_command(&project, &ids[0], AnimationSlot::Combo, None)
            .unwrap()
            .apply(&mut project)
            .unwrap();
        assert!(project.materials.animations.is_empty());
        assert!(project.tracks[0].segments[0].extras.is_empty());
        assert_eq!(
            set_slot_command(&project, &ids[0], AnimationSlot::Combo, None).unwrap_err(),
            "nothing to change"
        );
    }

    #[test]
    fn a_stale_edit_is_refused() {
        let (mut project, ids) = project(1);
        let first = set_slot_command(
            &project,
            &ids[0],
            AnimationSlot::In,
            Some(anim(P::Fade, 400_000)),
        )
        .unwrap();
        let second = set_slot_command(
            &project,
            &ids[0],
            AnimationSlot::In,
            Some(anim(P::Spin, 400_000)),
        )
        .unwrap();
        first.apply(&mut project).unwrap();
        assert!(second.apply(&mut project).is_err());
    }

    #[test]
    fn a_shared_material_survives_the_other_clip_changing() {
        let (mut project, ids) = project(2);
        set_slot_command(
            &project,
            &ids[0],
            AnimationSlot::In,
            Some(anim(P::Fade, 400_000)),
        )
        .unwrap()
        .apply(&mut project)
        .unwrap();
        // A duplicated clip carries the same reference.
        let shared = project.tracks[0].segments[0].extras[0].clone();
        project.tracks[0].segments[1].extras.push(shared.clone());
        let before = json(&project);

        let change = set_slot_command(
            &project,
            &ids[1],
            AnimationSlot::In,
            Some(anim(P::Spin, 400_000)),
        )
        .unwrap();
        change.apply(&mut project).unwrap();
        assert_eq!(material(&project, &ids[0]).unwrap().id, shared);
        assert_eq!(
            material(&project, &ids[0]).unwrap().intro.unwrap().preset,
            P::Fade
        );
        assert_eq!(
            material(&project, &ids[1]).unwrap().intro.unwrap().preset,
            P::Spin
        );
        assert_eq!(project.materials.animations.len(), 2);
        change.invert().apply(&mut project).unwrap();
        assert_eq!(json(&project), before);
    }

    #[test]
    fn an_animation_survives_trimming_the_tail() {
        let (mut project, ids) = project(1);
        let id = &ids[0];
        set_slot_command(
            &project,
            id,
            AnimationSlot::Out,
            Some(anim(P::Fade, 500_000)),
        )
        .unwrap()
        .apply(&mut project)
        .unwrap();
        let (_, s) = project.segment(id).unwrap();
        EditCommand::TrimSegment {
            segment_id: id.clone(),
            before_target: s.target_range,
            before_source: s.source_range,
            after_target: TimeRange::new(0, 1_200_000),
            after_source: TimeRange::new(0, 1_200_000),
        }
        .apply(&mut project)
        .unwrap();
        let (_, s) = project.segment(id).unwrap();
        let m = project.materials.animation_of(s).unwrap();
        let d = s.target_range.duration;
        // The fade-out moved with the new end: half-way through its window
        // at 0.95 s, gone at 1.2 s, untouched before 0.7 s.
        assert!((material_pose(m, 950_000, d).opacity - 0.5).abs() < 1e-4);
        assert!(material_pose(m, 1_200_000, d).opacity.abs() < 1e-4);
        assert_eq!(material_pose(m, 600_000, d).opacity, 1.0);
    }

    #[test]
    fn a_split_keeps_the_entrance_left_and_gives_the_exit_right() {
        let (mut project, ids) = project(1);
        let id = &ids[0];
        let mut history = History::new();
        let command = edit_command(&project, id, |m| {
            m.intro = Some(anim(P::Pop, 400_000));
            m.outro = Some(anim(P::Fade, 400_000));
            m.combo = Some(anim(P::Rock, 1_000_000));
            m.zoom = Some(PunchZoom {
                amount: 1.2,
                pivot: [0.1, 0.2],
                duration: 300_000,
                easing: Ease::Linear,
            });
        })
        .unwrap();
        history.apply(&mut project, command).unwrap();
        let before = json(&project);

        let split = split_at(&project, id, 1_000_000).unwrap();
        history.apply(&mut project, split).unwrap();
        let segments = &project.tracks[0].segments;
        assert_eq!(segments.len(), 2);
        let left = project.materials.animation_of(&segments[0]).unwrap();
        let right = project.materials.animation_of(&segments[1]).unwrap();
        assert_ne!(left.id, right.id);
        assert_eq!(left.intro.unwrap().preset, P::Pop);
        assert!(left.outro.is_none());
        assert!(right.intro.is_none());
        assert_eq!(right.outro.unwrap().preset, P::Fade);
        assert!(left.combo.is_some() && right.combo.is_some());
        assert_eq!(left.zoom.unwrap().duration, 300_000);
        assert_eq!(right.zoom.unwrap().duration, 0);
        assert_eq!(right.zoom.unwrap().pivot, [0.1, 0.2]);
        // Exactly one reference each, and nothing orphaned in the pool.
        assert_eq!(segments[1].extras.len(), 1);
        assert_eq!(project.materials.animations.len(), 2);

        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), before);
    }

    #[test]
    fn auto_zoom_alternates_across_the_jump_cuts_as_one_step() {
        let (mut project, ids) = project(4);
        // A clip on its own after a gap is not part of the run.
        let lone = segment("take", 10_000_000, 1_000_000);
        project.tracks[0].segments.push(lone);
        let before = json(&project);
        let mut history = History::new();
        let command = auto_zoom_command(&project, &ids[2], 1.1).unwrap();
        history.apply(&mut project, command).unwrap();
        let zoom = |i: usize| {
            project
                .materials
                .animation_of(&project.tracks[0].segments[i])
                .and_then(|m| m.zoom)
                .map(|z| z.amount)
        };
        assert_eq!(zoom(0), None);
        assert_eq!(zoom(1), Some(1.1));
        assert_eq!(zoom(2), None);
        assert_eq!(zoom(3), Some(1.1));
        assert_eq!(zoom(4), None);
        assert_eq!(history.undo_label().as_deref(), Some("Auto zoom"));
        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), before);

        let (single, ids) = project_alone();
        assert!(auto_zoom_command(&single, &ids, 1.1).is_err());
    }

    fn project_alone() -> (Project, String) {
        let (project, ids) = project(1);
        (project, ids[0].clone())
    }

    #[test]
    fn text_animation_is_for_text_clips_only() {
        let (mut project, ids) = project(1);
        let animator = crate::modules::motion::catalog::text_preset(
            crate::modules::project::animation::TextPreset::Pop,
        )
        .animator;
        assert!(set_text_command(&project, &ids[0], TextSlot::In, Some(animator)).is_err());
        project.materials.texts.push(TextMaterial {
            id: "take".into(),
            ..crate::modules::text::default_material(&project, Some("hi".into()))
        });
        assert!(set_text_command(&project, &ids[0], TextSlot::In, Some(animator)).is_ok());
    }

    #[test]
    fn a_project_with_animations_round_trips_through_json() {
        let (mut project, ids) = project(1);
        edit_command(&project, &ids[0], |m| {
            m.intro = Some(anim(P::WipeRight, 600_000));
            m.zoom = Some(PunchZoom::default());
        })
        .unwrap()
        .apply(&mut project)
        .unwrap();
        let text = serde_json::to_string_pretty(&project).unwrap();
        let back: Project = serde_json::from_str(&text).unwrap();
        assert_eq!(back.materials.animations, project.materials.animations);
        assert_eq!(serde_json::to_string_pretty(&back).unwrap(), text);
    }
}
