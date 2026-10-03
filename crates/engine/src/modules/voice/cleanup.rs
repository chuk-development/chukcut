//! The per-clip voice cleanup settings, and how the mixers read them.
//!
//! ## Where it lives
//!
//! A parameter block in `MaterialPool::extras` under the key
//! `"voice_cleanup"`, referenced by id from `Segment::extras` — the home that
//! map exists for ("parameter blocks nothing hot reads"). Nothing hot reads
//! this: the preview mixer resolves it once per project snapshot when it
//! builds its plan, and the export mixer once per segment. The colour grade
//! needed a typed category because the compositor reads it every frame; this
//! does not, and an extras entry changes no struct another module constructs.
//!
//! Like colour, a block is **never edited in place**: every change mints a new
//! one and the undoable edit swaps the segment's reference over to it, so undo
//! is exact without a command that mutates the pool. A clip that is split
//! keeps the reference on both halves (splitting clones `extras`), which is
//! right — both halves are the same recording.
//!
//! ## What the mixers do with it
//!
//! [`effective_source`] answers, for one segment, which file to read and what
//! extra gain to apply. Preview and export both call it, so a cleaned clip
//! sounds the same in both — the one rule this feature cannot break.

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{new_id, Project, Segment, TrackKind};
use crate::modules::timeline::ops::EditCommand;

use super::denoise;

/// The key of a cleanup block inside its `MaterialPool::extras` value.
pub const KEY: &str = "voice_cleanup";

/// Everything "Enhance voice" can do to one clip.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VoiceCleanup {
    /// Noise reduction, rendered to a cached file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denoise: Option<Denoise>,
    /// A gain that brings the clip to a loudness target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalize: Option<Normalize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Denoise {
    /// `0..=1`: how much of the denoised signal replaces the original.
    pub strength: f32,
    /// Which engine and model rendered it. The document records it so a
    /// missing cache is rebuilt with the same one rather than whatever the
    /// current build ships (ml-features §5.6).
    pub engine: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Normalize {
    /// Integrated loudness the clip was brought to, in LUFS.
    pub target_lufs: f32,
    /// The gain that does it, measured once when normalising. Stored rather
    /// than re-measured on every play, so the clip does not change level
    /// because a cache was rebuilt.
    pub gain_db: f32,
    /// Integrated loudness before the gain, for the panel's readout.
    #[serde(default)]
    pub measured_lufs: Option<f32>,
}

impl VoiceCleanup {
    pub fn is_identity(&self) -> bool {
        self.denoise.is_none() && self.normalize.is_none()
    }

    /// The linear gain the normalisation adds, `1.0` when there is none.
    pub fn gain(&self) -> f32 {
        self.normalize
            .as_ref()
            .filter(|n| n.gain_db.is_finite())
            .map(|n| 10f32.powf(n.gain_db / 20.0))
            .unwrap_or(1.0)
    }

    fn to_value(&self) -> serde_json::Value {
        serde_json::json!({ KEY: self })
    }
}

/// The cleanup block an extras id resolves to, if it is one.
pub fn cleanup_entry(project: &Project, id: &str) -> Option<VoiceCleanup> {
    project
        .materials
        .extras
        .get(id)
        .and_then(|value| value.get(KEY))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
}

/// The cleanup on `segment`, with the id that holds it.
pub fn cleanup_of(project: &Project, segment: &Segment) -> Option<(String, VoiceCleanup)> {
    segment
        .extras
        .iter()
        .find_map(|id| cleanup_entry(project, id).map(|c| (id.clone(), c)))
}

/// The segment that actually plays `segment_id`'s sound.
///
/// A file imported with picture and sound is two linked segments, and only
/// the one on the audio lane is heard (`Project::sound_is_on_a_linked_lane`).
/// Cleaning the picture's segment would change nothing audible, so every
/// command here is aimed at the audible one whichever of the two is selected.
pub fn audible_segment(project: &Project, segment_id: &str) -> Option<String> {
    let (track, segment) = project.segment(segment_id)?;
    if !project.sound_is_on_a_linked_lane(track, segment) {
        return Some(segment_id.to_string());
    }
    let group = project.link_group_of(segment_id)?;
    project
        .link_members(group)
        .into_iter()
        .find(|(t, _, s)| t.kind == TrackKind::Audio && s.id != segment_id)
        .map(|(_, _, s)| s.id.clone())
}

/// The file a segment's sound is decoded from before any cleanup, and the
/// material's duration.
pub fn original_source(project: &Project, segment: &Segment) -> Option<(String, i64)> {
    if let Some(audio) = project.materials.audio(&segment.material_id) {
        return Some((audio.path.clone(), audio.duration));
    }
    project
        .materials
        .video(&segment.material_id)
        .filter(|video| video.has_audio)
        .map(|video| (video.path.clone(), video.duration))
}

/// What a mixer reads for one segment.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveSource {
    pub path: String,
    /// Linear gain on top of the segment's own volume.
    pub gain: f32,
}

/// Which file to decode for `segment` and what gain to add, given the path
/// the mixer resolved before cleanup existed.
///
/// A denoised clip whose cache file is missing plays the original rather than
/// silence; the export renders missing caches before it mixes
/// ([`denoise::ensure_rendered`]), so only the preview can ever take that
/// fallback, and only until the render the command started has finished.
pub fn effective_source(project: &Project, segment: &Segment, original: &str) -> EffectiveSource {
    let Some((_, cleanup)) = cleanup_of(project, segment) else {
        return EffectiveSource {
            path: original.to_string(),
            gain: 1.0,
        };
    };
    let path = cleanup
        .denoise
        .as_ref()
        .map(|d| denoise::cache_path(original, d.strength, &d.engine))
        .filter(|cached| cached.is_file())
        .map(|cached| cached.to_string_lossy().into_owned())
        .unwrap_or_else(|| original.to_string());
    EffectiveSource {
        path,
        gain: cleanup.gain(),
    }
}

/// The edit that sets `segment_id`'s cleanup to `cleanup` (or clears it), and
/// the extras entry to add to the pool before applying it.
///
/// Remove + insert of the same segment with its reference swapped, the shape
/// `inspector::edit` uses for crop and colour and for the same reason: both
/// primitives snapshot the whole segment, so the pair inverts exactly.
pub fn set_cleanup_command(
    project: &Project,
    segment_id: &str,
    cleanup: Option<VoiceCleanup>,
) -> Result<(Option<(String, serde_json::Value)>, EditCommand), String> {
    let (track, before) = project
        .segment(segment_id)
        .ok_or_else(|| format!("unknown segment {segment_id}"))?;
    let cleanup = cleanup.filter(|c| !c.is_identity());
    let current = cleanup_of(project, before).map(|(_, c)| c);
    if current == cleanup {
        return Err("the clip already sounds like that".into());
    }
    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment_id)
        .expect("segment found on this track");

    let entry = cleanup.map(|c| (new_id(), c.to_value()));
    let mut after = before.clone();
    after
        .extras
        .retain(|id| cleanup_entry(project, id).is_none());
    if let Some((id, _)) = &entry {
        after.extras.push(id.clone());
    }
    let label = if entry.is_some() {
        "Clean up voice"
    } else {
        "Remove voice cleanup"
    };
    let command = EditCommand::Composite {
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
    };
    Ok((entry, command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, TimeRange, Track, Transform, VideoMaterial,
    };
    use crate::modules::timeline::History;

    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "take".into(),
            path: "/nonexistent/take.mp4".into(),
            width: 1080,
            height: 1920,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        p.materials.links.insert("g".into());
        let segment = |id: &str| Segment {
            id: id.into(),
            material_id: "take".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: vec!["g".into()],
            keyframes: Vec::new(),
        };
        let mut v = Track::new(TrackKind::Video, "V1");
        v.segments.push(segment("picture"));
        let mut a = Track::new(TrackKind::Audio, "A1");
        a.segments.push(segment("sound"));
        p.tracks = vec![v, a];
        p
    }

    fn apply(p: &mut Project, history: &mut History, segment: &str, c: Option<VoiceCleanup>) {
        let (entry, command) = set_cleanup_command(p, segment, c).unwrap();
        if let Some((id, value)) = entry {
            p.materials.extras.insert(id, value);
        }
        history.apply(p, command).unwrap();
    }

    #[test]
    fn the_picture_of_a_linked_pair_sends_cleanup_to_its_sound() {
        let p = project();
        assert_eq!(audible_segment(&p, "picture").as_deref(), Some("sound"));
        assert_eq!(audible_segment(&p, "sound").as_deref(), Some("sound"));
    }

    #[test]
    fn a_cleanup_round_trips_through_the_document_and_undoes_exactly() {
        let mut p = project();
        let mut history = History::new();
        let cleanup = VoiceCleanup {
            denoise: None,
            normalize: Some(Normalize {
                target_lufs: -14.0,
                gain_db: 6.0,
                measured_lufs: Some(-20.0),
            }),
        };
        apply(&mut p, &mut history, "sound", Some(cleanup.clone()));
        let (_, sound) = p.segment("sound").unwrap();
        assert_eq!(cleanup_of(&p, sound).map(|(_, c)| c), Some(cleanup));
        // The link survives the remove + insert.
        assert!(p.materials.link_of(sound).is_some());

        let effective = effective_source(&p, sound, "/nonexistent/take.mp4");
        assert!((effective.gain - 1.995).abs() < 0.01);
        assert_eq!(effective.path, "/nonexistent/take.mp4");

        history.undo(&mut p).unwrap();
        let (_, sound) = p.segment("sound").unwrap();
        assert!(cleanup_of(&p, sound).is_none());
        assert_eq!(effective_source(&p, sound, "x").gain, 1.0);
    }

    #[test]
    fn setting_the_same_cleanup_twice_is_refused_and_identity_clears() {
        let mut p = project();
        let mut history = History::new();
        let cleanup = VoiceCleanup {
            denoise: Some(Denoise {
                strength: 0.8,
                engine: denoise::ENGINE.into(),
            }),
            normalize: None,
        };
        apply(&mut p, &mut history, "sound", Some(cleanup.clone()));
        assert!(set_cleanup_command(&p, "sound", Some(cleanup)).is_err());
        // A denoise without a cache falls back to the original file.
        let (_, sound) = p.segment("sound").unwrap();
        assert_eq!(effective_source(&p, sound, "/orig.wav").path, "/orig.wav");

        apply(&mut p, &mut history, "sound", Some(VoiceCleanup::default()));
        let (_, sound) = p.segment("sound").unwrap();
        assert!(cleanup_of(&p, sound).is_none());
    }
}
