//! The `command` surface for audio tools: what the inspector, the CLI and the
//! MCP server call.
//!
//! Every edit acts on the clip that is *heard* — the audio partner of a linked
//! picture (`voice::audible_segment`) — and lands as one undo step. The slow
//! parts (ducking's speech analysis, a render to warm the preview's cache)
//! run with no lock held.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use crate::modules::project::document::{
    new_id, AnimatableProperty, KeyframeTrack, Micros, Segment, TimeRange, Track, TrackKind,
    Transform,
};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::modules::voice::commands::respond;
use crate::modules::voice::{audible_segment, effective_source};
use crate::state::AppState;

use super::catalog::{self, descriptor, EffectDescriptor};
use super::ducking::{duck_keyframes, speech_ranges};
use super::model::{fx_or_default, set_fx_command, AudioEffect, AudioFx, DuckParams, Ducking};
use super::record::Take;

/// A clip's audio settings, as the inspector reads them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioFxView {
    /// The clip that carries them: the heard one.
    pub segment_id: String,
    pub fx: AudioFx,
}

/// Every effect there is, with its parameters.
pub fn audiofx_catalog() -> &'static [EffectDescriptor] {
    catalog::catalog()
}

fn audible(state: &AppState, segment_id: &str) -> Result<(String, AudioFx), String> {
    state.with_project(|project| {
        let id = audible_segment(project, segment_id).ok_or("the clip has no sound")?;
        let (_, segment) = project.segment(&id).ok_or("unknown clip")?;
        Ok((id, fx_or_default(project, segment)))
    })?
}

/// Apply `fx` (and `mutate`) to `segment_id` as one undo step.
fn apply(
    state: &Arc<AppState>,
    segment_id: &str,
    fx: AudioFx,
    label: &str,
    mutate: impl Fn(&mut Segment),
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = set_fx_command(project, segment_id, fx, label, mutate)?;
        // Into the pool before the command runs, out again if it is refused:
        // the order `voice::commands` and `fx::commands` keep.
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

/// The audio settings of `segment_id`'s sound.
pub fn audiofx_get(state: &Arc<AppState>, segment_id: String) -> Result<AudioFxView, String> {
    let (segment_id, fx) = audible(state, &segment_id)?;
    Ok(AudioFxView { segment_id, fx })
}

/// Add an effect of `kind` at its defaults to the end of the stack. Answers
/// the new effect's id with the edit.
pub fn audiofx_add(
    state: &Arc<AppState>,
    segment_id: String,
    kind: String,
) -> Result<(EditResponse, String), String> {
    let desc =
        descriptor(&kind).ok_or_else(|| format!("there is no audio effect called {kind}"))?;
    let (id, mut fx) = audible(state, &segment_id)?;
    let effect = AudioEffect::new(desc.id);
    let effect_id = effect.id.clone();
    fx.effects.push(effect);
    let response = apply(state, &id, fx, &format!("Add {}", desc.label), |_| {})?;
    Ok((response, effect_id))
}

fn label_of(fx: &AudioFx, effect_id: &str) -> Result<&'static str, String> {
    let effect = fx
        .effect(effect_id)
        .ok_or("that effect is not on this clip")?;
    Ok(descriptor(&effect.kind).map_or("effect", |d| d.label))
}

pub fn audiofx_remove(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
) -> Result<EditResponse, String> {
    let (id, mut fx) = audible(state, &segment_id)?;
    let label = label_of(&fx, &effect_id)?;
    fx.effects.retain(|e| e.id != effect_id);
    apply(state, &id, fx, &format!("Remove {label}"), |_| {})
}

/// Set one parameter. Out-of-range values are clamped to the catalog's range.
pub fn audiofx_set_param(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    param: String,
    value: f32,
) -> Result<EditResponse, String> {
    let (id, mut fx) = audible(state, &segment_id)?;
    let label = label_of(&fx, &effect_id)?;
    let effect = fx.effect_mut(&effect_id).expect("checked above");
    let spec = descriptor(&effect.kind)
        .and_then(|d| d.param(&param))
        .ok_or_else(|| format!("{label} has no parameter {param}"))?;
    if !value.is_finite() {
        return Err(format!("{} must be a finite number", spec.label));
    }
    effect.params.insert(param, value);
    apply(state, &id, fx, &format!("Change {label}"), |_| {})
}

/// Switch an effect off and on without losing its settings.
pub fn audiofx_set_enabled(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    enabled: bool,
) -> Result<EditResponse, String> {
    let (id, mut fx) = audible(state, &segment_id)?;
    let label = label_of(&fx, &effect_id)?;
    fx.effect_mut(&effect_id).expect("checked above").enabled = enabled;
    let verb = if enabled { "Enable" } else { "Disable" };
    apply(state, &id, fx, &format!("{verb} {label}"), |_| {})
}

/// Move an effect to `index` in the stack (clamped), which changes the order
/// the sound goes through them.
pub fn audiofx_move(
    state: &Arc<AppState>,
    segment_id: String,
    effect_id: String,
    index: usize,
) -> Result<EditResponse, String> {
    let (id, mut fx) = audible(state, &segment_id)?;
    let label = label_of(&fx, &effect_id)?;
    let from = fx
        .effects
        .iter()
        .position(|e| e.id == effect_id)
        .expect("checked above");
    let effect = fx.effects.remove(from);
    let to = index.min(fx.effects.len());
    fx.effects.insert(to, effect);
    apply(state, &id, fx, &format!("Reorder {label}"), |_| {})
}

/// The voice changer: replace whatever voice preset the clip has with `kind`
/// (one of [`catalog::VOICE_PRESETS`]), or remove it with `None`. A voice is
/// one choice, like CapCut's grid, so there is never more than one.
pub fn audiofx_set_voice(
    state: &Arc<AppState>,
    segment_id: String,
    kind: Option<String>,
) -> Result<EditResponse, String> {
    if let Some(kind) = &kind {
        if !catalog::VOICE_PRESETS.contains(&kind.as_str()) {
            return Err(format!("{kind} is not a voice preset"));
        }
    }
    let (id, mut fx) = audible(state, &segment_id)?;
    let slot = fx
        .effects
        .iter()
        .position(|e| catalog::VOICE_PRESETS.contains(&e.kind.as_str()));
    fx.effects
        .retain(|e| !catalog::VOICE_PRESETS.contains(&e.kind.as_str()));
    let label = match &kind {
        Some(kind) => {
            let at = slot.unwrap_or(fx.effects.len()).min(fx.effects.len());
            fx.effects.insert(at, AudioEffect::new(kind));
            format!("Voice: {}", descriptor(kind).map_or("", |d| d.label))
        }
        None => "Remove voice effect".to_string(),
    };
    apply(state, &id, fx, &label, |_| {})
}

/// CapCut's "Change audio pitch": with `true`, a speed change moves the pitch
/// as a tape would; with `false` (the default) the pitch is kept.
pub fn audiofx_set_pitch_follows_speed(
    state: &Arc<AppState>,
    segment_id: String,
    follows: bool,
) -> Result<EditResponse, String> {
    let (id, mut fx) = audible(state, &segment_id)?;
    fx.pitch_follows_speed = follows;
    let label = if follows {
        "Change audio pitch with speed"
    } else {
        "Keep audio pitch"
    };
    apply(state, &id, fx, label, |_| {})
}

/// Duck `segment_id` (a music clip) under the speech on other lanes: `Volume`
/// keyframes down by the depth wherever someone speaks, in one undo step.
/// Ducking a clip again starts from its own volume, not the last ducking.
///
/// Slow (decodes and analyses the speech); call it off the UI thread.
pub fn audiofx_duck(
    state: &Arc<AppState>,
    segment_id: String,
    params: DuckParams,
    cancel: &AtomicBool,
) -> Result<EditResponse, String> {
    if !(0.0..=60.0).contains(&params.depth_db) || params.attack < 0 || params.release < 0 {
        return Err(
            "ducking takes a depth of 0 to 60 dB and attack and release times of zero or more"
                .into(),
        );
    }
    let (id, fx) = audible(state, &segment_id)?;
    let project = state.project.read().clone().ok_or("no project is open")?;
    let speech = speech_ranges(&project, &id, &params, cancel)?;
    if speech.is_empty() {
        return Err("no speech was found under this clip on the other lanes".into());
    }
    let (_, segment) = project.segment(&id).ok_or("unknown clip")?;
    let before: Vec<_> = match &fx.ducking {
        Some(ducking) => ducking.before.clone(),
        None => segment
            .keyframes
            .iter()
            .find(|k| k.property == AnimatableProperty::Volume)
            .map(|k| k.keyframes.clone())
            .unwrap_or_default(),
    };
    let keys = duck_keyframes(segment, &speech, &params, &before);
    let mut fx = fx;
    fx.ducking = Some(Ducking { params, before });
    apply(state, &id, fx, "Duck music under speech", move |s| {
        set_volume_keys(s, keys.clone())
    })
}

/// Take a clip's ducking off, giving it back the volume keyframes it had.
pub fn audiofx_unduck(state: &Arc<AppState>, segment_id: String) -> Result<EditResponse, String> {
    let (id, mut fx) = audible(state, &segment_id)?;
    let ducking = fx.ducking.take().ok_or("the clip is not ducked")?;
    apply(state, &id, fx, "Remove ducking", move |s| {
        set_volume_keys(s, ducking.before.clone())
    })
}

fn set_volume_keys(segment: &mut Segment, keys: Vec<crate::modules::project::document::Keyframe>) {
    segment
        .keyframes
        .retain(|k| k.property != AnimatableProperty::Volume);
    if !keys.is_empty() {
        segment.keyframes.push(KeyframeTrack {
            property: AnimatableProperty::Volume,
            keyframes: keys,
        });
    }
}

/// Render `segment_id`'s processed sound into the preview's cache now, and
/// answer where it is (`None` when the clip needs no render). The preview
/// does this by itself in the background; this is for a caller that wants to
/// wait for it, such as a CLI script or a test.
pub fn audiofx_render(
    state: &Arc<AppState>,
    segment_id: String,
    cancel: &AtomicBool,
) -> Result<Option<String>, String> {
    let spec = state.with_project(|project| {
        let id = audible_segment(project, &segment_id).ok_or("the clip has no sound")?;
        let (_, segment) = project.segment(&id).ok_or("unknown clip")?;
        let (path, _) = crate::modules::voice::cleanup::original_source(project, segment)
            .ok_or("the clip has no sound")?;
        let effective = effective_source(project, segment, &path);
        Ok::<_, String>(super::render::spec_for(project, segment, &effective.path))
    })??;
    match spec {
        None => Ok(None),
        Some(spec) => super::cache::render_to_cache(&spec, cancel)
            .map(|p| Some(p.to_string_lossy().into_owned())),
    }
}

/// Where a new take for the open project is saved.
pub fn audiofx_take_path(state: &Arc<AppState>) -> std::path::PathBuf {
    let project_path = state.project_path.read().clone();
    let dir = super::record::recordings_dir(project_path.as_deref());
    super::record::take_path(&dir)
}

/// Put a finished take on the timeline at `at`, on a new audio lane below the
/// existing ones: the punch-in. The file joins the media library like an
/// import; the lane and the clip are one undo step.
pub fn audiofx_place_take(
    state: &Arc<AppState>,
    take: &Take,
    at: Micros,
) -> Result<EditResponse, String> {
    if at < 0 {
        return Err("a take cannot start before the beginning of the timeline".into());
    }
    let info = crate::modules::media::probe(&take.path).map_err(|e| e.to_string())?;
    let name = std::path::Path::new(&take.path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Voiceover".into());
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let material =
            crate::modules::project::commands::import_material(project, &take.path, &name, &info)?;
        let duration = if material.duration > 0 {
            material.duration
        } else {
            take.duration
        };
        let index = project
            .tracks
            .iter()
            .rposition(|t| t.kind == TrackKind::Audio)
            .map_or(project.tracks.len(), |i| i + 1);
        let count = project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .count();
        let track = Track::new(TrackKind::Audio, format!("Voiceover {}", count + 1));
        let track_id = track.id.clone();
        let segment = Segment {
            id: new_id(),
            material_id: material.id,
            target_range: TimeRange::new(at, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        let command = EditCommand::Composite {
            label: "Record voiceover".into(),
            commands: vec![
                EditCommand::AddTrack { track, index },
                EditCommand::InsertSegment {
                    track_id,
                    index: 0,
                    segment,
                },
            ],
        };
        state.history.write().apply(project, command)?;
    }
    respond(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{AudioMaterial, CanvasConfig, Project};

    fn state() -> Arc<AppState> {
        let state = AppState::new();
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.audios.push(AudioMaterial {
            id: "a".into(),
            path: "/nonexistent/a.wav".into(),
            duration: 10_000_000,
            sample_rate: 48_000,
            channels: 2,
        });
        let mut lane = Track::new(TrackKind::Audio, "A1");
        lane.segments.push(Segment {
            id: "s".into(),
            material_id: "a".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        p.tracks.push(lane);
        *state.project.write() = Some(p);
        state
    }

    #[test]
    fn a_stack_is_built_edited_reordered_and_undone_one_step_at_a_time() {
        let state = state();
        let (_, eq) = audiofx_add(&state, "s".into(), "eq3".into()).unwrap();
        let (_, verb) = audiofx_add(&state, "s".into(), "reverb".into()).unwrap();
        audiofx_set_param(&state, "s".into(), eq.clone(), "low".into(), 6.0).unwrap();
        audiofx_move(&state, "s".into(), verb.clone(), 0).unwrap();
        audiofx_set_enabled(&state, "s".into(), eq.clone(), false).unwrap();

        let view = audiofx_get(&state, "s".into()).unwrap();
        let kinds: Vec<_> = view.fx.effects.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["reverb", "eq3"]);
        assert_eq!(view.fx.effects[1].value("low"), Some(6.0));
        assert!(!view.fx.effects[1].enabled);

        // Five edits, five undo steps.
        for _ in 0..5 {
            crate::modules::timeline::commands::timeline_undo(&state).unwrap();
        }
        assert!(audiofx_get(&state, "s".into()).unwrap().fx.is_identity());
        assert!(audiofx_set_param(&state, "s".into(), "nope".into(), "low".into(), 1.0).is_err());
        assert!(audiofx_add(&state, "s".into(), "flanger".into()).is_err());
    }

    #[test]
    fn the_voice_changer_holds_one_voice_at_a_time() {
        let state = state();
        audiofx_set_voice(&state, "s".into(), Some("voice_robot".into())).unwrap();
        audiofx_set_voice(&state, "s".into(), Some("voice_deep".into())).unwrap();
        let fx = audiofx_get(&state, "s".into()).unwrap().fx;
        assert_eq!(fx.effects.len(), 1);
        assert_eq!(fx.effects[0].kind, "voice_deep");
        audiofx_set_voice(&state, "s".into(), None).unwrap();
        assert!(audiofx_get(&state, "s".into())
            .unwrap()
            .fx
            .effects
            .is_empty());
        assert!(audiofx_set_voice(&state, "s".into(), Some("reverb".into())).is_err());
    }

    #[test]
    fn the_pitch_switch_is_stored_on_the_clip() {
        let state = state();
        audiofx_set_pitch_follows_speed(&state, "s".into(), true).unwrap();
        assert!(
            audiofx_get(&state, "s".into())
                .unwrap()
                .fx
                .pitch_follows_speed
        );
        assert!(audiofx_set_pitch_follows_speed(&state, "s".into(), true).is_err());
    }
}
