//! A generator of plausible-but-adversarial edit commands.
//!
//! The commands it produces are shaped like the ones the UI sends — every
//! `before` field is read out of the live document, every id refers to
//! something that exists — but their *values* are chosen to collide: starts
//! land on a coarse grid so segments constantly try to occupy each other's
//! ranges, and durations are small enough that a track fills up. Overlap and
//! ordering bugs live exactly there.
//!
//! Deliberately not a generator of nonsense. A command naming a track that was
//! never in the document tests the error path and nothing else; a command that
//! *nearly* works is what finds the bug.

#![allow(dead_code)]

use chukcut_lib::modules::project::document::{
    AnimatableProperty, Crop, Easing, Keyframe, KeyframeTrack, Micros, Project, Segment, TimeRange,
    Track, TrackKind, Transform,
};
use chukcut_lib::modules::timeline::ops::{split_at, EditCommand, TrackFlags};

use super::Lcg;

/// Times are chosen on a 0.1 s grid inside this window, so segments are
/// forever landing on top of one another.
const GRID: Micros = 100_000;
const WINDOW: i64 = 300;

pub struct EditFuzzer {
    pub rng: Lcg,
    next_id: u64,
    /// Growth caps, so a long run does not turn into a memory test.
    pub max_tracks: usize,
    pub max_segments_per_track: usize,
}

impl EditFuzzer {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Lcg::new(seed),
            next_id: 0,
            max_tracks: 8,
            max_segments_per_track: 40,
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{}", self.next_id)
    }

    fn grid_time(&mut self) -> Micros {
        self.rng.between(0, WINDOW) * GRID
    }

    fn grid_duration(&mut self) -> Micros {
        self.rng.between(1, 12) * GRID
    }

    /// A random `(track index, segment index)` from the document, if it has
    /// any segments at all.
    fn some_segment(&mut self, project: &Project) -> Option<(usize, usize)> {
        let populated: Vec<usize> = project
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.segments.is_empty())
            .map(|(i, _)| i)
            .collect();
        let track = *self.rng.pick(&populated)?;
        let segment = self.rng.below(project.tracks[track].segments.len());
        Some((track, segment))
    }

    fn some_material(&mut self, project: &Project) -> Option<String> {
        let mut ids: Vec<String> = project
            .materials
            .videos
            .iter()
            .map(|m| m.id.clone())
            .collect();
        ids.extend(project.materials.images.iter().map(|m| m.id.clone()));
        ids.extend(project.materials.audios.iter().map(|m| m.id.clone()));
        ids.extend(project.materials.texts.iter().map(|m| m.id.clone()));
        self.rng.pick(&ids).cloned()
    }

    fn transform(&mut self) -> Transform {
        Transform {
            position: [self.rng.unit() * 2.0 - 1.0, self.rng.unit() * 2.0 - 1.0],
            scale: [0.1 + self.rng.unit() * 2.0, 0.1 + self.rng.unit() * 2.0],
            rotation: self.rng.between(-360, 360) as f32,
            opacity: self.rng.unit(),
            flip_h: self.rng.chance(2),
            flip_v: self.rng.chance(2),
        }
    }

    /// A fresh segment for `material_id`, occupying a random slot.
    pub fn new_segment(&mut self, material_id: &str) -> Segment {
        let start = self.grid_time();
        let duration = self.grid_duration();
        let mut segment = Segment {
            id: self.id("fuzz-seg"),
            material_id: material_id.to_string(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(self.grid_time(), duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        if self.rng.chance(4) {
            segment.transform = self.transform();
        }
        if self.rng.chance(6) {
            segment.crop = Some(Crop {
                left: self.rng.unit() * 0.4,
                top: self.rng.unit() * 0.4,
                right: 0.6 + self.rng.unit() * 0.4,
                bottom: 0.6 + self.rng.unit() * 0.4,
            });
        }
        if self.rng.chance(5) {
            segment.keyframes = vec![KeyframeTrack {
                property: AnimatableProperty::Opacity,
                keyframes: vec![
                    Keyframe {
                        time: 0,
                        value: 0.0,
                        easing: Easing::EaseInOut,
                    },
                    Keyframe {
                        time: duration,
                        value: 1.0,
                        easing: Easing::Linear,
                    },
                ],
            }];
        }
        segment
    }

    /// One command against `project`, or `None` when the document has nothing
    /// to operate on.
    pub fn next(&mut self, project: &Project) -> Option<EditCommand> {
        // Weighted by how often the UI actually issues each: moving, trimming
        // and transforming dominate, structural changes are rarer.
        match self.rng.below(100) {
            0..=6 => self.add_track(project),
            7..=10 => self.remove_track(project),
            11..=25 => self.insert_segment(project),
            26..=33 => self.remove_segment(project),
            34..=53 => self.move_segment(project),
            54..=71 => self.trim_segment(project),
            72..=80 => self.set_transform(project),
            81..=84 => self.set_speed(project),
            85..=88 => self.set_volume(project),
            89..=92 => self.set_track_flags(project),
            93..=96 => self.split(project),
            _ => self.composite(project),
        }
        // Falling back rather than returning nothing keeps a long run from
        // stalling on an empty document.
        .or_else(|| self.add_track(project))
    }

    fn add_track(&mut self, project: &Project) -> Option<EditCommand> {
        if project.tracks.len() >= self.max_tracks {
            return None;
        }
        let kinds = [
            TrackKind::Video,
            TrackKind::Audio,
            TrackKind::Text,
            TrackKind::Sticker,
        ];
        let kind = *self.rng.pick(&kinds)?;
        let name = self.id("Lane");
        let mut track = Track::new(kind, name);
        track.id = self.id("fuzz-track");
        Some(EditCommand::AddTrack {
            track,
            index: self.rng.below(project.tracks.len() + 1),
        })
    }

    fn remove_track(&mut self, project: &Project) -> Option<EditCommand> {
        if project.tracks.len() <= 1 {
            return None;
        }
        let index = self.rng.below(project.tracks.len());
        Some(EditCommand::RemoveTrack {
            track: project.tracks[index].clone(),
            index,
        })
    }

    fn insert_segment(&mut self, project: &Project) -> Option<EditCommand> {
        let track_index = self.rng.below(project.tracks.len().max(1));
        let track = project.tracks.get(track_index)?;
        if track.segments.len() >= self.max_segments_per_track {
            return None;
        }
        let material_id = self.some_material(project)?;
        let segment = self.new_segment(&material_id);
        Some(EditCommand::InsertSegment {
            track_id: track.id.clone(),
            segment,
            index: self.rng.below(track.segments.len() + 1),
        })
    }

    fn remove_segment(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        Some(EditCommand::RemoveSegment {
            track_id: project.tracks[track].id.clone(),
            segment: project.tracks[track].segments[index].clone(),
            index,
        })
    }

    fn move_segment(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        let segment = &project.tracks[track].segments[index];
        let destination = self.rng.below(project.tracks.len());
        Some(EditCommand::MoveSegment {
            segment_id: segment.id.clone(),
            from_track: project.tracks[track].id.clone(),
            to_track: project.tracks[destination].id.clone(),
            from_start: segment.target_range.start,
            to_start: self.grid_time(),
        })
    }

    fn trim_segment(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        let segment = &project.tracks[track].segments[index];
        let before_target = segment.target_range;
        let before_source = segment.source_range;

        // Three shapes of trim, because they exercise different code: dragging
        // an edge (target and source move together), sliding (target only), and
        // slipping (source only).
        let (after_target, after_source) = match self.rng.below(3) {
            0 => {
                let delta = self.rng.between(-5, 5) * GRID;
                (
                    TimeRange::new(
                        (before_target.start + delta).max(0),
                        (before_target.duration - delta).max(GRID),
                    ),
                    TimeRange::new(
                        (before_source.start + delta).max(0),
                        (before_source.duration - delta).max(GRID),
                    ),
                )
            }
            1 => (
                TimeRange::new(self.grid_time(), before_target.duration),
                before_source,
            ),
            _ => (
                before_target,
                TimeRange::new(self.grid_time(), before_source.duration),
            ),
        };

        Some(EditCommand::TrimSegment {
            segment_id: segment.id.clone(),
            before_target,
            before_source,
            after_target,
            after_source,
        })
    }

    fn set_transform(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        let segment = &project.tracks[track].segments[index];
        let before = segment.transform;
        Some(EditCommand::SetTransform {
            segment_id: segment.id.clone(),
            before,
            after: self.transform(),
        })
    }

    fn set_speed(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        let segment = &project.tracks[track].segments[index];
        Some(EditCommand::SetSpeed {
            segment_id: segment.id.clone(),
            before: segment.speed,
            // Occasionally zero or negative, which must be refused rather than
            // producing a segment whose source time runs backwards.
            after: if self.rng.chance(8) {
                self.rng.between(-2, 0) as f32
            } else {
                0.25 + self.rng.unit() * 3.75
            },
        })
    }

    fn set_volume(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        let segment = &project.tracks[track].segments[index];
        Some(EditCommand::SetVolume {
            segment_id: segment.id.clone(),
            before: segment.volume,
            after: self.rng.unit() * 4.0,
        })
    }

    fn set_track_flags(&mut self, project: &Project) -> Option<EditCommand> {
        let index = self.rng.below(project.tracks.len().max(1));
        let track = project.tracks.get(index)?;
        Some(EditCommand::SetTrackFlags {
            track_id: track.id.clone(),
            before: TrackFlags::of(track),
            after: TrackFlags {
                muted: self.rng.chance(2),
                locked: self.rng.chance(2),
                hidden: self.rng.chance(2),
                volume: self.rng.unit() * 4.0,
            },
        })
    }

    fn split(&mut self, project: &Project) -> Option<EditCommand> {
        let (track, index) = self.some_segment(project)?;
        let segment = &project.tracks[track].segments[index];
        let range = segment.target_range;
        if range.duration <= GRID {
            return None;
        }
        let at = range.start + self.rng.between(1, range.duration / GRID - 1) * GRID;
        split_at(project, &segment.id, at).ok()
    }

    /// Two or three commands bundled, which is how every higher-level edit is
    /// built. A composite that fails partway has to leave nothing behind.
    fn composite(&mut self, project: &Project) -> Option<EditCommand> {
        let count = self.rng.between(2, 3) as usize;
        let mut commands = Vec::with_capacity(count);
        for _ in 0..count {
            // Generated against the *pre-composite* document, so the later
            // parts routinely conflict with what the earlier ones did — which
            // is exactly the rollback path worth exercising.
            match self.rng.below(4) {
                0 => commands.extend(self.insert_segment(project)),
                1 => commands.extend(self.remove_segment(project)),
                2 => commands.extend(self.move_segment(project)),
                _ => commands.extend(self.trim_segment(project)),
            }
        }
        if commands.is_empty() {
            return None;
        }
        Some(EditCommand::Composite {
            label: "Fuzzed composite".into(),
            commands,
        })
    }
}
