//! Cloud tools beyond translation, speech and stock: generated sound
//! effects and music, fal.ai processing of a clip, and the credits the
//! licences on the timeline ask for.
//!
//! Like `cloud.rs` these use the accounts the app's Settings, Accounts page
//! stores; the CLI never adds one.

use std::path::PathBuf;

use chukcut_engine::modules::cloud::commands as cloud_commands;
use chukcut_engine::modules::cloud::jobs::JobEvent;
use chukcut_engine::modules::cloud::providers::fal::MediaFacts;
use chukcut_engine::modules::cloud::{AudioGenRequest, Capability, CloudStore};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::cloud::{account, import_and_place, NEVER};
use super::{Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::{absolute, Session};
use crate::values::Time;
use crate::On;

/// Make a sound effect, or a piece of music, from a description with a
/// cloud account that can (ElevenLabs), and import it. With `at`, it also
/// goes on the timeline there.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SoundArgs {
    /// What it should sound like.
    pub prompt: String,
    /// Make music instead of a sound effect.
    #[arg(long)]
    #[serde(default)]
    pub music: bool,
    /// How long, in seconds; the provider decides when left out.
    #[arg(long)]
    pub seconds: Option<f32>,
    /// Sound effects: make it loop seamlessly.
    #[arg(long)]
    #[serde(default)]
    pub looping: bool,
    /// Music: no vocals.
    #[arg(long)]
    #[serde(default)]
    pub instrumental: bool,
    /// The cloud account id; the first one that can when left out.
    #[arg(long)]
    pub account: Option<String>,
    /// Put it on the timeline at this time.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for SoundArgs {
    const NAME: &'static str = "sound";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        if self.prompt.trim().is_empty() {
            return Err(CliError::usage("describe the sound to make"));
        }
        let capability = if self.music {
            Capability::Music
        } else {
            Capability::SoundEffects
        };
        let store = CloudStore::user();
        let account = account(&store, self.account.as_deref(), capability)?;
        let request = AudioGenRequest {
            prompt: self.prompt.trim().to_string(),
            duration_seconds: self.seconds,
            looping: self.looping,
            instrumental: self.instrumental,
            prompt_influence: None,
        };
        ctx.progress(
            if self.music {
                "Making the music"
            } else {
                "Making the sound"
            },
            None,
        );
        let generated = cloud_commands::cloud_generate_audio(
            &store,
            &account,
            capability,
            &request,
            &cloud_commands::generated_root(),
        )?;
        let (material, clip) = import_and_place(session, &generated.path, self.at)?;
        Ok(Outcome::changed(
            format!("made {}", generated.path.display()),
            json!({
                "path": generated.path,
                "material": material,
                "clip": clip,
                "licence": generated.origin.licence,
            }),
        ))
    }
}

/// Run a fal.ai action on a video clip's file (`catalog fal_actions` lists
/// them). Without `confirm` this only asks fal for the price; with it the
/// clip is uploaded, processed (this costs money on the account) and the
/// result imported and put on the timeline at the clip's start, on a lane
/// of its own. The clip itself is not changed.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FalArgs {
    /// The video clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The action's id.
    #[arg(long)]
    pub action: String,
    /// Run it and pay for it. Without this, only the price is shown.
    #[arg(long)]
    #[serde(default)]
    pub confirm: bool,
    /// The cloud account id; the first fal account when left out.
    #[arg(long)]
    pub account: Option<String>,
}

impl Operation for FalArgs {
    const NAME: &'static str = "fal";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let (path, media, start) = session
            .with(|p| {
                let (_, segment) = p.segment(&id)?;
                let video = p.materials.video(&segment.material_id)?;
                let bytes = std::fs::metadata(&video.path).map(|m| m.len()).unwrap_or(0);
                Some((
                    PathBuf::from(&video.path),
                    MediaFacts {
                        duration_seconds: video.duration as f64 / 1_000_000.0,
                        width: video.width,
                        height: video.height,
                        fps: video.fps,
                        bytes,
                    },
                    segment.target_range.start,
                ))
            })
            .ok_or_else(|| CliError::usage(format!("{} is not a video clip", self.clip)))?;
        let store = CloudStore::user();
        let account = account(&store, self.account.as_deref(), Capability::Process)?;
        let action = self.action.trim();
        let estimate = cloud_commands::cloud_fal_estimate(&store, &account, action, &media)?;
        if !self.confirm {
            return Ok(Outcome::read(
                format!("{}; run it with --confirm", estimate.summary),
                json!({"estimate": estimate, "ran": false}),
            ));
        }
        let events = |event: JobEvent| {
            let label = match &event {
                JobEvent::Uploading { sent, total } => {
                    return ctx.progress(
                        "Uploading the clip",
                        (*total > 0).then(|| *sent as f32 / *total as f32),
                    )
                }
                JobEvent::Queued { .. } => "Waiting in fal's queue",
                JobEvent::Running { .. } => "Working",
                JobEvent::Downloading { .. } => "Downloading the result",
                JobEvent::Done => "Done",
            };
            ctx.progress(label, None);
        };
        let result = cloud_commands::cloud_fal_run(
            &store,
            &account,
            action,
            &path,
            Some(&estimate),
            &cloud_commands::generated_root(),
            &events,
            &NEVER,
        )?;
        let (material, clip) = import_and_place(session, &result, Some(Time::Micros(start)))?;
        Ok(Outcome::changed(
            format!("{action} done: {}", result.display()),
            json!({
                "estimate": estimate, "ran": true, "path": result,
                "material": material, "clip": clip,
            }),
        ))
    }
}

/// What the licences of the media on the timeline ask of an export: which
/// items need a credit, which are not for commercial use, and the credits
/// text. With `write`, the credits file goes beside that video file.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CreditsArgs {
    /// An exported video: write `<name>.credits.txt` beside it.
    #[arg(long)]
    pub write: Option<PathBuf>,
}

impl Operation for CreditsArgs {
    const NAME: &'static str = "credits";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let summary = cloud_commands::cloud_licence_summary(&session.state)?;
        let text = cloud_commands::cloud_credits_text(&session.state)?;
        let mut data = json!({"summary": summary, "text": text});
        let mut message = if text.trim().is_empty() {
            "nothing on the timeline needs a credit".to_string()
        } else {
            text.clone()
        };
        if let Some(video) = &self.write {
            let video = absolute(video);
            let written = cloud_commands::cloud_write_credits(&session.state, &video)?;
            data["written"] = json!(written);
            if let Some(path) = &written {
                message.push_str(&format!("\nwrote {}", path.display()));
            }
        }
        Ok(Outcome::read(message, data))
    }
}

/// The voices a speech account offers, for `catalog voices`.
pub fn voices(account_id: Option<&str>) -> CliResult<Value> {
    let store = CloudStore::user();
    let account = account(&store, account_id, Capability::Tts)?;
    let voices = cloud_commands::cloud_voices(&store, &account)?;
    Ok(json!({"account": account, "voices": voices}))
}

/// The `cloud` subcommands added here.
#[derive(Subcommand)]
pub enum CloudMoreCommand {
    /// Make a sound effect or music from a description and import it.
    Sound(On<SoundArgs>),
    /// Price, or run, a fal.ai action on a video clip.
    Fal(On<FalArgs>),
    /// Show what the media's licences ask for, and write the credits file.
    Credits(On<CreditsArgs>),
}

impl CloudMoreCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Sound(o) => crate::on(o, dry, ctx),
            Self::Fal(o) => crate::on(o, dry, ctx),
            Self::Credits(o) => crate::on(o, dry, ctx),
        }
    }
}
