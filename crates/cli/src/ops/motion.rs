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
/// of holding each frame; `none` switches it off. Without --mode, says what
/// the clip has. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct FrameBlendArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// none or blend.
    #[arg(long)]
    pub mode: Option<String>,
}

impl Operation for FrameBlendArgs {
    const NAME: &'static str = "frame_blend";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let current = speed::speed_frame_blend(&session.state, id.clone())?;
        let Some(mode) = &self.mode else {
            return Ok(Outcome::read(
                format!("frame blending is {}", current.name()),
                json!({"clip": session.with(|p| summary::clip_by_id(p, &id)), "frame_blend": current.name()}),
            ));
        };
        let mode = FrameBlend::parse(mode).map_err(CliError::usage)?;
        if mode == current {
            return Ok(Outcome::read(
                format!("frame blending is already {}", mode.name()),
                json!({"clip": session.with(|p| summary::clip_by_id(p, &id)), "frame_blend": mode.name()}),
            ));
        }
        speed::speed_set_frame_blend(&session.state, id.clone(), mode)?;
        Ok(Outcome::changed(
            format!("frame blending {}", mode.name()),
            json!({"clip": session.with(|p| summary::clip_by_id(p, &id)), "frame_blend": mode.name()}),
        ))
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
