//! "Save as template": a project becomes a template by turning the clips the
//! user picks into slots.
//!
//! Works on a copy; the open document is never touched. A chosen clip keeps
//! its place, length, transform, animations, effects, transitions,
//! keyframes and grade, and loses its media: the material becomes a
//! placeholder of the clip's displayed shape, the source range the
//! placeholder's, the speed 1 and the crop none (the placeholder already has
//! the cropped shape, so a filled slot lands where the clip was). A speed
//! ramp goes, as does the clip's linked sound — it belonged to the footage.
//! Media no clip uses any more is dropped from the pool, so the template
//! carries only what it plays.

use std::collections::HashSet;

use crate::modules::project::document::{new_id, ImageMaterial, Project, TimeRange, TrackKind};

use super::assets;
use super::fill::displayed_aspect;
use super::format::{id_from_name, TemplateFile};
use super::slot::{self, SlotMarker, SlotMedia};

/// What the user said about the template they are saving.
#[derive(Debug, Clone, Default)]
pub struct SaveRequest {
    pub name: String,
    pub description: String,
    pub category: String,
    /// The clips that become slots, in fill order. Empty means "the slots the
    /// project already has" (a project made from a template).
    pub slots: Vec<String>,
    /// A label per slot, by position in `slots`; missing or empty is none.
    pub labels: Vec<String>,
}

/// The template `request` describes, made from a copy of `project`. Touches
/// no file: drawing the placeholders it names (`assets::ensure_for`) and
/// bundling the media (`format::bundle_media`) are the caller's, because
/// they write.
pub fn make_template(project: &Project, request: &SaveRequest) -> Result<TemplateFile, String> {
    let name = request.name.trim();
    if name.is_empty() {
        return Err("a template needs a name".into());
    }
    let chosen: Vec<String> = if request.slots.is_empty() {
        slot::slots(project)
            .into_iter()
            .map(|s| s.segment_id)
            .collect()
    } else {
        request.slots.clone()
    };
    if chosen.is_empty() {
        return Err("choose at least one video or photo clip to become a slot".into());
    }
    let mut seen = HashSet::new();
    if let Some(twice) = chosen.iter().find(|id| !seen.insert(id.as_str())) {
        return Err(format!("the clip {twice} is chosen twice"));
    }

    let mut template = project.clone();
    template.id = new_id();
    template.name = name.to_string();
    template.canvas_chosen = true;
    template.markers.clear();

    let mut gone_links: Vec<String> = Vec::new();
    for (n, segment_id) in chosen.iter().enumerate() {
        let index = n as u32 + 1;
        let (track, segment) = template
            .segment(segment_id)
            .ok_or_else(|| format!("there is no clip {segment_id}"))?;
        let pool = &template.materials;
        let is_video = pool.video(&segment.material_id).is_some();
        if track.kind != TrackKind::Video
            || (!is_video && pool.image(&segment.material_id).is_none())
        {
            return Err(format!(
                "slot {index} must be a video or photo clip on a video lane"
            ));
        }
        let aspect = displayed_aspect(&template, segment);
        let existing = slot::marker_of(&template, segment).map(|(_, m)| m);
        let accepts = existing.as_ref().map_or(SlotMedia::Any, |m| m.accepts);
        let label = request
            .labels
            .get(n)
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .or_else(|| existing.and_then(|m| m.label));
        let link = segment
            .extras
            .iter()
            .find(|id| pool.links.contains(*id))
            .cloned();

        let path = assets::placeholder_path(index, aspect)
            .to_string_lossy()
            .into_owned();
        let (width, height) = assets::placeholder_size(aspect);
        let material_id = match template.materials.images.iter().find(|m| m.path == path) {
            Some(m) => m.id.clone(),
            None => {
                let id = new_id();
                template.materials.images.push(ImageMaterial {
                    id: id.clone(),
                    path,
                    width,
                    height,
                });
                id
            }
        };
        let marker = SlotMarker {
            index,
            label,
            aspect,
            accepts,
            filled: false,
        };
        let (marker_id, value) = marker.new_entry();
        template.materials.extras.insert(marker_id.clone(), value);

        let strip: HashSet<String> = {
            let pool = &template.materials;
            let (_, segment) = template.segment(segment_id).expect("found above");
            segment
                .extras
                .iter()
                .filter(|id| {
                    slot::is_marker(&template, id)
                        || pool.speed_curve(id).is_some()
                        || pool.links.contains(*id)
                })
                .cloned()
                .collect()
        };
        let segment = template
            .tracks
            .iter_mut()
            .flat_map(|t| t.segments.iter_mut())
            .find(|s| &s.id == segment_id)
            .expect("found above");
        let length = segment.target_range.duration;
        segment.material_id = material_id;
        segment.source_range = TimeRange::new(0, length);
        segment.speed = 1.0;
        segment.crop = None;
        segment.keyframes.retain(|t| !t.property.is_crop());
        segment.extras.retain(|id| !strip.contains(id));
        segment.extras.push(marker_id);
        if let Some(link) = link {
            gone_links.push(link);
        }
    }

    // A clip the user did not choose is no slot of this template, whatever
    // template the project came from.
    let markers: HashSet<String> = template
        .materials
        .extras
        .keys()
        .filter(|id| slot::is_marker(&template, id))
        .cloned()
        .collect();
    let kept: HashSet<String> = chosen
        .iter()
        .filter_map(|id| template.segment(id))
        .flat_map(|(_, s)| s.extras.iter().cloned())
        .collect();
    for track in &mut template.tracks {
        for segment in &mut track.segments {
            if !chosen.contains(&segment.id) {
                segment
                    .extras
                    .retain(|id| !markers.contains(id) || kept.contains(id));
            }
        }
    }

    // The footage's own sound, linked to a clip that is now a slot.
    for link in &gone_links {
        for track in &mut template.tracks {
            if track.kind == TrackKind::Audio {
                track.segments.retain(|s| !s.extras.contains(link));
            }
        }
        for track in &mut template.tracks {
            for segment in &mut track.segments {
                segment.extras.retain(|id| id != link);
            }
        }
        template.materials.links.remove(link);
    }

    drop_unused_media(&mut template);

    let id = id_from_name(name);
    let mut file = TemplateFile::new(id, name.to_string(), &template)?;
    file.description = request.description.trim().to_string();
    file.category = match request.category.trim() {
        "" => "My templates".to_string(),
        other => other.to_string(),
    };
    Ok(file)
}

/// Take videos, stills and sounds no clip uses out of the pool.
fn drop_unused_media(project: &mut Project) {
    let used: HashSet<String> = project
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .map(|s| s.material_id.clone())
        .collect();
    let pool = &mut project.materials;
    pool.videos.retain(|m| used.contains(&m.id));
    pool.images.retain(|m| used.contains(&m.id));
    pool.audios.retain(|m| used.contains(&m.id));
    pool.origins.retain(|id, _| used.contains(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AudioMaterial, CanvasConfig, Crop, Segment, Track, Transform, VideoMaterial,
    };

    fn clip(id: &str, material: &str, start: i64, extras: &[&str]) -> Segment {
        Segment {
            id: id.into(),
            material_id: material.into(),
            target_range: TimeRange::new(start, 2_000_000),
            source_range: TimeRange::new(5_000_000, 2_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: extras.iter().map(|s| s.to_string()).collect(),
            keyframes: Vec::new(),
        }
    }

    fn project() -> Project {
        let mut p = Project::new("Mine", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "wide".into(),
            path: "/footage/wide.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 20_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        p.materials.videos.push(VideoMaterial {
            id: "unused".into(),
            path: "/footage/unused.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 20_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        p.materials.audios.push(AudioMaterial {
            id: "wide-sound".into(),
            path: "/footage/wide.mp4".into(),
            duration: 20_000_000,
            sample_rate: 48_000,
            channels: 2,
        });
        p.materials.links.insert("g".into());
        let mut v = Track::new(TrackKind::Video, "V");
        let mut cropped = clip("b", "wide", 2_000_000, &[]);
        cropped.crop = Some(Crop {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 1.0,
        });
        v.segments.push(clip("a", "wide", 0, &["g"]));
        v.segments.push(cropped);
        let mut a = Track::new(TrackKind::Audio, "A");
        a.segments.push(clip("a-sound", "wide-sound", 0, &["g"]));
        p.tracks.push(v);
        p.tracks.push(a);
        p
    }

    #[test]
    fn chosen_clips_become_numbered_placeholders_of_their_shown_shape() {
        let original = project();
        let request = SaveRequest {
            name: "Wide Intro".into(),
            slots: vec!["b".into(), "a".into()],
            labels: vec!["Half".into()],
            ..SaveRequest::default()
        };
        let file = make_template(&original, &request).unwrap();
        let t = file.project(None).unwrap();
        let slots = slot::slots(&t);
        assert_eq!(slots.len(), 2);
        // Fill order is the order chosen, not the timeline's.
        assert_eq!(slots[0].segment_id, "b");
        assert_eq!(slots[0].label.as_deref(), Some("Half"));
        // Half of a 16:9 frame is 8:9.
        assert_eq!(slots[0].aspect, [8, 9]);
        assert_eq!(slots[1].aspect, [16, 9]);
        let (_, b) = t.segment("b").unwrap();
        assert!(b.crop.is_none());
        assert_eq!(b.source_range, TimeRange::new(0, 2_000_000));
        assert!(t.materials.image(&b.material_id).is_some());
        // The footage, its sound and its link went; the original did not
        // change.
        assert!(t.materials.videos.is_empty());
        assert!(t.materials.audios.is_empty());
        assert!(t.tracks[1].segments.is_empty());
        assert!(t.materials.links.is_empty());
        assert_eq!(original.materials.videos.len(), 2);
        assert_eq!(file.category, "My templates");
    }

    #[test]
    fn a_title_or_a_sound_cannot_be_a_slot() {
        let error = make_template(
            &project(),
            &SaveRequest {
                name: "x".into(),
                slots: vec!["a-sound".into()],
                ..SaveRequest::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("video or photo"), "{error}");
        let error = make_template(
            &project(),
            &SaveRequest {
                name: "x".into(),
                ..SaveRequest::default()
            },
        )
        .unwrap_err();
        assert!(error.contains("at least one"), "{error}");
    }
}
