//! Frame blending and animated-sticker playback from the command line.
//!
//! Motion blur needs nothing here: it is the `motion_blur` effect in the
//! catalogue, so `effect-add <clip> motion_blur --set shutter=270` adds it.

use chukcut_engine::modules::animated::commands as animated;
use chukcut_engine::modules::animated::playback::Playback;
use chukcut_engine::modules::speed::blend::FrameBlend;
use chukcut_engine::modules::speed::commands as speed;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::Session;

/// Frame blending for a video clip: `blend` mixes the two source frames
/// around each instant, so slow motion and speed ramps play smoothly instead
/// of holding each frame; `flow` ("Optical flow (AI)") shows frames RIFE
/// makes between the source frames instead (the ML worker; the model
/// downloads on first use, a GPU bundle makes it 10–30 times faster than
/// the CPU) and bakes them before it returns; `none` switches it off.
/// Without --mode, says what the clip has and, for `flow`, how many of its
/// frames are baked. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FrameBlendArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// none, blend or flow.
    #[arg(long)]
    pub mode: Option<String>,
    /// With flow: set it and return without baking (an export bakes what is
    /// missing).
    #[arg(long)]
    #[serde(default)]
    pub no_bake: bool,
}

/// Wait for optical-flow bake `job`, with progress, and say what it did.
fn wait_for_flow(ctx: &Ctx, job: Option<u64>) -> CliResult<serde_json::Value> {
    let Some(job) = job else {
        return Ok(json!({"baked": 0, "note": "every frame was baked already"}));
    };
    let mut warned = false;
    let done = speed::speed_flow_wait(job, |status| {
        let p = &status.progress;
        if let (Some(warning), false) = (&status.warning, warned) {
            ctx.progress(warning, None);
            warned = true;
        }
        if p.total > 0 {
            ctx.progress(
                &format!("Making slow-motion frames: {} of {}", p.done, p.total),
                Some(p.done as f32 / p.total as f32),
            );
        }
    })
    .map_err(|error| {
        CliError::refused(format!(
            "optical flow is on, but its frames could not be made: {error}"
        ))
    })?;
    Ok(json!({
        "baked": done.written,
        "seconds": done.seconds,
        "provider": done.provider,
        "model_ms_per_frame": done.model_millis_per_frame,
        "cancelled": done.cancelled,
    }))
}

fn coverage(session: &Session, id: &str) -> serde_json::Value {
    match speed::speed_flow_coverage(&session.state, id.to_string()) {
        Ok(c) => json!({"baked": c.baked, "total": c.total}),
        Err(error) => json!({"error": error}),
    }
}

impl Operation for FrameBlendArgs {
    const NAME: &'static str = "frame_blend";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let current = speed::speed_frame_blend(&session.state, id.clone())?;
        let Some(mode) = &self.mode else {
            let mut value = json!({"clip": session.with(|p| summary::clip_by_id(p, &id)), "frame_blend": current.name()});
            let mut message = format!("frame blending is {}", current.name());
            if current == FrameBlend::Flow {
                value["flow"] = coverage(session, &id);
                // The docs promise how many frames are baked; say it here too,
                // not only in the JSON.
                if let (Some(baked), Some(total)) = (
                    value["flow"]["baked"].as_u64(),
                    value["flow"]["total"].as_u64(),
                ) {
                    message.push_str(&format!("; {baked} of {total} frames baked"));
                }
            }
            return Ok(Outcome::read(message, value));
        };
        let mode = FrameBlend::parse(mode).map_err(CliError::usage)?;
        let changed = mode != current;
        if changed {
            speed::speed_set_frame_blend(&session.state, id.clone(), mode)?;
        }
        let mut value = json!({"clip": session.with(|p| summary::clip_by_id(p, &id)), "frame_blend": mode.name()});
        // Flow again on a clip that has it bakes what is missing (after a
        // speed change, a trim or a cleared cache).
        if mode == FrameBlend::Flow && !self.no_bake {
            let job = speed::speed_flow_bake(&session.state, id.clone())?;
            value["bake"] = wait_for_flow(ctx, job)?;
            value["flow"] = coverage(session, &id);
        }
        let message = match (changed, mode) {
            (false, FrameBlend::Flow) if !self.no_bake => "optical flow frames baked".to_string(),
            (false, _) => format!("frame blending is already {}", mode.name()),
            (true, _) => format!("frame blending {}", mode.name()),
        };
        Ok(if changed {
            Outcome::changed(message, value)
        } else {
            Outcome::read(message, value)
        })
    }
}

/// "Smooth slow-mo": switch on optical flow ("Optical flow (AI)", frames
/// made by RIFE between the source frames) and slow the clip down to
/// --speed, or to 0.5x when it is not slowed yet (a clip already below 1x
/// or on a speed curve keeps its speed). Bakes the new frames before it
/// returns. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SmoothSlowMoArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// The speed to slow to, below 1 (e.g. 0.25).
    #[arg(long)]
    pub speed: Option<f32>,
    /// Set it and return without baking (an export bakes what is missing).
    #[arg(long)]
    #[serde(default)]
    pub no_bake: bool,
}

impl Operation for SmoothSlowMoArgs {
    const NAME: &'static str = "smooth_slow_mo";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let response = speed::speed_smooth_slow_mo(&session.state, id.clone(), self.speed)?;
        let edited = response.edit.is_some();
        let mut value =
            json!({"clip": session.with(|p| summary::clip_by_id(p, &id)), "frame_blend": "flow"});
        if self.no_bake {
            if let Some(job) = response.job {
                speed::speed_flow_cancel(job);
            }
        } else {
            value["bake"] = wait_for_flow(ctx, response.job)?;
        }
        value["flow"] = coverage(session, &id);
        let message = "smooth slow motion".to_string();
        Ok(if edited {
            Outcome::changed(message, value)
        } else {
            Outcome::read(message, value)
        })
    }
}

/// An animated sticker (Lottie, animated GIF or WebP): `loop` plays it again
/// and again for as long as the clip lasts, `once` plays it one time and
/// holds the last frame. Without --mode, says what the clip has and how long
/// one pass of its animation is. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StickerPlaybackArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// loop or once.
    #[arg(long)]
    pub mode: Option<String>,
}

impl Operation for StickerPlaybackArgs {
    const NAME: &'static str = "sticker_playback";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let path = session
            .with(|p| {
                p.segment(&id)
                    .and_then(|(_, s)| p.materials.image(&s.material_id))
                    .map(|image| image.path.clone())
            })
            .ok_or_else(|| CliError::usage("the clip is not a sticker"))?;
        let info = animated::animated_info(std::path::Path::new(&path))
            .map_err(|e| CliError::usage(format!("the clip is not an animated sticker: {e}")))?;
        let current = animated::animated_playback(&session.state, id.clone())?;
        let data = |p: Playback, s: &Session| {
            json!({
                "clip": s.with(|pr| summary::clip_by_id(pr, &id)),
                "playback": p.name(),
                "animation": info,
            })
        };
        let Some(mode) = &self.mode else {
            return Ok(Outcome::read(
                format!(
                    "the sticker plays {}; one pass is {:.3} s",
                    current.name(),
                    info.duration as f64 / 1e6
                ),
                data(current, session),
            ));
        };
        let mode = Playback::parse(mode).map_err(CliError::usage)?;
        if mode == current {
            return Ok(Outcome::read(
                format!("the sticker already plays {}", mode.name()),
                data(mode, session),
            ));
        }
        animated::animated_set_playback(&session.state, id.clone(), mode)?;
        Ok(Outcome::changed(
            format!("the sticker plays {}", mode.name()),
            data(mode, session),
        ))
    }
}
