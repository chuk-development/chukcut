//! What `EditCommand::SetSpeedCurve` does, and the commands the UI builds.

use crate::modules::project::document::{
    new_id, source_duration_for, speed_slack, Micros, Project, Segment, TimeRange, Track,
};
use crate::modules::project::speed::{
    check_curve_ranges, curve_target_duration, custom_points, SpeedCurveMaterial, SpeedPoint,
    SpeedPreset,
};
use crate::modules::timeline::ops::EditCommand;

/// The body of `EditCommand::SetSpeedCurve`.
///
/// Checked, then applied pool first, so the document never references a curve
/// it lacks; the old curve leaves the pool when nothing references it any
/// more, which makes the command and its inverse exact opposites.
pub fn set(
    project: &mut Project,
    segment_id: &str,
    before: Option<&SpeedCurveMaterial>,
    after: Option<&SpeedCurveMaterial>,
    before_target: TimeRange,
    after_target: TimeRange,
    slot: Option<usize>,
) -> Result<(), String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let current = project
        .materials
        .speed_curve_of(segment)
        .map(|m| m.id.clone());
    if current.as_deref() != before.map(|m| m.id.as_str()) {
        return Err("the clip's speed curve changed since this edit was made".into());
    }
    if segment.target_range != before_target {
        return Err("the clip moved since this edit was made; try it again".into());
    }
    if after_target.duration <= 0 {
        return Err("a clip must be longer than nothing".into());
    }
    if after_target.start < 0 {
        return Err("a clip cannot start before the beginning of the timeline".into());
    }
    let source = segment.source_range;
    match after {
        Some(after) => {
            if let Some(problem) = after.problem() {
                return Err(problem);
            }
            if let Some(existing) = project.materials.speed_curve(&after.id) {
                // A curve is never edited in place (two clips may share it),
                // so an id that exists must be exactly this curve.
                if existing != after {
                    return Err(format!("speed curve {} already exists", after.id));
                }
            }
            check_curve_ranges(after, after_target, source)?;
        }
        None => {
            let implied = source_duration_for(after_target.duration, segment.speed);
            if (source.duration - implied).abs() > speed_slack(segment.speed) {
                return Err(format!(
                    "without its speed curve the clip plays {} µs of material in {} µs at {}x, \
                     not {} µs",
                    implied, after_target.duration, segment.speed, source.duration
                ));
            }
        }
    }
    if !track.is_range_free(&after_target, Some(segment_id)) {
        return Err("the clip would overlap the one after it".into());
    }

    if let Some(after) = after {
        let pool = &mut project.materials.speed_curves;
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
    segment.target_range = after_target;

    if let Some(before) = before {
        let replaced = after.is_none_or(|a| a.id != before.id);
        if replaced && !referenced(project, &before.id) {
            project.materials.speed_curves.retain(|m| m.id != before.id);
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

/// What to do to a clip's speed curve.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurveChange {
    /// Lay a preset over the clip's current source range.
    Preset { preset: SpeedPreset },
    /// The flat curve "Custom" starts from, unless the clip already has a
    /// curve, which is then kept as the starting point.
    Custom,
    /// These points, as they are (absolute source microseconds).
    Points { points: Vec<SpeedPoint> },
    /// Back to the clip's constant speed.
    Remove,
}

/// The edit that gives `segment_id` — and every clip linked to it — a speed
/// curve, changes it, or takes it away: one `SetSpeedCurve` per clip, which
/// also sets the clip's new length, and the moves that keep everything after
/// it on its lanes where it was relative to the clip's end. One undo step.
///
/// Linked clips share the one curve: a picture ramped over its own sound at
/// constant speed is out of sync from the first ramp on. Their curve is
/// anchored to source time, so it fits each clip that reads the same part of
/// the file.
pub fn set_curve_command(
    project: &Project,
    segment_id: &str,
    change: CurveChange,
) -> Result<EditCommand, String> {
    let (_, primary) = project
        .segment(segment_id)
        .ok_or("the clip is no longer on the timeline")?;
    let before = project.materials.speed_curve_of(primary);
    let after: Option<SpeedCurveMaterial> = match change {
        CurveChange::Remove => None,
        CurveChange::Preset { preset } => Some(SpeedCurveMaterial {
            id: new_id(),
            preset: Some(preset),
            points: preset.points(primary.source_range),
        }),
        CurveChange::Custom => Some(SpeedCurveMaterial {
            id: new_id(),
            preset: None,
            points: match before {
                Some(curve) => curve.points.clone(),
                None => custom_points(primary.source_range),
            },
        }),
        CurveChange::Points { points } => Some(SpeedCurveMaterial {
            id: new_id(),
            preset: None,
            points,
        }),
    };
    if let Some(after) = &after {
        if let Some(problem) = after.problem() {
            return Err(problem);
        }
    }

    let members: Vec<(&Track, &Segment)> = match project.link_group_of(segment_id) {
        Some(group) => project
            .link_members(group)
            .into_iter()
            .map(|(track, _, segment)| (track, segment))
            .collect(),
        None => vec![project.segment(segment_id).expect("found above")],
    };

    let length = |segment: &Segment| -> Micros {
        match &after {
            Some(curve) => curve_target_duration(&curve.points, segment.source_range).max(1),
            None => {
                let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
                    segment.speed as f64
                } else {
                    1.0
                };
                ((segment.source_range.duration as f64 / speed).round() as Micros).max(1)
            }
        }
    };

    let unchanged = members.iter().all(|(_, segment)| {
        let had = project.materials.speed_curve_of(segment);
        let same_curve = match (had, &after) {
            (None, None) => true,
            (Some(a), Some(b)) => a.points == b.points && a.preset == b.preset,
            _ => false,
        };
        same_curve && segment.target_range.duration == length(segment)
    });
    if unchanged {
        return Err(match after {
            None => "the clip has no speed curve".into(),
            Some(_) => "the clip already has this speed curve".into(),
        });
    }

    let delta = length(primary) - primary.target_range.duration;
    let moves = crate::modules::inspector::edit::ripple_moves(project, &members, delta);

    let mut sets = Vec::with_capacity(members.len());
    for (_, segment) in &members {
        let had = project.materials.speed_curve_of(segment).cloned();
        let after_target = TimeRange::new(segment.target_range.start, length(segment));
        if had.is_none() && after.is_none() {
            continue;
        }
        sets.push(EditCommand::SetSpeedCurve {
            segment_id: segment.id.clone(),
            slot: had
                .as_ref()
                .and_then(|c| segment.extras.iter().position(|id| *id == c.id)),
            before: had,
            after: after.clone(),
            before_target: segment.target_range,
            after_target,
        });
    }

    let label = match (&before, &after) {
        (_, None) => "Remove speed curve",
        (None, Some(_)) => "Add speed curve",
        (Some(_), Some(_)) => "Change speed curve",
    };
    let commands = if delta > 0 {
        moves.into_iter().chain(sets).collect()
    } else {
        sets.into_iter().chain(moves).collect()
    };
    Ok(EditCommand::Composite {
        label: label.into(),
        commands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, TrackKind, Transform};
    use crate::modules::project::speed::SpeedPreset;
    use crate::modules::timeline::History;

    fn clip(start: Micros, source: TimeRange, speed: f32) -> Segment {
        Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(
                start,
                (source.duration as f64 / speed as f64).round() as Micros,
            ),
            source_range: source,
            render_index: 0,
            speed,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// One lane: a 4 s clip, then a 2 s clip right after it.
    fn project() -> (Project, String, String) {
        let mut project = Project::new("t", CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        let a = clip(0, TimeRange::new(1_000_000, 4_000_000), 1.0);
        let b = clip(4_000_000, TimeRange::new(0, 2_000_000), 1.0);
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        track.segments.push(a);
        track.segments.push(b);
        project.tracks.push(track);
        (project, a_id, b_id)
    }

    fn json(project: &Project) -> String {
        serde_json::to_string(project).unwrap()
    }

    #[test]
    fn a_preset_sets_the_length_to_the_integral_and_moves_what_follows() {
        let (mut project, a, b) = project();
        let original = json(&project);
        let mut history = History::default();
        let command = set_curve_command(
            &project,
            &a,
            CurveChange::Preset {
                preset: SpeedPreset::Hero,
            },
        )
        .unwrap();
        history.apply(&mut project, command).unwrap();

        let (_, seg) = project.segment(&a).unwrap();
        let curve = project.materials.speed_curve_of(seg).unwrap();
        let expected = curve_target_duration(&curve.points, seg.source_range);
        assert_eq!(seg.target_range.duration, expected);
        assert_eq!(seg.source_range, TimeRange::new(1_000_000, 4_000_000));
        assert_eq!(
            project.segment(&b).unwrap().1.target_range.start,
            seg.target_range.end(),
            "the next clip still butts against the ramped one"
        );
        assert!(project
            .validate()
            .iter()
            .all(|i| i.severity != crate::modules::project::Severity::Error));

        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), original, "undo is exact");
        history.redo(&mut project).unwrap();
        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), original);
    }

    #[test]
    fn removing_the_curve_returns_the_constant_speed_length() {
        let (mut project, a, _) = project();
        let mut history = History::default();
        let add = set_curve_command(
            &project,
            &a,
            CurveChange::Preset {
                preset: SpeedPreset::Montage,
            },
        )
        .unwrap();
        history.apply(&mut project, add).unwrap();
        let ramped = json(&project);
        let remove = set_curve_command(&project, &a, CurveChange::Remove).unwrap();
        history.apply(&mut project, remove).unwrap();
        let (_, seg) = project.segment(&a).unwrap();
        assert_eq!(seg.target_range.duration, 4_000_000);
        assert!(
            project.materials.speed_curves.is_empty(),
            "the pool is pruned"
        );
        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), ramped);
    }

    #[test]
    fn a_constant_speed_edit_takes_the_curve_away_in_one_step() {
        let (mut project, a, b) = project();
        let mut history = History::default();
        let add = set_curve_command(
            &project,
            &a,
            CurveChange::Preset {
                preset: SpeedPreset::FlashIn,
            },
        )
        .unwrap();
        history.apply(&mut project, add).unwrap();
        let ramped = json(&project);
        let command =
            crate::modules::inspector::edit::set_speed_command(&project, &a, 2.0).unwrap();
        history.apply(&mut project, command).unwrap();
        let (_, seg) = project.segment(&a).unwrap();
        assert!(project.materials.speed_curve_of(seg).is_none());
        assert_eq!(seg.speed, 2.0);
        assert_eq!(seg.target_range.duration, 2_000_000);
        assert_eq!(project.segment(&b).unwrap().1.target_range.start, 2_000_000);
        history.undo(&mut project).unwrap();
        assert_eq!(json(&project), ramped);
    }

    #[test]
    fn an_out_of_range_curve_is_refused() {
        let (project, a, _) = project();
        let bad = set_curve_command(
            &project,
            &a,
            CurveChange::Points {
                points: vec![SpeedPoint {
                    source: 0,
                    speed: 50.0,
                }],
            },
        );
        assert!(bad.is_err());
    }
}
