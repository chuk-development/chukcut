//! The transition half of `Project::validate()`.
//!
//! A transition is the only thing in the document whose correctness depends on
//! two segments at once, which makes it the only thing that can be broken by an
//! edit to something else. Deleting the outgoing clip, moving it away, or
//! trimming it so the two no longer meet all leave a transition describing a
//! join that no longer exists — an **orphan**. Nothing renders it, so the frame
//! is fine; but the timeline still draws a transition marker for it and the
//! user cannot get rid of a thing they cannot see, which is why this is an
//! error and not a shrug.
//!
//! The severity split follows `docs/architecture/project-format.md`: an error
//! means the document is internally inconsistent and an edit command has a bug;
//! a warning means the document is fine and the world is awkward.

use crate::modules::project::document::{Project, Severity, ValidationIssue};

use super::resolve::max_duration;

pub fn issues(project: &Project) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let error = |message: String, subject: &str| ValidationIssue {
        severity: Severity::Error,
        message,
        subject_id: Some(subject.to_string()),
    };

    for track in &project.tracks {
        for (index, segment) in track.segments.iter().enumerate() {
            let attached: Vec<_> = segment
                .extras
                .iter()
                .filter(|id| project.materials.transition(id).is_some())
                .collect();

            if attached.len() > 1 {
                issues.push(error(
                    format!(
                        "clip carries {} transitions; it may have one",
                        attached.len()
                    ),
                    &segment.id,
                ));
            }

            let Some(material) = project.materials.transition_of(segment) else {
                continue;
            };

            if material.duration <= 0 {
                issues.push(error(
                    "transition has a non-positive duration".into(),
                    &segment.id,
                ));
            }

            // The orphan check, in its two shapes: nothing before this clip at
            // all, or something before it that no longer reaches it.
            let Some(previous) = index.checked_sub(1).and_then(|i| track.segments.get(i)) else {
                issues.push(error(
                    "transition has no clip before it to come from".into(),
                    &segment.id,
                ));
                continue;
            };
            if previous.target_range.end() != segment.target_range.start {
                issues.push(error(
                    format!(
                        "transition's neighbours no longer touch: a gap of {} µs sits between them",
                        segment.target_range.start - previous.target_range.end()
                    ),
                    &segment.id,
                ));
                continue;
            }

            let limit = max_duration(previous, segment);
            if material.duration > limit {
                // Not an error: a legal trim can produce this, and the renderer
                // clamps the window rather than misbehaving. The UI wants to
                // say so, which is what a warning is for.
                issues.push(ValidationIssue {
                    severity: Severity::Warning,
                    message: format!(
                        "transition is longer than the clips it joins and will play as {:.2} s",
                        limit as f64 / 1_000_000.0
                    ),
                    subject_id: Some(segment.id.clone()),
                });
            }
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Micros, Segment, TimeRange, Track, TrackKind, Transform, TransitionKind,
        TransitionMaterial, VideoMaterial,
    };
    use crate::modules::project::new_id;

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
        let make = |start: Micros| Segment {
            id: new_id(),
            material_id: "m".into(),
            target_range: TimeRange::new(start, 4_000_000),
            source_range: TimeRange::new(start, 4_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        let (left, right) = (make(0), make(4_000_000));
        let (left_id, right_id) = (left.id.clone(), right.id.clone());
        let track_id = track.id.clone();
        track.segments.push(left);
        track.segments.push(right);
        project.tracks.push(track);
        (project, track_id, left_id, right_id)
    }

    fn attach(project: &mut Project, segment_id: &str, duration: Micros) -> String {
        let material = TransitionMaterial::new(TransitionKind::Dissolve, duration);
        let id = material.id.clone();
        project.materials.transitions.push(material);
        project
            .segment_mut(segment_id)
            .unwrap()
            .extras
            .push(id.clone());
        id
    }

    fn errors(project: &Project) -> Vec<String> {
        issues(project)
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.message)
            .collect()
    }

    #[test]
    fn a_docked_pair_is_clean() {
        let (mut project, _, _, right_id) = cut_project();
        attach(&mut project, &right_id, 1_000_000);
        assert!(errors(&project).is_empty());
        assert!(issues(&project).is_empty());
    }

    #[test]
    fn a_transition_whose_neighbours_stopped_touching_is_an_error() {
        let (mut project, _, left_id, right_id) = cut_project();
        attach(&mut project, &right_id, 1_000_000);
        project.segment_mut(&left_id).unwrap().target_range.duration = 3_000_000;

        let errors = errors(&project);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("no longer touch"), "{errors:?}");
    }

    #[test]
    fn a_transition_on_the_first_clip_of_a_track_is_an_error() {
        let (mut project, _, left_id, _) = cut_project();
        attach(&mut project, &left_id, 1_000_000);
        let errors = errors(&project);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("no clip before it"), "{errors:?}");
    }

    #[test]
    fn deleting_the_outgoing_clip_leaves_an_orphan_validate_can_see() {
        let (mut project, track_id, left_id, right_id) = cut_project();
        attach(&mut project, &right_id, 1_000_000);
        project
            .track_mut(&track_id)
            .unwrap()
            .segments
            .retain(|s| s.id != left_id);

        assert!(errors(&project)
            .iter()
            .any(|m| m.contains("no clip before it")));
    }

    #[test]
    fn two_transitions_on_one_clip_are_an_error() {
        let (mut project, _, _, right_id) = cut_project();
        attach(&mut project, &right_id, 1_000_000);
        attach(&mut project, &right_id, 1_000_000);
        assert!(errors(&project).iter().any(|m| m.contains("may have one")));
    }

    #[test]
    fn an_over_long_transition_is_only_a_warning() {
        let (mut project, _, _, right_id) = cut_project();
        attach(&mut project, &right_id, 20_000_000);
        assert!(errors(&project).is_empty());
        let issues = issues(&project);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Warning);
        assert!(issues[0].message.contains("8.00 s"), "{:?}", issues[0]);
    }

    #[test]
    fn an_id_that_resolves_to_nothing_is_simply_not_a_transition() {
        // `Segment::extras` carries ids of several kinds and this module is
        // only responsible for its own; an unknown id belongs to whoever owns
        // the category it was meant to be in.
        let (mut project, _, _, right_id) = cut_project();
        project
            .segment_mut(&right_id)
            .unwrap()
            .extras
            .push("not-a-transition".into());
        assert!(issues(&project).is_empty());
    }
}
