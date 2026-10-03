//! The `command` surface for voice cleanup.
//!
//! Both edits do slow work first — a denoise render, a loudness measurement —
//! with **no lock held**, and only then take the project lock to apply one
//! undoable edit. A UI calls them off its main thread; a CLI just waits.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::cleanup::{
    audible_segment, cleanup_of, original_source, set_cleanup_command, Denoise, Normalize,
    VoiceCleanup,
};
use super::denoise::{self, ENGINE};
use crate::modules::loudness::measure::measure_file;
use crate::modules::loudness::normalize::MAX_GAIN_DB;
use crate::modules::loudness::TRUE_PEAK_CEILING;
use crate::modules::project::document::{Micros, TimeRange};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// Everything a cleanup command needs from the document, cloned out of it.
struct Target {
    segment_id: String,
    original: String,
    duration: Micros,
    range: TimeRange,
    current: VoiceCleanup,
}

fn target(state: &AppState, segment_id: &str) -> Result<Target, String> {
    state.with_project(|project| {
        let audible = audible_segment(project, segment_id).ok_or("the clip has no sound")?;
        let (_, segment) = project.segment(&audible).ok_or("unknown clip")?;
        let (original, duration) =
            original_source(project, segment).ok_or("the clip has no sound")?;
        Ok(Target {
            current: cleanup_of(project, segment)
                .map(|(_, c)| c)
                .unwrap_or_default(),
            segment_id: audible,
            original,
            duration,
            range: segment.source_range,
        })
    })?
}

/// The cleanup a clip carries now, or `None`. Read-only; for the panel.
pub fn voice_cleanup(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<Option<VoiceCleanup>, String> {
    let t = target(state, &segment_id)?;
    Ok(Some(t.current).filter(|c| !c.is_identity()))
}

/// Turn noise reduction on at `strength` (`0..=1`), change its strength, or
/// turn it off with `None`.
///
/// Renders the cleaned file first if the cache does not have it (`progress`
/// gets `0..=1`), and re-measures a normalised clip, because removing noise
/// changes its loudness.
pub fn voice_set_denoise(
    state: &Arc<AppState>,
    segment_id: String,
    strength: Option<f32>,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<EditResponse, String> {
    let t = target(state, &segment_id)?;
    let mut cleanup = t.current.clone();
    cleanup.denoise = strength.map(|s| Denoise {
        strength: denoise::quantize(s),
        engine: ENGINE.into(),
    });
    let source = match &cleanup.denoise {
        Some(d) => {
            let cached = denoise::render(&t.original, t.duration, d.strength, cancel, progress)?;
            cached.to_string_lossy().into_owned()
        }
        None => t.original.clone(),
    };
    if let Some(normalize) = &cleanup.normalize {
        cleanup.normalize = Some(measure_normalize(
            &source,
            t.range,
            normalize.target_lufs,
            cancel,
        )?);
    }
    apply(state, &t.segment_id, cleanup)
}

/// Bring a clip to `target_lufs`, or remove its normalisation with `None`.
pub fn voice_normalize(
    state: &Arc<AppState>,
    segment_id: String,
    target_lufs: Option<f32>,
    cancel: &AtomicBool,
) -> Result<EditResponse, String> {
    let t = target(state, &segment_id)?;
    let mut cleanup = t.current.clone();
    cleanup.normalize = match target_lufs {
        None => None,
        Some(target) => {
            if !target.is_finite() || !(-40.0..=-5.0).contains(&target) {
                return Err("a loudness target is between −40 and −5 LUFS".into());
            }
            let source = cleanup
                .denoise
                .as_ref()
                .map(|d| denoise::cache_path(&t.original, d.strength, &d.engine))
                .filter(|p| p.is_file())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| t.original.clone());
            Some(measure_normalize(&source, t.range, target, cancel)?)
        }
    };
    apply(state, &t.segment_id, cleanup)
}

/// The gain that brings `range` of `path` to `target`, capped so its true
/// peak stays under the ceiling.
///
/// Capped rather than limited: a clip gain cannot limit, and an export
/// without a loudness target clamps the mix, so a gain that pushed peaks over
/// full scale would come out as hard clipping. A clip that cannot reach the
/// target without clipping gets as close as its peaks allow, and the panel
/// shows where it landed.
fn measure_normalize(
    path: &str,
    range: TimeRange,
    target: f32,
    cancel: &AtomicBool,
) -> Result<Normalize, String> {
    let loudness = measure_file(path, range, cancel)?;
    let measured = loudness
        .integrated
        .ok_or("the clip is silent; there is nothing to normalise")?;
    let mut gain = target as f64 - measured;
    let headroom = TRUE_PEAK_CEILING - loudness.true_peak_db;
    if gain > headroom {
        gain = headroom;
    }
    let gain = gain.clamp(-MAX_GAIN_DB, MAX_GAIN_DB);
    Ok(Normalize {
        target_lufs: target,
        gain_db: (gain * 100.0).round() as f32 / 100.0,
        measured_lufs: Some((measured * 10.0).round() as f32 / 10.0),
    })
}

fn apply(
    state: &Arc<AppState>,
    segment_id: &str,
    cleanup: VoiceCleanup,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = set_cleanup_command(project, segment_id, Some(cleanup))?;
        // The entry goes into the pool before the command runs, and comes back
        // out if the edit is refused: the contract `inspector_set_color` keeps
        // for the same reason.
        if let Some((id, value)) = entry {
            project.materials.extras.insert(id.clone(), value);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.extras.remove(&id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(state)
}

pub(crate) fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);
    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}
