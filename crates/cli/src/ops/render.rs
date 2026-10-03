//! Rendering: the export, and single frames.

use std::path::PathBuf;
use std::sync::mpsc;

use chukcut_engine::modules::captions::commands as caption_commands;
use chukcut_engine::modules::captions::srt::SubtitleFormat;
use chukcut_engine::modules::export::commands as export_commands;
use chukcut_engine::modules::export::presets::{AudioCodec, Container, Quality, VideoCodec};
use chukcut_engine::modules::export::{
    ExportOverrides, ExportProgress, ExportRequest, ExportStage,
};
use chukcut_engine::shell::Channel;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::{enum_named, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::session::{absolute, Session};
use crate::values::{seconds, Time};

/// Render the timeline to a video file. Blocks until it is written; progress
/// goes to stderr. Software encoding unless `hardware` names an encoder
/// (`catalog hardware`), or "auto" picks the first usable one.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ExportArgs {
    /// The video file to write. Its extension is corrected to the container.
    pub output: PathBuf,
    /// youtube_1080p, youtube_4k, vertical_1080x1920, instagram_square or
    /// custom (the project's own canvas and rate; the default).
    #[arg(long)]
    pub preset: Option<String>,
    /// A hardware encoder id such as nvenc_h264 or vaapi_h265, or "auto".
    #[arg(long)]
    pub hardware: Option<String>,
    #[arg(long)]
    pub width: Option<u32>,
    #[arg(long)]
    pub height: Option<u32>,
    #[arg(long)]
    pub fps: Option<f64>,
    /// h264, h265, vp9 or av1.
    #[arg(long)]
    pub codec: Option<String>,
    /// Constant quality in the codec's scale (lower is better; 18-23 for h264).
    #[arg(long, conflicts_with = "bitrate")]
    pub crf: Option<u8>,
    /// Average video bitrate in bits per second.
    #[arg(long)]
    pub bitrate: Option<u64>,
    /// mp4, mov, mkv or webm.
    #[arg(long)]
    pub container: Option<String>,
    /// aac, opus or none.
    #[arg(long)]
    pub audio_codec: Option<String>,
    /// Leave the audio out.
    #[arg(long)]
    #[serde(default)]
    pub no_audio: bool,
    /// Bring the mix to this integrated loudness in LUFS (-14 for most
    /// platforms), with true peaks at -1 dBTP.
    #[arg(long, allow_hyphen_values = true)]
    pub loudness: Option<f32>,
    /// Export only from this time...
    #[arg(long)]
    pub from: Option<Time>,
    /// ...to this time.
    #[arg(long)]
    pub to: Option<Time>,
    /// Also write the captions next to the video as .srt (or .vtt).
    #[arg(long)]
    pub sidecar: Option<String>,
}

impl Operation for ExportArgs {
    const NAME: &'static str = "export";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let fps = session.fps();
        let hardware = match self.hardware.as_deref() {
            None => None,
            // Probing trial-encodes on every candidate, so only when asked.
            Some(h) if h.eq_ignore_ascii_case("auto") => export_commands::export_presets()
                .hardware
                .iter()
                .find(|e| e.usable)
                .map(|e| e.id.clone()),
            Some(h) if h.eq_ignore_ascii_case("none") || h.eq_ignore_ascii_case("software") => None,
            Some(h) => Some(h.to_string()),
        };
        let overrides = ExportOverrides {
            width: self.width,
            height: self.height,
            fps: self.fps,
            video_codec: self
                .codec
                .as_deref()
                .map(|c| {
                    enum_named::<VideoCodec>("video codec", c, &["h264", "h265", "vp9", "av1"])
                })
                .transpose()?,
            quality: match (self.crf, self.bitrate) {
                (Some(crf), _) => Some(Quality::Crf(crf)),
                (None, Some(b)) => Some(Quality::Bitrate(b)),
                _ => None,
            },
            audio_codec: self
                .audio_codec
                .as_deref()
                .map(|c| enum_named::<AudioCodec>("audio codec", c, &["aac", "opus", "none"]))
                .transpose()?,
            container: self
                .container
                .as_deref()
                .map(|c| enum_named::<Container>("container", c, &["mp4", "mov", "mkv", "webm"]))
                .transpose()?,
            loudness_target: self.loudness,
            ..Default::default()
        };
        if let Some(l) = self.loudness {
            if !(-40.0..=-5.0).contains(&l) {
                return Err(CliError::usage(
                    "a loudness target is between -40 and -5 LUFS",
                ));
            }
        }
        let duration = session.with(|p| p.duration());
        let range = match (self.from, self.to) {
            (None, None) => None,
            (a, b) => Some((
                a.map_or(0, |t| t.resolve(fps)),
                b.map_or(duration, |t| t.resolve(fps)),
            )),
        };
        let output = absolute(&self.output);
        if let Some(dir) = output.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CliError::render(format!("cannot create {}: {e}", dir.display())))?;
        }
        let request = ExportRequest {
            output_path: output.to_string_lossy().into_owned(),
            preset_id: self.preset.clone(),
            overrides: Some(overrides),
            hardware: hardware.clone(),
            include_audio: !self.no_audio,
            range,
        };

        let (tx, rx) = mpsc::channel::<ExportProgress>();
        let channel = Channel::new(move |p| tx.send(p).is_ok());
        export_commands::export_start(&session.state, request, channel)
            .map_err(CliError::render)?;

        let finished = loop {
            let progress = rx
                .recv()
                .map_err(|_| CliError::render("the export stopped without saying why"))?;
            if progress.stage.is_terminal() {
                break progress;
            }
            let label = match progress.stage {
                ExportStage::Preparing => "Preparing".to_string(),
                ExportStage::MixingAudio => "Mixing audio".to_string(),
                ExportStage::Finalizing => "Finishing the file".to_string(),
                _ => format!(
                    "Encoding frame {} of {} ({:.0} fps)",
                    progress.frame, progress.total_frames, progress.fps
                ),
            };
            ctx.progress(&label, Some(progress.fraction));
        };
        match finished.stage {
            ExportStage::Done => {}
            ExportStage::Cancelled => return Err(CliError::render("the export was cancelled")),
            _ => {
                return Err(CliError::render(
                    finished
                        .message
                        .unwrap_or_else(|| "the export failed".to_string()),
                ))
            }
        }
        let path = finished
            .output_path
            .map(PathBuf::from)
            .unwrap_or_else(|| output.clone());
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        let mut data = json!({
            "path": path,
            "bytes": bytes,
            "frames": finished.total_frames,
            "seconds": finished.elapsed_seconds,
            "encode_fps": finished.fps,
            "hardware": hardware,
        });
        if let Some(format) = &self.sidecar {
            let format: SubtitleFormat = enum_named("subtitle format", format, &["srt", "vtt"])?;
            let sidecar = caption_commands::sidecar_path(&path, format);
            let count = caption_commands::captions_export(&session.state, &sidecar, Some(format))?;
            data["sidecar"] = json!({"path": sidecar, "captions": count});
        }
        Ok(Outcome::read(
            format!(
                "exported {} ({} frames, {:.1} MB) in {:.1} s",
                path.display(),
                finished.total_frames,
                bytes as f64 / 1e6,
                finished.elapsed_seconds
            ),
            data,
        ))
    }
}

/// Render one frame of the timeline as a PNG at the full canvas size.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RenderFrameArgs {
    /// The timeline time to render.
    #[arg(long)]
    pub at: Time,
    /// The PNG to write.
    pub output: PathBuf,
}

impl Operation for RenderFrameArgs {
    const NAME: &'static str = "render_frame";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let at = self.at.resolve(session.fps());
        let path = render_png(session, at, &absolute(&self.output))?;
        Ok(Outcome::read(
            format!("rendered {:.3} s to {}", seconds(at), path.display()),
            json!({"path": path, "at": seconds(at)}),
        ))
    }
}

/// Render the frame at `at` into `output`; returns the path written.
pub fn render_png(session: &Session, at: i64, output: &std::path::Path) -> CliResult<PathBuf> {
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError::render(format!("cannot create {}: {e}", dir.display())))?;
    }
    let written = pollster::block_on(export_commands::export_snapshot(
        &session.state,
        at,
        output.to_string_lossy().into_owned(),
    ))
    .map_err(CliError::render)?;
    Ok(PathBuf::from(written))
}
