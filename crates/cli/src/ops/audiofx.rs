//! Audio tools: a clip's audio effect stack, the voice changer, the "change
//! audio pitch" switch, auto-ducking and voiceover recording. Every operation
//! calls `chukcut_engine::modules::audiofx::commands`, the functions the
//! app's Audio tab calls.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chukcut_engine::modules::audiofx::commands as fx;
use chukcut_engine::modules::audiofx::{descriptor, AudioFx, DuckParams};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::values::{parse_assignment, Time};

static NEVER: AtomicBool = AtomicBool::new(false);

/// The clip that carries `clip`'s sound, and its audio settings.
fn view(session: &Session, clip: &str) -> CliResult<(String, AudioFx)> {
    let id = session.with(|p| select::clip(p, clip))?;
    let view = fx::audiofx_get(&session.state, id)?;
    Ok((view.segment_id, view.fx))
}

/// An effect in a stack by its index, its kind or its id.
fn effect_ref(fx: &AudioFx, reference: &str) -> CliResult<String> {
    let reference = reference.trim();
    if let Ok(index) = reference.parse::<usize>() {
        return fx.effects.get(index).map(|e| e.id.clone()).ok_or_else(|| {
            CliError::usage(format!(
                "the clip has {} audio effect(s); there is no index {index}",
                fx.effects.len()
            ))
        });
    }
    if let Some(e) = fx.effects.iter().find(|e| e.id == reference) {
        return Ok(e.id.clone());
    }
    let mut by_kind = fx.effects.iter().filter(|e| e.kind == reference);
    match (by_kind.next(), by_kind.next()) {
        (Some(e), None) => Ok(e.id.clone()),
        (Some(_), Some(_)) => Err(CliError::usage(format!(
            "the clip has more than one {reference}; name it by index or id"
        ))),
        _ => Err(CliError::usage(format!(
            "the clip has no audio effect {reference:?}"
        ))),
    }
}

fn numbers(set: &[(String, String)]) -> CliResult<Vec<(String, f32)>> {
    set.iter()
        .map(|(k, v)| {
            v.trim()
                .parse::<f32>()
                .map(|n| (k.clone(), n))
                .map_err(|_| CliError::usage(format!("{k}={v}: audio effect values are numbers")))
        })
        .collect()
}

fn set_params(
    session: &Session,
    clip: &str,
    effect: &str,
    params: &[(String, f32)],
) -> CliResult<()> {
    for (name, value) in params {
        fx::audiofx_set_param(
            &session.state,
            clip.to_string(),
            effect.to_string(),
            name.clone(),
            *value,
        )?;
    }
    Ok(())
}

/// Add an audio effect to a clip's sound: eq3, eq5, compressor, reverb,
/// delay, pitch or a voice preset (`catalog audio` lists them with their
/// parameters). Parameters not given keep their defaults.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AudioEffectAddArgs {
    /// The clip: id, id prefix or `lane:index`. A video clip's sound is its
    /// linked audio clip when it has one.
    pub clip: String,
    /// The effect kind, e.g. reverb.
    pub kind: String,
    /// Parameters to set, as name=value.
    #[arg(long = "set", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "super::assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub set: Vec<(String, String)>,
}

impl Operation for AudioEffectAddArgs {
    const NAME: &'static str = "audio_effect_add";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let params = numbers(&self.set)?;
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let (_, effect_id) = fx::audiofx_add(&session.state, id.clone(), self.kind.clone())?;
        set_params(session, &id, &effect_id, &params)?;
        let (clip, fx) = view(session, &id)?;
        Ok(Outcome::changed(
            format!("added {}", self.kind),
            json!({"clip": clip, "effect_id": effect_id, "audio": fx}),
        ))
    }
}

/// Change an audio effect on a clip: set parameters, switch it on or off, or
/// move it in the stack.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AudioEffectSetArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's audio stack, its kind, or its id.
    pub effect: String,
    /// Parameters to set, as name=value.
    #[arg(long = "set", value_parser = parse_assignment)]
    #[serde(default, deserialize_with = "super::assignments")]
    #[schemars(with = "crate::values::Assignments")]
    pub set: Vec<(String, String)>,
    /// Switch the effect on (true) or off (false).
    #[arg(long)]
    pub enabled: Option<bool>,
    /// Move the effect to this position in the stack (0 is first).
    #[arg(long)]
    pub index: Option<usize>,
}

impl Operation for AudioEffectSetArgs {
    const NAME: &'static str = "audio_effect_set";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let params = numbers(&self.set)?;
        if params.is_empty() && self.enabled.is_none() && self.index.is_none() {
            return Err(CliError::usage(
                "audio effect set needs --set, --enabled or --index",
            ));
        }
        let (clip, current) = view(session, &self.clip)?;
        let effect = effect_ref(&current, &self.effect)?;
        set_params(session, &clip, &effect, &params)?;
        if let Some(enabled) = self.enabled {
            let is = current.effect(&effect).map(|e| e.enabled);
            if is != Some(enabled) {
                fx::audiofx_set_enabled(&session.state, clip.clone(), effect.clone(), enabled)?;
            }
        }
        if let Some(index) = self.index {
            let at = current.effects.iter().position(|e| e.id == effect);
            if at != Some(index) {
                fx::audiofx_move(&session.state, clip.clone(), effect.clone(), index)?;
            }
        }
        let (clip, fx) = view(session, &clip)?;
        Ok(Outcome::changed(
            "changed the audio effect",
            json!({"clip": clip, "effect_id": effect, "audio": fx}),
        ))
    }
}

/// Take an audio effect off a clip.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AudioEffectRemoveArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The effect: its index in the clip's audio stack, its kind, or its id.
    pub effect: String,
}

impl Operation for AudioEffectRemoveArgs {
    const NAME: &'static str = "audio_effect_remove";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (clip, current) = view(session, &self.clip)?;
        let effect = effect_ref(&current, &self.effect)?;
        fx::audiofx_remove(&session.state, clip.clone(), effect)?;
        let (clip, fx) = view(session, &clip)?;
        Ok(Outcome::changed(
            "removed the audio effect",
            json!({"clip": clip, "audio": fx}),
        ))
    }
}

/// List a clip's audio settings: its effect stack (with every parameter's
/// value), the pitch switch and any ducking.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AudioEffectsArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
}

impl Operation for AudioEffectsArgs {
    const NAME: &'static str = "audio_effects";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (clip, fx) = view(session, &self.clip)?;
        let effects: Vec<_> = fx
            .effects
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let values: serde_json::Map<String, serde_json::Value> = descriptor(&e.kind)
                    .map(|d| {
                        d.params
                            .iter()
                            .map(|p| (p.id.to_string(), json!(e.value(p.id))))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({"index": i, "id": e.id, "kind": e.kind, "enabled": e.enabled, "params": values})
            })
            .collect();
        Ok(Outcome::read(
            format!("{} audio effect(s)", effects.len()),
            json!({
                "clip": clip,
                "effects": effects,
                "pitch_follows_speed": fx.pitch_follows_speed,
                "ducking": fx.ducking,
            }),
        ))
    }
}

/// The voice changer: give a clip's voice one preset (deep, chipmunk, robot,
/// telephone, megaphone), or take it off with --off.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct VoiceArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// deep, chipmunk, robot, telephone or megaphone.
    pub preset: Option<String>,
    /// How much of the preset, 0..1. Default 1.
    #[arg(long)]
    pub intensity: Option<f32>,
    /// Remove the voice preset.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
}

impl Operation for VoiceArgs {
    const NAME: &'static str = "voice";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let kind = match (&self.preset, self.off) {
            (_, true) => None,
            (Some(p), false) => {
                let p = p.trim().to_ascii_lowercase();
                Some(if p.starts_with("voice_") {
                    p
                } else {
                    format!("voice_{p}")
                })
            }
            (None, false) => return Err(CliError::usage("name a preset, or pass --off")),
        };
        fx::audiofx_set_voice(&session.state, id.clone(), kind.clone())?;
        if let (Some(kind), Some(intensity)) = (&kind, self.intensity) {
            let (clip, current) = view(session, &id)?;
            let effect = effect_ref(&current, kind)?;
            fx::audiofx_set_param(&session.state, clip, effect, "intensity".into(), intensity)?;
        }
        let (clip, fx) = view(session, &id)?;
        Ok(Outcome::changed(
            match &kind {
                Some(k) => format!("voice set to {}", k.trim_start_matches("voice_")),
                None => "voice preset removed".to_string(),
            },
            json!({"clip": clip, "audio": fx}),
        ))
    }
}

/// CapCut's "Change audio pitch": with --follow-speed true a speed change
/// moves the pitch like a tape; false (the default for every clip) keeps the
/// pitch and stretches time.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct AudioPitchArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// Whether the pitch follows the speed.
    #[arg(long)]
    pub follow_speed: bool,
}

impl Operation for AudioPitchArgs {
    const NAME: &'static str = "audio_pitch";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        fx::audiofx_set_pitch_follows_speed(&session.state, id.clone(), self.follow_speed)?;
        let (clip, fx) = view(session, &id)?;
        Ok(Outcome::changed(
            if self.follow_speed {
                "the pitch follows the speed"
            } else {
                "the pitch is kept"
            },
            json!({"clip": clip, "audio": fx}),
        ))
    }
}

/// Auto-ducking: turn a music clip down wherever someone speaks on another
/// lane (speech found by level and RNNoise's voice detector), written as
/// volume keyframes. Ducking again starts from the clip's own volume; --off
/// gives it back.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct DuckArgs {
    /// The music clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// How far the music goes down, in dB. Default 12.
    #[arg(long)]
    pub depth: Option<f32>,
    /// How long it takes to go down, ending where speech starts. Default 250ms.
    #[arg(long)]
    pub attack: Option<Time>,
    /// How long it takes to come back up. Default 500ms.
    #[arg(long)]
    pub release: Option<Time>,
    /// Speech quieter than this (dBFS) is ignored. Default -45.
    #[arg(long, allow_hyphen_values = true)]
    pub threshold: Option<f32>,
    /// Remove the ducking instead.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
}

impl Operation for DuckArgs {
    const NAME: &'static str = "duck";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        if self.off {
            fx::audiofx_unduck(&session.state, id.clone())?;
            return Ok(Outcome::changed("ducking removed", json!({"clip": id})));
        }
        let fps = session.fps();
        let defaults = DuckParams::default();
        let params = DuckParams {
            depth_db: self.depth.unwrap_or(defaults.depth_db),
            attack: self.attack.map_or(defaults.attack, |t| t.resolve(fps)),
            release: self.release.map_or(defaults.release, |t| t.resolve(fps)),
            threshold_db: self.threshold.unwrap_or(defaults.threshold_db),
        };
        ctx.progress("Finding speech", None);
        fx::audiofx_duck(&session.state, id.clone(), params, &NEVER)?;
        let (clip, fx) = view(session, &id)?;
        let keys = session.with(|p| {
            p.segment(&clip).and_then(|(_, s)| {
                s.keyframes
                    .iter()
                    .find(|k| {
                        k.property == chukcut_engine::modules::project::AnimatableProperty::Volume
                    })
                    .map(|k| k.keyframes.len())
            })
        });
        Ok(Outcome::changed(
            format!(
                "ducked by {} dB ({} volume keyframes)",
                params.depth_db,
                keys.unwrap_or(0)
            ),
            json!({"clip": clip, "ducking": fx.ducking, "volume_keyframes": keys.unwrap_or(0)}),
        ))
    }
}

/// Record a voiceover from the default input device and put it on a new
/// audio lane at a time. Blocks for the count-in plus the duration. The file
/// is saved next to the project, in "<project name> Media/".
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RecordArgs {
    /// How long to record.
    #[arg(long)]
    pub duration: Time,
    /// Where the take starts on the timeline. Default 0.
    #[arg(long)]
    pub at: Option<Time>,
    /// Seconds to wait before recording starts. Default 0.
    #[arg(long)]
    pub count_in: Option<f32>,
}

impl Operation for RecordArgs {
    const NAME: &'static str = "record";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let duration = self.duration.resolve(fps);
        if duration <= 0 {
            return Err(CliError::usage("record needs a --duration above zero"));
        }
        let count_in = Duration::from_secs_f32(self.count_in.unwrap_or(0.0).max(0.0));
        let path = fx::audiofx_take_path(&session.state);
        let recorder = chukcut_engine::modules::audiofx::record::Recorder::start(&path, count_in)?;
        ctx.progress("Recording", None);
        std::thread::sleep(count_in + Duration::from_micros(duration as u64));
        let take = recorder.stop()?;
        let at = self.at.map_or(0, |t| t.resolve(fps));
        fx::audiofx_place_take(&session.state, &take, at)?;
        Ok(Outcome::changed(
            format!("recorded {:.1} s", take.duration as f64 / 1e6),
            json!({"take": take}),
        ))
    }
}
