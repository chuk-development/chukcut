//! The `command` surface for loudness: measure a clip, measure the mix.
//!
//! Both are read-only and blocking — they decode audio — so a UI calls them
//! off its main thread. The project lock is held only long enough to clone
//! what is needed, never across the decode.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use super::measure::{measure, measure_file, Loudness, CHANNELS, RATE};
use crate::modules::audio::decode::FileAudioSource;
use crate::modules::export::audio::mix_timeline;
use crate::modules::voice::cleanup::{audible_segment, effective_source, original_source};
use crate::state::AppState;

/// A clip's loudness as recorded, and the gain its cleanup adds on top.
#[derive(Debug, Clone, Serialize)]
pub struct ClipLoudness {
    /// The segment that was measured: the audible one of a linked pair.
    pub segment_id: String,
    /// The clip's own sound over its source range, after denoise, before any
    /// gain.
    pub loudness: Loudness,
    /// Gain from "Normalize", in dB; `0` when the clip is not normalised.
    pub gain_db: f32,
}

/// Measure the sound of `segment_id` (or of the clip that plays it). For a
/// compound clip: the mix of its contents over the part it shows.
pub fn loudness_measure_clip(
    state: &Arc<AppState>,
    segment_id: String,
    cancel: &AtomicBool,
) -> Result<ClipLoudness, String> {
    let compound = state.with_project(|project| {
        let (_, segment) = project.segment(&segment_id)?;
        project.materials.sequence(&segment.material_id)?;
        Some((project.clone(), segment.clone()))
    })?;
    if let Some((project, segment)) = compound {
        let mixed = crate::modules::sequence::audio::mix_of(
            &project,
            &segment.material_id,
            segment.source_range,
            RATE,
            cancel,
        )?;
        return Ok(ClipLoudness {
            segment_id,
            loudness: measure(&mixed, CHANNELS as u32, RATE)?,
            gain_db: 0.0,
        });
    }
    let (audible, path, range, gain_db) = state.with_project(|project| {
        let audible = audible_segment(project, &segment_id).ok_or("the clip has no sound")?;
        let (_, segment) = project.segment(&audible).ok_or("unknown clip")?;
        let (original, _) = original_source(project, segment).ok_or("the clip has no sound")?;
        let effective = effective_source(project, segment, &original);
        let gain_db = 20.0 * effective.gain.max(1e-6).log10();
        Ok::<_, String>((audible, effective.path, segment.source_range, gain_db))
    })??;
    let loudness = measure_file(&path, range, cancel)?;
    Ok(ClipLoudness {
        segment_id: audible,
        loudness,
        gain_db,
    })
}

/// Measure the whole timeline as it would export: every audible clip, with
/// volumes, fades and cleanup, mixed to stereo. The root timeline, like the
/// export, also while a compound clip is open; compound clips' contents are
/// in the mix.
pub fn loudness_measure_mix(
    state: &Arc<AppState>,
    cancel: &AtomicBool,
) -> Result<Loudness, String> {
    let project = crate::modules::sequence::export_root(state.with_project(|p| p.clone())?);
    if project.duration() <= 0 {
        return Err("the timeline is empty".into());
    }
    crate::modules::voice::denoise::ensure_rendered(&project, cancel)?;
    let mixed = mix_timeline(&project, &FileAudioSource, RATE, CHANNELS, cancel)
        .map_err(|e| e.to_string())?;
    measure(&mixed, CHANNELS as u32, RATE)
}
