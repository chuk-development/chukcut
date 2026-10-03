//! The tracking half of `Project::validate()`, and the one rule the inspector
//! and the delete prompt share: whether a follow link can still find what it
//! follows.
//!
//! A follow link names two things another edit can delete: its track (a pool
//! material) and its target clip. Losing the named clip alone is harmless when
//! another clip of the tracked file is on the timeline —
//! `follow::target_segment` uses any clip of that file that covers the instant
//! — so only a link with no clip of its file anywhere is reported. Warnings,
//! not errors: the document is consistent, the overlay just stops moving, and
//! undoing the delete brings the motion back.

use std::collections::BTreeSet;

use crate::modules::project::document::{Id, Project, Severity, ValidationIssue};

use super::model::FollowMaterial;

/// Whether a follow link still has something to follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkStatus {
    Ok,
    /// The tracking material is gone from the pool.
    TrackMissing,
    /// The named clip is gone, and no clip of the tracked file is left to
    /// stand in for it.
    TargetMissing,
}

impl LinkStatus {
    pub fn is_broken(self) -> bool {
        self != LinkStatus::Ok
    }
}

pub fn link_status(project: &Project, link: &FollowMaterial) -> LinkStatus {
    let Some(track) = project.materials.tracking(&link.track_id) else {
        return LinkStatus::TrackMissing;
    };
    let named = project
        .segment(&link.target_segment_id)
        .is_some_and(|(_, s)| s.material_id == track.media_id);
    let stand_in = project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .any(|s| s.material_id == track.media_id);
    if named || stand_in {
        LinkStatus::Ok
    } else {
        LinkStatus::TargetMissing
    }
}

/// One warning per clip whose follow link is broken.
pub fn issues(project: &Project) -> Vec<ValidationIssue> {
    if project.materials.follows.is_empty() {
        return Vec::new();
    }
    let mut issues = Vec::new();
    for segment in project.tracks.iter().flat_map(|t| t.segments.iter()) {
        let Some(link) = project.materials.follow_of(segment) else {
            continue;
        };
        let message = match link_status(project, link) {
            LinkStatus::Ok => continue,
            LinkStatus::TrackMissing => "clip follows a track that is no longer in the project",
            LinkStatus::TargetMissing => {
                "clip follows a tracked video that is no longer on the timeline"
            }
        };
        issues.push(ValidationIssue {
            severity: Severity::Warning,
            message: message.into(),
            subject_id: Some(segment.id.clone()),
        });
    }
    issues
}

/// The overlays that would lose what they follow if `deleted` went: they
/// follow one of those clips by name, or a track of a file one of those clips
/// shows. Overlays that are themselves being deleted are left out — there is
/// nothing to keep for them.
///
/// Deliberately broad: a split half of the same file may still stand in for
/// part of the overlay's length, but not for the part the deleted clip
/// covered, and baking an overlay that would have kept following costs
/// nothing visible.
pub fn dependent_followers(project: &Project, deleted: &[Id]) -> Vec<Id> {
    let deleted_set: BTreeSet<&str> = deleted.iter().map(String::as_str).collect();
    let media: BTreeSet<&str> = deleted
        .iter()
        .filter_map(|id| project.segment(id))
        .filter(|(_, s)| project.materials.video(&s.material_id).is_some())
        .map(|(_, s)| s.material_id.as_str())
        .collect();
    if media.is_empty() {
        return Vec::new();
    }
    project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .filter(|s| !deleted_set.contains(s.id.as_str()))
        .filter(|s| {
            project.materials.follow_of(s).is_some_and(|link| {
                deleted_set.contains(link.target_segment_id.as_str())
                    || project
                        .materials
                        .tracking(&link.track_id)
                        .is_some_and(|t| media.contains(t.media_id.as_str()))
            })
        })
        .map(|s| s.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::timeline::ops::EditCommand;

    fn fixture() -> Project {
        super::super::follow::tests::project()
    }

    fn remove_video(p: &Project) -> EditCommand {
        let (track, segment) = p.segment("v").unwrap();
        EditCommand::RemoveSegment {
            track_id: track.id.clone(),
            segment: segment.clone(),
            index: 0,
        }
    }

    fn warnings(p: &Project) -> Vec<String> {
        p.validate()
            .into_iter()
            .filter(|i| i.severity == Severity::Warning && i.subject_id.as_deref() == Some("o"))
            .map(|i| i.message)
            .collect()
    }

    #[test]
    fn a_follower_whose_tracked_clip_was_deleted_is_reported() {
        let mut p = fixture();
        assert!(warnings(&p).is_empty());
        remove_video(&p).apply(&mut p).unwrap();
        let link = p.materials.follow("follow").unwrap().clone();
        assert_eq!(link_status(&p, &link), LinkStatus::TargetMissing);
        assert_eq!(warnings(&p).len(), 1, "{:?}", p.validate());
    }

    #[test]
    fn another_clip_of_the_tracked_file_stands_in() {
        let mut p = fixture();
        let mut copy = p.segment("v").unwrap().1.clone();
        copy.id = "v2".into();
        copy.target_range.start = 20_000_000;
        p.tracks[0].segments.push(copy);
        remove_video(&p).apply(&mut p).unwrap();
        assert!(warnings(&p).is_empty());
    }

    #[test]
    fn a_follower_whose_track_was_deleted_is_reported() {
        let mut p = fixture();
        p.materials.trackings.clear();
        let link = p.materials.follow("follow").unwrap().clone();
        assert_eq!(link_status(&p, &link), LinkStatus::TrackMissing);
        assert_eq!(warnings(&p).len(), 1);
    }

    #[test]
    fn deleting_the_tracked_clip_names_its_followers() {
        let p = fixture();
        assert_eq!(
            dependent_followers(&p, &["v".into()]),
            vec!["o".to_string()]
        );
        // Deleting the follower itself, or an unrelated clip, asks nothing.
        assert!(dependent_followers(&p, &["v".into(), "o".into()]).is_empty());
        assert!(dependent_followers(&p, &["o".into()]).is_empty());
    }
}
