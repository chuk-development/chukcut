//! Auto-ducking: turn a music clip down while someone is speaking.
//!
//! 1. **Where is speech?** Every clip that is heard on another lane is
//!    analysed with the silence module's envelope — RMS per 10 ms window plus
//!    RNNoise's voice probability ([`crate::modules::silence::analyse`]). A
//!    window is speech when it is louder than the threshold *and* RNNoise
//!    hears a voice in it, so a drum hit on a sound-effects lane does not
//!    duck the music. Short gaps between words are closed and short blips
//!    dropped, or the music would pump on every syllable.
//! 2. **What does the music do?** It reaches `-depth` dB exactly where speech
//!    starts, having ramped down over `attack` before it, and comes back up
//!    over `release` after speech stops. Stretches whose ramps would touch
//!    are joined into one dip.
//! 3. **How is it written?** As ordinary `Volume` keyframes on the music clip,
//!    multiplied into whatever fades it already had, in one undo step that
//!    also records the keyframes it replaced ([`super::model::Ducking`]), so
//!    ducking again starts from the clip's own volume and "remove ducking"
//!    gives it back. Both mixers already sample `Volume` keyframes, so the
//!    preview and the export follow without any new code in either.

use std::sync::atomic::AtomicBool;

use crate::modules::project::document::{
    Easing, Keyframe, KeyframeTrack, Micros, Project, Segment, TimeRange, TrackKind,
};
use crate::modules::silence::analyse::envelope_from_file;

use super::model::{fx_of, DuckParams};

/// RNNoise's probability above which a window is a voice.
const VOICE: f32 = 0.5;
/// Pauses shorter than this between speech are bridged.
const BRIDGE: Micros = 300_000;
/// Speech shorter than this is ignored.
const MIN_SPEECH: Micros = 120_000;

/// A clip whose speech should duck the music: on another lane, heard, and
/// not itself ducked (a second music bed is not a speaker).
fn is_speaker(project: &Project, music: &Segment, music_track: &str, candidate: &Segment) -> bool {
    if candidate.id == music.id {
        return false;
    }
    let Some((track, _)) = project.segment(&candidate.id) else {
        return false;
    };
    if track.id == music_track
        || track.muted
        || !matches!(track.kind, TrackKind::Audio | TrackKind::Video)
        || project.sound_is_on_a_linked_lane(track, candidate)
    {
        return false;
    }
    !fx_of(project, candidate).is_some_and(|(_, fx)| fx.ducking.is_some())
}

/// The timeline ranges in which someone speaks under `music_id`, merged.
///
/// Slow: decodes and analyses every speaking clip that overlaps the music.
/// Run without the project lock.
pub fn speech_ranges(
    project: &Project,
    music_id: &str,
    params: &DuckParams,
    cancel: &AtomicBool,
) -> Result<Vec<TimeRange>, String> {
    let (music_track, music) = project
        .segment(music_id)
        .ok_or_else(|| format!("unknown segment {music_id}"))?;
    let span = music.target_range;
    let mut ranges = Vec::new();
    for track in &project.tracks {
        for segment in &track.segments {
            if !is_speaker(project, music, &music_track.id, segment) {
                continue;
            }
            let target = segment.target_range;
            if target.end() <= span.start || target.start >= span.end() {
                continue;
            }
            let Some((path, _)) = crate::modules::voice::cleanup::original_source(project, segment)
            else {
                continue;
            };
            let envelope = envelope_from_file(&path, segment.source_range, true, cancel)?;
            let voice = envelope.voice.as_deref().unwrap_or(&[]);
            let map = project.materials.time_map(segment);
            for (i, db) in envelope.db.iter().enumerate() {
                let speaking =
                    *db >= params.threshold_db && voice.get(i).copied().unwrap_or(0.0) >= VOICE;
                if !speaking {
                    continue;
                }
                let source = envelope.start + i as Micros * envelope.window;
                let from = target.start + map.offset_of(source);
                let to = target.start + map.offset_of(source + envelope.window);
                let (from, to) = (from.max(target.start), to.min(target.end()));
                if to > from {
                    ranges.push(TimeRange::new(from, to - from));
                }
            }
        }
    }
    Ok(clean(ranges))
}

/// Sort, join what touches or is closer than [`BRIDGE`], drop blips.
pub fn clean(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<TimeRange> = Vec::new();
    for r in ranges {
        match merged.last_mut() {
            Some(last) if r.start <= last.end() + BRIDGE => {
                let end = last.end().max(r.end());
                last.duration = end - last.start;
            }
            _ => merged.push(r),
        }
    }
    merged.retain(|r| r.duration >= MIN_SPEECH);
    merged
}

/// The ducking gain at segment-relative `t` for `dips` (segment-relative),
/// `1` outside every dip.
fn duck_gain(dips: &[(Micros, Micros)], params: &DuckParams, t: Micros) -> f32 {
    let floor = 10f32.powf(-params.depth_db.abs() / 20.0);
    let mut gain = 1.0f32;
    for &(s, e) in dips {
        let g = if t >= s && t <= e {
            floor
        } else if t < s && t > s - params.attack {
            let u = (s - t) as f32 / params.attack.max(1) as f32;
            floor + (1.0 - floor) * u
        } else if t > e && t < e + params.release {
            let u = (t - e) as f32 / params.release.max(1) as f32;
            floor + (1.0 - floor) * u
        } else {
            1.0
        };
        gain = gain.min(g);
    }
    gain
}

/// The music clip's new `Volume` keyframes: `before` (its own, segment-
/// relative) multiplied by the dips that `speech` (timeline ranges) asks for.
pub fn duck_keyframes(
    music: &Segment,
    speech: &[TimeRange],
    params: &DuckParams,
    before: &[Keyframe],
) -> Vec<Keyframe> {
    let length = music.target_range.duration;
    let attack = params.attack.max(0);
    let release = params.release.max(0);
    // Into segment time, clipped to the clip, and joined where the release
    // of one would run into the attack of the next.
    let mut dips: Vec<(Micros, Micros)> = Vec::new();
    for r in speech {
        let s = (r.start - music.target_range.start).max(0);
        let e = (r.end() - music.target_range.start).min(length);
        if e <= s {
            continue;
        }
        match dips.last_mut() {
            Some(last) if s - attack <= last.1 + release => last.1 = last.1.max(e),
            _ => dips.push((s, e)),
        }
    }

    let own = KeyframeTrack {
        property: crate::modules::project::document::AnimatableProperty::Volume,
        keyframes: before.to_vec(),
    };
    let own_at = |t: Micros| own.sample(t).unwrap_or(1.0);

    let mut times: Vec<Micros> = before.iter().map(|k| k.time).collect();
    for &(s, e) in &dips {
        times.extend([s - attack, s, e, e + release]);
    }
    if !dips.is_empty() {
        // Anchor both ends so the clip's head and tail keep their level.
        times.extend([0, length]);
    }
    times.retain(|t| (0..=length).contains(t));
    times.sort_unstable();
    times.dedup();

    times
        .into_iter()
        .map(|t| Keyframe {
            time: t,
            value: own_at(t) * duck_gain(&dips, params, t),
            easing: Easing::Linear,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::Transform;

    fn music(start: Micros, length: Micros) -> Segment {
        Segment {
            id: "m".into(),
            material_id: "song".into(),
            target_range: TimeRange::new(start, length),
            source_range: TimeRange::new(0, length),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    fn params() -> DuckParams {
        DuckParams {
            depth_db: 12.0,
            attack: 200_000,
            release: 400_000,
            threshold_db: -45.0,
        }
    }

    fn at(keys: &[Keyframe], t: Micros) -> f32 {
        KeyframeTrack {
            property: crate::modules::project::document::AnimatableProperty::Volume,
            keyframes: keys.to_vec(),
        }
        .sample(t)
        .unwrap()
    }

    #[test]
    fn the_music_is_down_by_the_depth_exactly_while_someone_speaks() {
        // The music starts at 1 s; speech from 3 s to 5 s on the timeline.
        let keys = duck_keyframes(
            &music(1_000_000, 10_000_000),
            &[TimeRange::new(3_000_000, 2_000_000)],
            &params(),
            &[],
        );
        let floor = 10f32.powf(-12.0 / 20.0);
        // Segment time = timeline − 1 s.
        assert!(
            (at(&keys, 2_000_000) - floor).abs() < 1e-4,
            "at the first word"
        );
        assert!(
            (at(&keys, 4_000_000) - floor).abs() < 1e-4,
            "at the last word"
        );
        assert!(
            (at(&keys, 1_700_000) - 1.0).abs() < 1e-4,
            "before the attack"
        );
        assert!(
            at(&keys, 1_900_000) < 1.0 && at(&keys, 1_900_000) > floor,
            "mid attack"
        );
        assert!(
            (at(&keys, 4_400_000) - 1.0).abs() < 1e-4,
            "after the release"
        );
        assert!((at(&keys, 0) - 1.0).abs() < 1e-4);
        assert!((at(&keys, 10_000_000) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn existing_fades_are_kept_and_multiplied() {
        let fade_in = [
            Keyframe {
                time: 0,
                value: 0.0,
                easing: Easing::Linear,
            },
            Keyframe {
                time: 1_000_000,
                value: 1.0,
                easing: Easing::Linear,
            },
        ];
        let keys = duck_keyframes(
            &music(0, 10_000_000),
            &[TimeRange::new(5_000_000, 1_000_000)],
            &params(),
            &fade_in,
        );
        assert_eq!(at(&keys, 0), 0.0, "the fade still starts at silence");
        assert!((at(&keys, 500_000) - 0.5).abs() < 1e-3);
        assert!((at(&keys, 5_500_000) - 10f32.powf(-0.6)).abs() < 1e-4);
    }

    #[test]
    fn close_stretches_of_speech_make_one_dip_not_a_pump() {
        let keys = duck_keyframes(
            &music(0, 10_000_000),
            &[
                TimeRange::new(2_000_000, 1_000_000),
                TimeRange::new(3_300_000, 1_000_000),
            ],
            &params(),
            &[],
        );
        let floor = 10f32.powf(-12.0 / 20.0);
        assert!(
            (at(&keys, 3_150_000) - floor).abs() < 1e-4,
            "the gap stays down"
        );
    }

    #[test]
    fn short_gaps_are_bridged_and_blips_dropped() {
        let cleaned = clean(vec![
            TimeRange::new(1_000_000, 500_000),
            TimeRange::new(1_600_000, 500_000),
            TimeRange::new(5_000_000, 50_000),
        ]);
        assert_eq!(cleaned, vec![TimeRange::new(1_000_000, 1_100_000)]);
    }

    #[test]
    fn speech_outside_the_music_writes_nothing() {
        let keys = duck_keyframes(
            &music(10_000_000, 5_000_000),
            &[TimeRange::new(1_000_000, 2_000_000)],
            &params(),
            &[],
        );
        assert!(keys.is_empty());
    }
}
