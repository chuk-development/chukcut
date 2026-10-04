//! Machine-learning edits on a clip: isolate voice. Each is one engine
//! command (and one undo step); the slow part, a model run in the ML
//! worker, happens after the edit and is cached.

use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::voice::commands::{self as voice_commands, IsolationSetting};
use chukcut_engine::modules::voice::isolate::Keep;
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{enum_named, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;
use crate::On;

static NEVER: AtomicBool = AtomicBool::new(false);

/// Isolate the voice of a clip: separate speech from music and noise with a
/// model (HTDemucs, in the ML worker), or with --keep background keep only
/// the music. The isolated sound is rendered into the cache now unless
/// --no-render is given (then the app or the export renders it).
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct IsolateVoiceArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// How much of what is not kept goes, 0..1. Default 1.
    #[arg(long)]
    pub strength: Option<f32>,
    /// What to keep: voice (default) or background.
    #[arg(long)]
    pub keep: Option<String>,
    /// Turn voice isolation off.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
    /// Only change the setting; leave the render to the app or the export.
    #[arg(long)]
    #[serde(default)]
    pub no_render: bool,
}

impl Operation for IsolateVoiceArgs {
    const NAME: &'static str = "isolate_voice";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let setting = if self.off {
            None
        } else {
            let strength = self.strength.unwrap_or(1.0);
            if !(0.0..=1.0).contains(&strength) {
                return Err(CliError::usage("--strength is 0..1"));
            }
            let keep: Keep = match &self.keep {
                Some(k) => enum_named("part to keep", k, &["voice", "background"])?,
                None => Keep::Voice,
            };
            Some(IsolationSetting { strength, keep })
        };
        voice_commands::voice_set_isolation(&session.state, id.clone(), setting)?;
        let mut rendered = None;
        if setting.is_some() && !self.no_render {
            let progress = |f: f32| ctx.progress("Isolating the voice", Some(f));
            rendered = Some(voice_commands::voice_isolation_render(
                &session.state,
                id.clone(),
                &NEVER,
                &progress,
            )?);
        }
        let cleanup = voice_commands::voice_cleanup(&session.state, id.clone())?;
        Ok(Outcome::changed(
            match (&setting, &rendered) {
                (None, _) => "voice isolation off".to_string(),
                (Some(_), Some(r)) => format!("voice isolated in {:.1} s", r.seconds),
                (Some(_), None) => "voice isolation set; it renders on first use".to_string(),
            },
            json!({"clip": id, "cleanup": cleanup, "render": rendered}),
        ))
    }
}

/// Top-level subcommands for the ML edits.
#[derive(Subcommand)]
pub enum AiCommand {
    /// Separate a clip's voice from music and noise (or keep only the music).
    IsolateVoice(On<IsolateVoiceArgs>),
}

impl AiCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::IsolateVoice(o) => crate::on(o, dry, ctx),
        }
    }
}
