//! The captions' words as the silence module's transcript.
//!
//! Filler-word cutting (`silence::filler`) needs the words spoken in one clip,
//! in that clip's **source** time — the time a cut list is kept in, because it
//! does not move when the clip is trimmed or slid. Captions keep their words
//! in each caption segment's own source time, and [`edit::cues`] turns them
//! into timeline time. This module goes the last step: timeline time through
//! the cut clip's placement and speed into its source.
//!
//! Captions transcribe the timeline *mix*, so a word belongs to whatever clip
//! is under it on the timeline; that is the clip whose sound said it.

use std::sync::Arc;

use super::{edit, TimedWord};
use crate::modules::project::{Micros, Project, Segment};
use crate::modules::silence::filler::{register_word_timings, WordTimings};

/// [`WordTimings`] over the project's captions.
pub struct CaptionWordTimings;

impl WordTimings for CaptionWordTimings {
    fn words(&self, project: &Project, segment_id: &str) -> Option<Vec<TimedWord>> {
        words_in_clip(project, segment_id)
    }
}

/// Install the captions as the filler-word transcript. Called when the
/// engine's state is made, so every shell (app, CLI, MCP) has it.
pub fn register() {
    register_word_timings(Arc::new(CaptionWordTimings));
}

/// The timed caption words spoken while `segment_id` plays, in its source
/// time. `None` when no caption with word times overlaps the clip: an `.srt`
/// import has no word times, and estimating them would cut beside the filler.
pub fn words_in_clip(project: &Project, segment_id: &str) -> Option<Vec<TimedWord>> {
    let (_, segment) = project.segment(segment_id)?;
    let clip = segment.target_range;
    let mut words: Vec<TimedWord> = edit::cues(project)
        .into_iter()
        .filter(|cue| cue.start < clip.end() && cue.end > clip.start)
        .flat_map(|cue| cue.words)
        .filter(|w| w.start >= clip.start && w.start < clip.end())
        .map(|w| TimedWord {
            start: to_source(segment, w.start),
            end: to_source(segment, w.end.min(clip.end())),
            text: w.text,
        })
        .collect();
    if words.is_empty() {
        return None;
    }
    // Two caption lanes over one stretch (a translation, a second take) say
    // the same words twice; a filler is one cut however often it is listed.
    words.sort_by(|a, b| (a.start, &a.text).cmp(&(b.start, &b.text)));
    words.dedup_by(|a, b| a.start == b.start && a.text == b.text);
    Some(words)
}

/// Timeline instant `t` inside `segment` as source time, through its speed.
fn to_source(segment: &Segment, t: Micros) -> Micros {
    let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
        segment.speed as f64
    } else {
        1.0
    };
    let offset = (t - segment.target_range.start).max(0);
    segment.source_range.start + (offset as f64 * speed).round() as Micros
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::captions::{edit::place, edit::PlaceOptions, CaptionStyle, Cue};
    use crate::modules::project::{
        CanvasConfig, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    };
    use crate::modules::silence::filler::filler_cuts;

    const S: Micros = 1_000_000;

    fn word(text: &str, start: Micros, end: Micros) -> TimedWord {
        TimedWord {
            text: text.into(),
            start,
            end,
        }
    }

    /// A take placed at 10 s on the timeline, reading the file from 4 s on at
    /// 2x, captioned with "so um today".
    fn project() -> Project {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "take".into(),
            path: "/nonexistent/take.mp4".into(),
            width: 1080,
            height: 1920,
            duration: 60 * S,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
        let mut v = Track::new(TrackKind::Video, "V1");
        v.segments.push(Segment {
            id: "take-v".into(),
            material_id: "take".into(),
            target_range: TimeRange::new(10 * S, 5 * S),
            source_range: TimeRange::new(4 * S, 10 * S),
            render_index: 0,
            speed: 2.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        p.tracks.push(v);
        let cue = Cue {
            start: 10 * S,
            end: 12 * S,
            text: "so um today".into(),
            words: vec![
                word("so", 10 * S, 10 * S + 300_000),
                word("um", 11 * S, 11 * S + 200_000),
                word("today", 11 * S + 500_000, 12 * S),
            ],
        };
        // A caption from an .srt, without word times, later in the clip.
        let srt = Cue::new(13 * S, 14 * S, "uh hello");
        let style = CaptionStyle::default_for(&p.canvas);
        let placed = place(&p, &[cue, srt], &style, PlaceOptions::default()).unwrap();
        p.materials.texts.extend(placed.materials);
        placed.command.apply(&mut p).unwrap();
        p
    }

    #[test]
    fn caption_words_map_into_the_clips_source_time_through_its_speed() {
        let p = project();
        let words = words_in_clip(&p, "take-v").unwrap();
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["so", "um", "today"]
        );
        // 11 s on the timeline is 1 s into the clip, 2 s of source at 2x.
        assert_eq!(words[1].start, 6 * S);
        assert_eq!(words[1].end, 6 * S + 400_000);
        // And the filler finder turns that into a source-time cut.
        let cuts = filler_cuts(&words, "en", 0);
        assert_eq!(cuts, vec![TimeRange::new(6 * S, 400_000)]);
    }

    #[test]
    fn a_clip_with_no_timed_caption_words_has_no_transcript() {
        let mut p = project();
        // Move the clip past every caption.
        p.tracks[0].segments[0].target_range = TimeRange::new(30 * S, 5 * S);
        assert!(words_in_clip(&p, "take-v").is_none());
        assert!(words_in_clip(&p, "no-such-clip").is_none());
    }

    #[test]
    fn the_registered_provider_answers_the_silence_module() {
        register();
        let p = project();
        let provider = crate::modules::silence::filler::word_timings().unwrap();
        assert_eq!(provider.words(&p, "take-v").unwrap().len(), 3);
    }
}
