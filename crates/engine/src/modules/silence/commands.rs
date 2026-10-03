//! The `command` surface for silence and filler cutting.
//!
//! `silence_analyse` is the slow one (it decodes); `silence_detect` and
//! `silence_filler_cuts` are cheap and are what a panel calls on every slider
//! move; `silence_remove` is the edit.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::analyse::{envelope_from_file, Envelope};
use super::cut::remove_ranges;
use super::detect::{detect, suggest_threshold, SilenceParams};
use super::filler::{filler_cuts, word_timings};
use crate::modules::project::document::{Micros, TimeRange};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::voice::cleanup::{audible_segment, effective_source, original_source};
use crate::state::AppState;

/// One clip's sound, measured, plus what the panel needs to place cuts on
/// the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    /// The clip the panel was opened on.
    pub segment_id: String,
    /// The clip whose sound was measured: the audio partner of a linked
    /// picture, or the clip itself.
    pub sound_segment_id: String,
    /// The clip's source range when it was analysed.
    pub source: TimeRange,
    /// Where the clip started on the timeline, and its speed, to show cuts in
    /// timeline time.
    pub timeline_start: Micros,
    pub speed: f32,
    pub envelope: Envelope,
    /// A threshold that fits this recording; the panel's starting value.
    pub suggested_threshold_db: f32,
}

impl Analysis {
    /// Where source time `t` of the clip sits on the timeline.
    pub fn timeline_time(&self, t: Micros) -> Micros {
        let speed = if self.speed.is_finite() && self.speed > 0.0 {
            self.speed as f64
        } else {
            1.0
        };
        self.timeline_start + ((t - self.source.start) as f64 / speed).round() as Micros
    }
}

/// Decode and measure the sound of `segment_id`. Blocking; seconds for a long
/// take. With `with_voice`, RNNoise's voice probability is measured too (for
/// [`super::Method::Voice`]); it roughly doubles the time.
///
/// A clip with noise reduction is analysed through its cleaned file, where the
/// pauses are quieter and the threshold easier to set.
pub fn silence_analyse(
    state: &Arc<AppState>,
    segment_id: String,
    with_voice: bool,
    cancel: &AtomicBool,
) -> Result<Analysis, String> {
    let (sound, path, source, timeline_start, speed) = state.with_project(|project| {
        let sound = audible_segment(project, &segment_id).ok_or("the clip has no sound")?;
        let (_, picked) = project.segment(&segment_id).ok_or("unknown clip")?;
        let (_, heard) = project.segment(&sound).ok_or("unknown clip")?;
        let (original, _) = original_source(project, heard).ok_or("the clip has no sound")?;
        let path = effective_source(project, heard, &original).path;
        Ok::<_, String>((
            sound,
            path,
            picked.source_range,
            picked.target_range.start,
            picked.speed,
        ))
    })??;
    let envelope = envelope_from_file(&path, source, with_voice, cancel)?;
    Ok(Analysis {
        suggested_threshold_db: suggest_threshold(&envelope),
        segment_id,
        sound_segment_id: sound,
        source,
        timeline_start,
        speed,
        envelope,
    })
}

/// The pauses in an analysis, as source-time cuts.
pub fn silence_detect(analysis: &Analysis, params: &SilenceParams) -> Vec<TimeRange> {
    detect(&analysis.envelope, params)
        .into_iter()
        .filter_map(|cut| cut.intersect(&analysis.source))
        .collect()
}

/// Cuts for every filler word in the clip's transcript, or an explanation of
/// why there is no transcript to read.
pub fn silence_filler_cuts(
    state: &Arc<AppState>,
    segment_id: String,
    language: String,
    padding: Micros,
) -> Result<Vec<TimeRange>, String> {
    let provider = word_timings()
        .ok_or("filler words are found in a transcript — generate captions for this clip first")?;
    let words = state
        .with_project(|project| provider.words(project, &segment_id))?
        .ok_or("this clip has no transcript yet — generate captions for it first")?;
    let source = state
        .with_project(|project| project.segment(&segment_id).map(|(_, s)| s.source_range))?
        .ok_or("unknown clip")?;
    Ok(filler_cuts(&words, &language, padding)
        .into_iter()
        .filter_map(|cut| cut.intersect(&source))
        .collect())
}

/// Remove `cuts` (source time) from `segment_id` and its linked partners and
/// close the gaps — one undo step. `label` is what the undo menu says.
pub fn silence_remove(
    state: &Arc<AppState>,
    segment_id: String,
    cuts: Vec<TimeRange>,
    label: String,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let plan = remove_ranges(project, &segment_id, &cuts, &label)?;
        state.history.write().apply(project, plan.command)?;
    }
    crate::modules::voice::commands::respond(state)
}
