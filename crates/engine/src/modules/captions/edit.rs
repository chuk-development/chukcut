//! Placing and changing captions, as edit commands.
//!
//! Pure, like `text::edit`: every function reads a `Project` and returns the
//! [`EditCommand`] that would make the change, plus any new materials the
//! caller pushes into the pool first (the pool is not on the undo stack — see
//! `text/commands.rs` for why). Nothing here needs an `AppState`, a window or
//! a GPU, which is what makes the timing arithmetic testable.

use serde::Serialize;

use super::style::CaptionStyle;
use super::{emoji, group, karaoke, Cue, TimedWord};
use crate::modules::project::{
    new_id, CaptionData, CaptionWord, Micros, Project, Segment, TextMaterial, TimeRange, Track,
    TrackKind, Transform,
};
use crate::modules::timeline::ops::{EditCommand, EditResult};

/// The name of the lane captions are put on.
pub const LANE_NAME: &str = "Captions";

/// Whether `segment` is a caption.
pub fn is_caption_segment(project: &Project, segment: &Segment) -> bool {
    project
        .materials
        .text(&segment.material_id)
        .is_some_and(|m| m.caption.is_some())
}

/// Whether `track` is a caption lane: a text lane named for captions, or one
/// that holds a caption.
pub fn is_caption_track(project: &Project, track: &Track) -> bool {
    track.kind == TrackKind::Text
        && (track.name == LANE_NAME
            || track.name.starts_with("Captions ")
            || track
                .segments
                .iter()
                .any(|s| is_caption_segment(project, s)))
}

/// The lane new captions go on, if the project has one.
pub fn caption_lane(project: &Project) -> Option<&Track> {
    project
        .tracks
        .iter()
        .find(|t| !t.locked && is_caption_track(project, t))
}

/// One caption as the panel lists it.
#[derive(Debug, Clone, Serialize)]
pub struct CaptionClip {
    pub segment_id: String,
    pub track_id: String,
    pub material_id: String,
    pub start: Micros,
    pub end: Micros,
    pub text: String,
    /// How many timed words it carries; 0 means karaoke has nothing to light.
    pub words: usize,
}

/// Every caption in the project, in time order.
pub fn clips(project: &Project) -> Vec<CaptionClip> {
    let mut out: Vec<CaptionClip> = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Text)
        .flat_map(|track| {
            track.segments.iter().filter_map(move |segment| {
                let material = project.materials.text(&segment.material_id)?;
                let caption = material.caption.as_ref()?;
                Some(CaptionClip {
                    segment_id: segment.id.clone(),
                    track_id: track.id.clone(),
                    material_id: material.id.clone(),
                    start: segment.target_range.start,
                    end: segment.target_range.end(),
                    text: material.content.clone(),
                    words: caption.words.len(),
                })
            })
        })
        .collect();
    out.sort_by_key(|c| (c.start, c.end));
    out
}

/// The project's captions as cues in timeline time, for export.
pub fn cues(project: &Project) -> Vec<Cue> {
    clips(project)
        .into_iter()
        .filter_map(|clip| {
            let (_, segment) = project.segment(&clip.segment_id)?;
            let material = project.materials.text(&clip.material_id)?;
            let words = material
                .caption
                .as_ref()
                .map(|c| {
                    // A word the clip has been trimmed away from is not
                    // shown, so it is not part of the caption any more.
                    c.words
                        .iter()
                        .map(|w| TimedWord {
                            text: w.text.clone(),
                            start: to_timeline(segment, w.start),
                            end: to_timeline(segment, w.end).min(clip.end),
                        })
                        .filter(|w| w.start >= clip.start && w.start < clip.end)
                        .collect()
                })
                .unwrap_or_default();
            Some(Cue {
                start: clip.start,
                end: clip.end,
                text: clip.text,
                words,
            })
        })
        .collect()
}

/// Source time of a text segment to timeline time. Text runs at speed 1.
fn to_timeline(segment: &Segment, source: Micros) -> Micros {
    source - segment.source_range.start + segment.target_range.start
}

/// A segment for a caption material, mirroring `text::edit`'s title segment:
/// the source range starts at 0 and runs at speed 1.
pub fn caption_segment(material_id: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
        id: new_id(),
        material_id: material_id.to_string(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(0, duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// The material for one cue, with its words made relative to the cue's start.
pub fn material_for(cue: &Cue, style: &CaptionStyle, auto_emoji: bool) -> TextMaterial {
    let text = if auto_emoji {
        emoji::with_auto_emoji(&cue.text)
    } else {
        cue.text.clone()
    };
    let mut material = TextMaterial {
        id: new_id(),
        content: text,
        font_family: String::new(),
        font_size: 1.0,
        color: [1.0; 4],
        bold: false,
        italic: false,
        align: Default::default(),
        stroke_width: 0.0,
        stroke_color: [0.0, 0.0, 0.0, 1.0],
        shadow: None,
        background: None,
        caption: Some(CaptionData {
            words: cue
                .words
                .iter()
                .map(|w| CaptionWord {
                    text: w.text.trim().to_string(),
                    start: (w.start - cue.start).max(0),
                    end: (w.end - cue.start).max(0),
                })
                .collect(),
            highlight: None,
        }),
        ..Default::default()
    };
    style.apply_to(&mut material);
    material
}

/// How [`place`] treats captions that are already there.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct PlaceOptions {
    /// Delete every existing caption first — CapCut's "clear current captions".
    pub replace: bool,
    /// Append one fitting emoji to each caption.
    pub auto_emoji: bool,
}

/// What [`place`] produced.
#[derive(Debug, Clone)]
pub struct Placed {
    /// Push these into `materials.texts` before applying `command`.
    pub materials: Vec<TextMaterial>,
    pub command: EditCommand,
    pub track_id: String,
    pub segment_ids: Vec<String>,
}

/// Put `cues` on a caption lane as one undo step.
///
/// The cues are sorted and clipped so they never overlap — a lane cannot hold
/// two clips at one instant, and an SRT where one cue runs into the next is
/// common. When captions are kept and the new ones would collide with them,
/// the new ones go on a lane of their own rather than being refused: a second
/// language is the usual reason for that.
pub fn place(
    project: &Project,
    cues: &[Cue],
    style: &CaptionStyle,
    options: PlaceOptions,
) -> Result<Placed, String> {
    let mut cues: Vec<Cue> = cues
        .iter()
        .filter(|c| !c.text.trim().is_empty())
        .cloned()
        .collect();
    cues.sort_by_key(|c| c.start);
    for i in 0..cues.len() {
        let next = cues.get(i + 1).map(|c| c.start);
        let cue = &mut cues[i];
        cue.start = cue.start.max(0);
        if let Some(next) = next {
            cue.end = cue.end.min(next);
        }
    }
    cues.retain(|c| c.end > c.start);
    if cues.is_empty() {
        return Err("there are no captions to add".to_string());
    }

    let mut commands = Vec::new();
    if options.replace {
        if let Some(clear) = clear(project) {
            commands.push(clear);
        }
    }

    let existing = caption_lane(project).filter(|lane| {
        options.replace
            || cues
                .iter()
                .all(|c| lane.is_range_free(&TimeRange::new(c.start, c.end - c.start), None))
    });
    let (track_id, kept): (String, Vec<Micros>) = match existing {
        Some(lane) => {
            let kept = if options.replace {
                // Only captions were removed; anything else on the lane stays.
                lane.segments
                    .iter()
                    .filter(|s| !is_caption_segment(project, s))
                    .map(|s| s.target_range.start)
                    .collect()
            } else {
                lane.segments.iter().map(|s| s.target_range.start).collect()
            };
            (lane.id.clone(), kept)
        }
        None => {
            let lanes = project
                .tracks
                .iter()
                .filter(|t| is_caption_track(project, t))
                .count();
            let name = if lanes == 0 {
                LANE_NAME.to_string()
            } else {
                format!("{LANE_NAME} {}", lanes + 1)
            };
            let lane = Track::new(TrackKind::Text, name);
            let id = lane.id.clone();
            commands.push(EditCommand::AddTrack {
                track: lane,
                index: project.tracks.len(),
            });
            (id, Vec::new())
        }
    };

    if options.replace {
        if let Some(lane) = project.track(&track_id) {
            let blocked = cues.iter().any(|c| {
                lane.segments.iter().any(|s| {
                    !is_caption_segment(project, s)
                        && s.target_range
                            .overlaps(&TimeRange::new(c.start, c.end - c.start))
                })
            });
            if blocked {
                return Err(
                    "the caption lane has other clips where the new captions would go".to_string(),
                );
            }
        }
    }

    let mut materials = Vec::with_capacity(cues.len());
    let mut segment_ids = Vec::with_capacity(cues.len());
    for (i, cue) in cues.iter().enumerate() {
        let material = material_for(cue, style, options.auto_emoji);
        let mut segment = caption_segment(&material.id, cue.start, cue.end - cue.start);
        segment.transform.position = style.position;
        let index = kept.iter().filter(|s| **s < cue.start).count() + i;
        segment_ids.push(segment.id.clone());
        commands.push(EditCommand::InsertSegment {
            track_id: track_id.clone(),
            segment,
            index,
        });
        materials.push(material);
    }

    Ok(Placed {
        materials,
        command: EditCommand::Composite {
            label: "Add captions".to_string(),
            commands,
        },
        track_id,
        segment_ids,
    })
}

/// Delete every caption, as one command. `None` when there are none.
pub fn clear(project: &Project) -> Option<EditCommand> {
    let mut commands = Vec::new();
    for track in &project.tracks {
        // Highest index first, so each removal leaves the indices of the ones
        // still to come — and of their undo — where they were.
        for (index, segment) in track.segments.iter().enumerate().rev() {
            if is_caption_segment(project, segment) {
                commands.push(EditCommand::RemoveSegment {
                    track_id: track.id.clone(),
                    segment: segment.clone(),
                    index,
                });
            }
        }
    }
    (!commands.is_empty()).then(|| EditCommand::Composite {
        label: "Delete captions".to_string(),
        commands,
    })
}

/// The caption segment `segment_id`, its lane and its material.
fn caption<'a>(
    project: &'a Project,
    segment_id: &str,
) -> Result<(&'a Track, &'a Segment, &'a TextMaterial), String> {
    let (track, segment) = project
        .segment(segment_id)
        .ok_or_else(|| "that caption is no longer on the timeline".to_string())?;
    let material = project
        .materials
        .text(&segment.material_id)
        .filter(|m| m.caption.is_some())
        .ok_or_else(|| "that clip is not a caption".to_string())?;
    Ok((track, segment, material))
}

/// What `EditCommand::SetTextMaterial` does.
pub fn set_text_material(
    project: &mut Project,
    before: &TextMaterial,
    after: &TextMaterial,
) -> EditResult {
    if before.id != after.id {
        return Err("a text edit cannot change which title it is".to_string());
    }
    crate::modules::text::edit::check_material(after)?;
    let slot = project
        .materials
        .texts
        .iter_mut()
        .find(|m| m.id == after.id)
        .ok_or_else(|| format!("no title with the id {}", after.id))?;
    *slot = after.clone();
    Ok(())
}

/// Change a caption's words, keeping its timing where it still applies.
pub fn set_text(project: &Project, segment_id: &str, text: &str) -> Result<EditCommand, String> {
    let (_, segment, material) = caption(project, segment_id)?;
    let mut after = material.clone();
    after.content = text.to_string();
    if let Some(caption) = after.caption.as_mut() {
        caption.words = resync_words(&caption.words, text, segment);
    }
    Ok(EditCommand::SetTextMaterial {
        before: material.clone(),
        after,
    })
}

/// Word timing for edited text.
///
/// If every word is still found, nothing changes. If the user retyped words
/// one for one — fixing a misheard word is the common edit — the times carry
/// over in order. Otherwise the words are re-estimated across the caption.
fn resync_words(words: &[CaptionWord], text: &str, segment: &Segment) -> Vec<CaptionWord> {
    let probe = TextMaterial {
        content: text.to_string(),
        caption: Some(CaptionData {
            words: words.to_vec(),
            highlight: None,
        }),
        ..crate::modules::text::edit::default_material(
            &Project::new("", Default::default(), 30.0),
            None,
        )
    };
    if karaoke::word_ranges(&probe).iter().all(Option::is_some) {
        return words.to_vec();
    }
    let tokens: Vec<&str> = text
        .split_whitespace()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect();
    if tokens.len() == words.len() {
        return words
            .iter()
            .zip(tokens)
            .map(|(w, t)| CaptionWord {
                text: t.to_string(),
                ..w.clone()
            })
            .collect();
    }
    let (from, to) = match (words.first(), words.last()) {
        (Some(first), Some(last)) => (first.start, last.end),
        _ => (segment.source_range.start, segment.source_range.end()),
    };
    super::estimate_words(&tokens.join(" "), from, to)
        .into_iter()
        .map(|w| CaptionWord {
            text: w.text,
            start: w.start,
            end: w.end,
        })
        .collect()
}

/// Apply `style` to one caption, or to all of them when `segment_id` is
/// `None`, as one undo step.
pub fn restyle(
    project: &Project,
    segment_id: Option<&str>,
    style: &CaptionStyle,
) -> Result<EditCommand, String> {
    let targets: Vec<String> = match segment_id {
        Some(id) => {
            caption(project, id)?;
            vec![id.to_string()]
        }
        None => clips(project).into_iter().map(|c| c.segment_id).collect(),
    };
    if targets.is_empty() {
        return Err("there are no captions to style".to_string());
    }

    let mut commands = Vec::new();
    let mut seen_materials = std::collections::HashSet::new();
    for id in targets {
        let (_, segment, material) = caption(project, &id)?;
        if seen_materials.insert(material.id.clone()) {
            let mut after = material.clone();
            style.apply_to(&mut after);
            if serde_json::to_value(&after).ok() != serde_json::to_value(material).ok() {
                commands.push(EditCommand::SetTextMaterial {
                    before: material.clone(),
                    after,
                });
            }
        }
        if segment.transform.position != style.position {
            let after = Transform {
                position: style.position,
                ..segment.transform
            };
            commands.push(EditCommand::SetTransform {
                segment_id: id,
                before: segment.transform,
                after,
            });
        }
    }
    Ok(EditCommand::Composite {
        label: "Style captions".to_string(),
        commands,
    })
}

/// Split a caption at timeline instant `at` into two captions, dividing its
/// words and its text between them.
///
/// Unlike the timeline's own split — which leaves both halves showing the
/// whole sentence, because a title has no idea which of its words belong
/// where — this cuts the text at the first word spoken after `at`.
pub fn split(
    project: &Project,
    segment_id: &str,
    at: Micros,
) -> Result<(Vec<TextMaterial>, EditCommand, [String; 2]), String> {
    let (track, segment, material) = caption(project, segment_id)?;
    let range = segment.target_range;
    if at <= range.start || at >= range.end() {
        return Err("put the playhead inside the caption to split it".to_string());
    }
    let cut = at - range.start + segment.source_range.start;
    let words = material
        .caption
        .as_ref()
        .map(|c| c.words.clone())
        .unwrap_or_default();

    let (left_text, right_text) =
        split_text(material, &words, cut, at - range.start, range.duration);
    if left_text.is_empty() || right_text.is_empty() {
        return Err("there are no words on one side of the playhead".to_string());
    }

    let mut left = material.clone();
    left.id = new_id();
    left.content = left_text;
    let mut right = material.clone();
    right.id = new_id();
    right.content = right_text;
    if let (Some(l), Some(r)) = (left.caption.as_mut(), right.caption.as_mut()) {
        l.words = words.iter().filter(|w| w.start < cut).cloned().collect();
        r.words = words
            .iter()
            .filter(|w| w.start >= cut)
            .map(|w| CaptionWord {
                text: w.text.clone(),
                start: w.start - cut,
                end: (w.end - cut).max(0),
            })
            .collect();
    }

    let mut left_segment = segment.clone();
    left_segment.id = new_id();
    left_segment.material_id = left.id.clone();
    left_segment.target_range = TimeRange::new(range.start, at - range.start);
    left_segment.source_range = TimeRange::new(segment.source_range.start, at - range.start);
    let mut right_segment = caption_segment(&right.id, at, range.end() - at);
    right_segment.transform = segment.transform;

    let index = track
        .segments
        .iter()
        .position(|s| s.id == segment.id)
        .unwrap_or(0);
    let ids = [left_segment.id.clone(), right_segment.id.clone()];
    let command = EditCommand::Composite {
        label: "Split caption".to_string(),
        commands: vec![
            EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: segment.clone(),
                index,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment: left_segment,
                index,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment: right_segment,
                index: index + 1,
            },
        ],
    };
    Ok((vec![left, right], command, ids))
}

/// The two materials the timeline's split gives a caption cut `offset` into
/// its clip: the left keeps the material's id and the words before the cut,
/// the right is a new material with the words from the cut on. Word times
/// stay in source time, because the timeline's right half starts its source
/// at the cut (`timeline::ops::split_at`).
///
/// `None` for a clip that is not a caption, and for a cut with no text on one
/// side — a one-word caption — where both halves keep the whole caption as
/// before rather than one of them showing nothing.
pub fn split_material(
    project: &Project,
    segment: &Segment,
    offset: Micros,
) -> Option<(TextMaterial, TextMaterial)> {
    let material = project
        .materials
        .text(&segment.material_id)
        .filter(|m| m.caption.is_some())?;
    let cut = segment.source_range.start + offset;
    let words = material
        .caption
        .as_ref()
        .map(|c| c.words.clone())
        .unwrap_or_default();
    let (left_text, right_text) =
        split_text(material, &words, cut, offset, segment.target_range.duration);
    if left_text.is_empty() || right_text.is_empty() {
        return None;
    }
    let mut left = material.clone();
    left.content = left_text;
    let mut right = material.clone();
    right.id = new_id();
    right.content = right_text;
    if let (Some(l), Some(r)) = (left.caption.as_mut(), right.caption.as_mut()) {
        l.words = words.iter().filter(|w| w.start < cut).cloned().collect();
        r.words = words.iter().filter(|w| w.start >= cut).cloned().collect();
    }
    Some((left, right))
}

/// Where the text divides: at the first word spoken after the cut, found in
/// the text as the user has it; or, without words, at the space nearest the
/// same fraction of the text as the cut is of the time.
fn split_text(
    material: &TextMaterial,
    words: &[CaptionWord],
    cut: Micros,
    offset: Micros,
    duration: Micros,
) -> (String, String) {
    let content = material.content.as_str();
    let ranges = karaoke::word_ranges(material);
    let first_after = words
        .iter()
        .zip(ranges.iter())
        .find(|(w, r)| w.start >= cut && r.is_some())
        .and_then(|(_, r)| r.clone());
    let at_byte = match first_after {
        Some(range) => range.start,
        None if !words.is_empty() && words.iter().all(|w| w.start < cut) => content.len(),
        None => {
            let fraction = offset as f64 / duration.max(1) as f64;
            let target = (content.len() as f64 * fraction) as usize;
            content
                .char_indices()
                .filter(|(_, c)| c.is_whitespace())
                .map(|(i, _)| i)
                .min_by_key(|i| i.abs_diff(target))
                .unwrap_or(content.len())
        }
    };
    let tidy = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let keep_lines = |s: &str| {
        s.lines()
            .map(tidy)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    };
    (
        keep_lines(&content[..at_byte]),
        keep_lines(&content[at_byte..]),
    )
}

/// Merge caption `first` with the caption `second` that follows it on the
/// same lane, as one undo step. The merged caption keeps `first`'s style.
pub fn merge(
    project: &Project,
    first_id: &str,
    second_id: &str,
) -> Result<(TextMaterial, EditCommand, String), String> {
    let (track, first, first_material) = caption(project, first_id)?;
    let (second_track, second, second_material) = caption(project, second_id)?;
    if track.id != second_track.id {
        return Err("only captions on the same lane can be merged".to_string());
    }
    let (first, second, first_material, second_material) =
        if first.target_range.start <= second.target_range.start {
            (first, second, first_material, second_material)
        } else {
            (second, first, second_material, first_material)
        };
    let between = track.segments.iter().any(|s| {
        s.id != first.id
            && s.id != second.id
            && s.target_range.start >= first.target_range.end()
            && s.target_range.end() <= second.target_range.start
    });
    if between {
        return Err("there is another clip between those captions".to_string());
    }

    let mut merged = first_material.clone();
    merged.id = new_id();
    merged.content = format!(
        "{} {}",
        first_material.content.trim_end(),
        second_material.content.trim_start()
    );
    // The second caption's words, moved into the first one's source time.
    let shift = second.target_range.start - first.target_range.start + first.source_range.start
        - second.source_range.start;
    if let Some(caption) = merged.caption.as_mut() {
        let more = second_material
            .caption
            .as_ref()
            .map(|c| c.words.clone())
            .unwrap_or_default();
        caption.words.extend(more.into_iter().map(|w| CaptionWord {
            text: w.text,
            start: w.start + shift,
            end: w.end + shift,
        }));
    }

    let start = first.target_range.start;
    let end = second.target_range.end();
    let mut segment = first.clone();
    segment.id = new_id();
    segment.material_id = merged.id.clone();
    segment.target_range = TimeRange::new(start, end - start);
    segment.source_range = TimeRange::new(first.source_range.start, end - start);

    let index_of = |id: &str| track.segments.iter().position(|s| s.id == id).unwrap_or(0);
    let first_index = index_of(&first.id);
    let second_index = index_of(&second.id);
    let id = segment.id.clone();
    let command = EditCommand::Composite {
        label: "Merge captions".to_string(),
        commands: vec![
            EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: second.clone(),
                index: second_index,
            },
            EditCommand::RemoveSegment {
                track_id: track.id.clone(),
                segment: first.clone(),
                index: first_index,
            },
            EditCommand::InsertSegment {
                track_id: track.id.clone(),
                segment,
                index: first_index,
            },
        ],
    };
    Ok((merged, command, id))
}

/// Re-group the words of every caption into a new mode — switching an
/// existing set between word and sentence captions without transcribing
/// again. Each caption keeps its own style; the new ones take the style of the
/// caption they start in.
pub fn regroup(
    project: &Project,
    mode: group::CaptionMode,
) -> Result<(Vec<TextMaterial>, EditCommand), String> {
    let cues = cues(project);
    let words: Vec<TimedWord> = cues
        .iter()
        .flat_map(|c| {
            if c.words.is_empty() {
                super::estimate_words(&c.text, c.start, c.end)
            } else {
                c.words.clone()
            }
        })
        .collect();
    if words.is_empty() {
        return Err("there are no captions to regroup".to_string());
    }
    let first = clips(project)
        .into_iter()
        .next()
        .ok_or("there are no captions")?;
    let (_, segment, material) = caption(project, &first.segment_id)?;
    let style = CaptionStyle::of(material, segment);
    let placed = place(
        project,
        &group::group(&words, mode),
        &style,
        PlaceOptions {
            replace: true,
            auto_emoji: false,
        },
    )?;
    Ok((placed.materials, placed.command))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::captions::{CaptionMode, Placement};
    use crate::modules::project::CanvasConfig;
    use crate::modules::timeline::ops::EditCommand;

    fn project() -> Project {
        Project::new("t", CanvasConfig::default(), 30.0)
    }

    fn word(text: &str, start: Micros, end: Micros) -> TimedWord {
        TimedWord {
            text: text.into(),
            start,
            end,
        }
    }

    /// Push the materials and apply the command, as `commands.rs` does.
    fn apply(project: &mut Project, materials: Vec<TextMaterial>, command: EditCommand) {
        project.materials.texts.extend(materials);
        command.apply(project).expect("the edit applies");
        let errors: Vec<_> = project
            .validate()
            .into_iter()
            .filter(|i| matches!(i.severity, crate::modules::project::Severity::Error))
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    fn sample(project: &mut Project) -> Placed {
        let cues = vec![
            Cue {
                start: 1_000_000,
                end: 2_000_000,
                text: "hello big world".into(),
                words: vec![
                    word("hello", 1_000_000, 1_300_000),
                    word("big", 1_400_000, 1_600_000),
                    word("world", 1_700_000, 1_900_000),
                ],
            },
            // Overlaps the first; it is clipped rather than refused.
            Cue::new(1_500_000, 3_000_000, "second"),
        ];
        let style = CaptionStyle::default_for(&project.canvas);
        let placed = place(project, &cues, &style, PlaceOptions::default()).unwrap();
        apply(project, placed.materials.clone(), placed.command.clone());
        placed
    }

    #[test]
    fn placing_makes_a_lane_and_clips_overlaps() {
        let mut p = project();
        let placed = sample(&mut p);
        let lane = p.track(&placed.track_id).unwrap();
        assert_eq!(lane.name, LANE_NAME);
        assert_eq!(lane.kind, TrackKind::Text);
        let listed = clips(&p);
        assert_eq!(listed.len(), 2);
        assert_eq!((listed[0].start, listed[0].end), (1_000_000, 1_500_000));
        assert_eq!((listed[1].start, listed[1].end), (1_500_000, 3_000_000));
        // Words are stored relative to the caption.
        let material = p.materials.text(&listed[0].material_id).unwrap();
        assert_eq!(material.caption.as_ref().unwrap().words[1].start, 400_000);
        // And come back in timeline time.
        assert_eq!(cues(&p)[0].words[1].start, 1_400_000);
        // Position from the style.
        let (_, segment) = p.segment(&listed[0].segment_id).unwrap();
        assert_eq!(segment.transform.position[1], Placement::Bottom.y());
    }

    #[test]
    fn placing_again_without_replacing_uses_a_second_lane_on_collision() {
        let mut p = project();
        sample(&mut p);
        let style = CaptionStyle::default_for(&p.canvas);
        let again = place(
            &p,
            &[Cue::new(1_200_000, 1_800_000, "again")],
            &style,
            PlaceOptions::default(),
        )
        .unwrap();
        apply(&mut p, again.materials, again.command);
        let lanes: Vec<_> = p
            .tracks
            .iter()
            .filter(|t| is_caption_track(&p, t))
            .collect();
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[1].name, "Captions 2");

        // Into a gap, it reuses the first lane.
        let gap = place(
            &p,
            &[Cue::new(5_000_000, 6_000_000, "later")],
            &style,
            PlaceOptions::default(),
        )
        .unwrap();
        assert_eq!(gap.track_id, lanes[0].id);
    }

    #[test]
    fn replacing_clears_first_and_undoes_as_one_step() {
        let mut p = project();
        sample(&mut p);
        let before = serde_json::to_value(&p.tracks).unwrap();
        let style = CaptionStyle::default_for(&p.canvas);
        let placed = place(
            &p,
            &[Cue::new(0, 500_000, "only this")],
            &style,
            PlaceOptions {
                replace: true,
                auto_emoji: false,
            },
        )
        .unwrap();
        let command = placed.command.clone();
        apply(&mut p, placed.materials, placed.command);
        assert_eq!(clips(&p).len(), 1);
        assert_eq!(
            p.tracks.iter().filter(|t| is_caption_track(&p, t)).count(),
            1
        );

        command.invert().apply(&mut p).unwrap();
        assert_eq!(serde_json::to_value(&p.tracks).unwrap(), before);
    }

    #[test]
    fn auto_emoji_is_added_to_the_text() {
        let p = project();
        let style = CaptionStyle::default_for(&p.canvas);
        let placed = place(
            &p,
            &[Cue::new(0, 500_000, "I love pizza")],
            &style,
            PlaceOptions {
                replace: false,
                auto_emoji: true,
            },
        )
        .unwrap();
        assert!(placed.materials[0].content.starts_with("I love pizza "));
        assert!(emoji::has_emoji(&placed.materials[0].content));
    }

    #[test]
    fn set_text_keeps_timing_for_a_one_for_one_fix_and_undoes() {
        let mut p = project();
        sample(&mut p);
        let first = clips(&p)[0].clone();
        let command = set_text(&p, &first.segment_id, "hello pig world").unwrap();
        command.apply(&mut p).unwrap();
        let material = p.materials.text(&first.material_id).unwrap();
        assert_eq!(material.content, "hello pig world");
        let words = &material.caption.as_ref().unwrap().words;
        assert_eq!(words[1].text, "pig");
        assert_eq!(words[1].start, 400_000);

        command.invert().apply(&mut p).unwrap();
        assert_eq!(
            p.materials.text(&first.material_id).unwrap().content,
            "hello big world"
        );
    }

    #[test]
    fn set_text_reestimates_when_the_word_count_changes() {
        let mut p = project();
        sample(&mut p);
        let first = clips(&p)[0].clone();
        set_text(&p, &first.segment_id, "a completely different sentence now")
            .unwrap()
            .apply(&mut p)
            .unwrap();
        let words = &p
            .materials
            .text(&first.material_id)
            .unwrap()
            .caption
            .as_ref()
            .unwrap()
            .words;
        assert_eq!(words.len(), 5);
        assert_eq!(words[0].start, 0);
        assert_eq!(words[4].end, 900_000);
    }

    #[test]
    fn split_divides_words_and_text_at_the_playhead() {
        let mut p = project();
        sample(&mut p);
        let first = clips(&p)[0].clone();
        let (materials, command, ids) = split(&p, &first.segment_id, 1_350_000).unwrap();
        apply(&mut p, materials, command.clone());
        let listed = clips(&p);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].text, "hello");
        assert_eq!(listed[1].text, "big world");
        assert_eq!(listed[1].segment_id, ids[1]);
        assert_eq!((listed[0].end, listed[1].start), (1_350_000, 1_350_000));
        // The right half's words start from its own beginning.
        let right = p.materials.text(&listed[1].material_id).unwrap();
        assert_eq!(right.caption.as_ref().unwrap().words[0].start, 50_000);
        // And the timeline-time view is unchanged by the split.
        assert_eq!(cues(&p)[1].words[0].start, 1_400_000);

        command.invert().apply(&mut p).unwrap();
        assert_eq!(clips(&p).len(), 2);
    }

    #[test]
    fn the_timeline_split_divides_a_caption_like_the_panel_and_undoes_exactly() {
        use crate::modules::timeline::{ops::split_at, History};
        let mut p = project();
        sample(&mut p);
        let first = clips(&p)[0].clone();
        let before = serde_json::to_value(&p).unwrap();
        let cues_before = cues(&p);

        let mut history = History::new();
        let command = split_at(&p, &first.segment_id, 1_350_000).unwrap();
        history.apply(&mut p, command).unwrap();
        assert!(p
            .validate()
            .iter()
            .all(|i| i.severity != crate::modules::project::Severity::Error));

        let listed = clips(&p);
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].text, "hello");
        assert_eq!(listed[1].text, "big world");
        // The left half keeps its id, the right one has a material of its own.
        assert_eq!(listed[0].segment_id, first.segment_id);
        assert_ne!(listed[0].material_id, listed[1].material_id);
        // In timeline time every word is where it was.
        let after = cues(&p);
        assert_eq!(after[0].words.len(), 1);
        assert_eq!(after[1].words[0].start, cues_before[0].words[1].start);

        history.undo(&mut p).unwrap();
        assert_eq!(serde_json::to_value(&p).unwrap(), before);
        history.redo(&mut p).unwrap();
        assert_eq!(clips(&p).len(), 3);
    }

    #[test]
    fn the_timeline_split_of_a_one_word_caption_keeps_the_word_on_both_halves() {
        use crate::modules::timeline::ops::split_at;
        let mut p = project();
        sample(&mut p);
        let second = clips(&p)[1].clone();
        let command = split_at(&p, &second.segment_id, 2_000_000).unwrap();
        command.apply(&mut p).unwrap();
        let listed = clips(&p);
        assert_eq!(listed.len(), 3);
        assert_eq!(
            (listed[1].text.as_str(), listed[2].text.as_str()),
            ("second", "second")
        );
    }

    #[test]
    fn split_outside_or_with_nothing_on_one_side_is_refused() {
        let mut p = project();
        sample(&mut p);
        let first = clips(&p)[0].clone();
        assert!(split(&p, &first.segment_id, 900_000).is_err());
        assert!(split(&p, &first.segment_id, first.end).is_err());
        // One word and no timing: there is no space to cut at.
        let second = clips(&p)[1].clone();
        assert!(split(&p, &second.segment_id, 2_000_000).is_err());
    }

    #[test]
    fn merge_joins_text_and_words_in_one_step() {
        let mut p = project();
        sample(&mut p);
        let listed = clips(&p);
        let (material, command, id) =
            merge(&p, &listed[1].segment_id, &listed[0].segment_id).unwrap();
        apply(&mut p, vec![material], command.clone());
        let merged = clips(&p);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].segment_id, id);
        assert_eq!(merged[0].text, "hello big world second");
        assert_eq!((merged[0].start, merged[0].end), (1_000_000, 3_000_000));

        command.invert().apply(&mut p).unwrap();
        assert_eq!(clips(&p).len(), 2);
    }

    #[test]
    fn restyle_all_changes_every_caption_and_its_position() {
        let mut p = project();
        sample(&mut p);
        let style = CaptionStyle {
            color: [1.0, 0.0, 0.0, 1.0],
            ..CaptionStyle::default_for(&p.canvas).with_placement(Placement::Top)
        };
        let command = restyle(&p, None, &style).unwrap();
        command.apply(&mut p).unwrap();
        for clip in clips(&p) {
            let (_, segment) = p.segment(&clip.segment_id).unwrap();
            let material = p.materials.text(&clip.material_id).unwrap();
            assert_eq!(material.color, [1.0, 0.0, 0.0, 1.0]);
            assert_eq!(segment.transform.position[1], Placement::Top.y());
        }
        command.invert().apply(&mut p).unwrap();
        let material = p.materials.text(&clips(&p)[0].material_id).unwrap();
        assert_eq!(material.color, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn regroup_switches_to_word_captions() {
        let mut p = project();
        sample(&mut p);
        let (materials, command) = regroup(&p, CaptionMode::words(1)).unwrap();
        apply(&mut p, materials, command);
        let texts: Vec<String> = clips(&p).into_iter().map(|c| c.text).collect();
        // "world" was trimmed off the first caption by the overlap, so it is gone.
        assert_eq!(texts, ["hello", "big", "second"]);
    }

    #[test]
    fn titles_are_not_captions() {
        let mut p = project();
        let material = crate::modules::text::edit::default_material(&p, None);
        let id = material.id.clone();
        let placement = crate::modules::text::edit::insert_command(&p, &id, 0, 1_000_000).unwrap();
        apply(&mut p, vec![material], placement.command);
        assert!(clips(&p).is_empty());
        assert!(caption_lane(&p).is_none());
        // And a new title never lands on the caption lane.
        sample(&mut p);
        let title = crate::modules::text::edit::default_material(&p, None);
        let placement =
            crate::modules::text::edit::insert_command(&p, &title.id, 5_000_000, 1_000_000)
                .unwrap();
        let lane = p.track(&placement.track_id).unwrap();
        assert!(!is_caption_track(&p, lane));
    }
}
