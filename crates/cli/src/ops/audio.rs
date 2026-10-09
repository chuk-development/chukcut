//! Sound and words: captions, silence cutting, loudness, noise reduction.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::captions::commands as caption_commands;
use chukcut_engine::modules::captions::edit::PlaceOptions;
use chukcut_engine::modules::captions::srt::SubtitleFormat;
use chukcut_engine::modules::captions::{CaptionMode, CaptionStyle, Placement};
use chukcut_engine::modules::loudness::commands as loudness_commands;
use chukcut_engine::modules::project::{Project, TextAlign, TimeRange};
use chukcut_engine::modules::silence::commands as silence_commands;
use chukcut_engine::modules::silence::{Method, SilenceParams};
use chukcut_engine::modules::speech::commands as speech_commands;
use chukcut_engine::modules::speech::{Backend, LocalModel, SpeechSettings};
use chukcut_engine::modules::voice::cleanup::audible_segment;
use chukcut_engine::modules::voice::commands as voice_commands;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{enum_named, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::select;
use crate::session::{absolute, Session};
use crate::values::{hex, parse_color, seconds, Time};

/// Nothing in a one-shot invocation cancels; the flag exists because the
/// engine's long commands take one.
static NEVER: AtomicBool = AtomicBool::new(false);

/// How existing captions and emoji are treated when new ones are placed.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct Placing {
    /// Keep the captions already on the lane instead of replacing them.
    #[arg(long)]
    #[serde(default)]
    pub keep: bool,
    /// Append one fitting emoji to each caption.
    #[arg(long)]
    #[serde(default)]
    pub emoji: bool,
    /// A style preset for the new captions: classic, karaoke, yellow, boxed
    /// or big. Defaults to the style of the captions already there.
    #[arg(long)]
    pub preset: Option<String>,
}

impl Placing {
    fn options(&self) -> PlaceOptions {
        PlaceOptions {
            replace: !self.keep,
            auto_emoji: self.emoji,
        }
    }

    fn style(&self, project: &Project) -> CliResult<Option<CaptionStyle>> {
        let Some(name) = &self.preset else {
            return Ok(None);
        };
        let presets = CaptionStyle::presets(&project.canvas);
        presets
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, s)| Some(s.clone()))
            .ok_or_else(|| {
                let names: Vec<String> = presets.iter().map(|(n, _)| n.to_lowercase()).collect();
                CliError::usage(format!(
                    "there is no caption preset {name}; choose {}",
                    names.join(", ")
                ))
            })
    }
}

fn captions_added(added: caption_commands::CaptionsAdded, what: &str) -> Outcome {
    let n = added.segment_ids.len();
    Outcome::changed(
        format!("{what}: {n} caption(s)"),
        json!({"track_id": added.track_id, "clips": added.segment_ids}),
    )
}

/// Transcribe the timeline's speech and put it on the caption lane, with
/// word timings for karaoke highlighting. Uses the transcriber chosen in the
/// app's caption panel unless told otherwise: a cloud account
/// (OpenAI-compatible), or whisper.cpp on this machine.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsTranscribeArgs {
    /// cloud or local.
    #[arg(long)]
    pub backend: Option<String>,
    /// The cloud account id (set up in the app's Accounts settings).
    #[arg(long)]
    pub account: Option<String>,
    /// Override the cloud account's transcription model.
    #[arg(long)]
    pub model: Option<String>,
    /// The local model: tiny, base, small, medium or large_v3_turbo.
    #[arg(long)]
    pub local_model: Option<String>,
    /// ISO 639-1 language code; detected when left out.
    #[arg(long)]
    pub language: Option<String>,
    /// Word captions with at most this many words on screen (1 is the
    /// one-word-at-a-time look). Without it, sentence captions.
    #[arg(long)]
    pub words: Option<usize>,
    #[command(flatten)]
    #[serde(flatten)]
    pub placing: Placing,
}

impl Operation for CaptionsTranscribeArgs {
    const NAME: &'static str = "captions_transcribe";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let mut settings = SpeechSettings::load();
        if let Some(b) = &self.backend {
            settings.backend = enum_named::<Backend>("backend", b, &["cloud", "local"])?;
        }
        if let Some(a) = &self.account {
            settings.account = Some(a.clone());
        }
        if let Some(m) = &self.model {
            settings.model = Some(m.clone());
        }
        if let Some(m) = &self.local_model {
            settings.local_model = enum_named::<LocalModel>(
                "local model",
                m,
                &["tiny", "base", "small", "medium", "large_v3_turbo"],
            )?;
        }
        if let Some(l) = &self.language {
            settings.language = Some(l.clone());
        }
        if settings.backend == Backend::Local && !speech_commands::speech_local_available() {
            return Err(CliError::refused(
                "this build has no local transcription; use --backend cloud",
            ));
        }
        let mode = match self.words {
            Some(max_words) => CaptionMode::Words {
                max_words: max_words.max(1),
            },
            None => match settings.mode {
                m @ CaptionMode::Sentences { .. } => m,
                CaptionMode::Words { .. } => CaptionMode::default(),
            },
        };
        let progress = |p: speech_commands::SpeechProgress| ctx.progress(&p.label, p.fraction);
        let transcript =
            speech_commands::speech_transcribe(&session.state, &settings, &progress, &NEVER)?;
        let style = session.with(|p| self.placing.style(p))?;
        let added = caption_commands::captions_from_transcript(
            &session.state,
            &transcript,
            mode,
            style,
            self.placing.options(),
        )?;
        let mut outcome = captions_added(added, "transcribed");
        outcome.data["language"] = json!(transcript.language);
        if settings.backend == Backend::Local {
            outcome.data["device"] = json!(speech_commands::speech_device_known());
        }
        Ok(outcome)
    }
}

/// Read an .srt or .vtt file onto the caption lane.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsImportArgs {
    /// The subtitle file.
    pub file: PathBuf,
    #[command(flatten)]
    #[serde(flatten)]
    pub placing: Placing,
}

impl Operation for CaptionsImportArgs {
    const NAME: &'static str = "captions_import";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let style = session.with(|p| self.placing.style(p))?;
        let added = caption_commands::captions_import(
            &session.state,
            &absolute(&self.file),
            style,
            self.placing.options(),
        )?;
        Ok(captions_added(added, "imported"))
    }
}

/// Write the project's captions as an .srt or .vtt file.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsExportArgs {
    /// Where to write. The format follows the extension unless given.
    pub file: PathBuf,
    /// srt or vtt.
    #[arg(long)]
    pub format: Option<String>,
}

impl Operation for CaptionsExportArgs {
    const NAME: &'static str = "captions_export";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let format = self
            .format
            .as_deref()
            .map(|f| enum_named::<SubtitleFormat>("subtitle format", f, &["srt", "vtt"]))
            .transpose()?;
        let path = absolute(&self.file);
        let count = caption_commands::captions_export(&session.state, &path, format)?;
        Ok(Outcome::read(
            format!("wrote {count} caption(s) to {}", path.display()),
            json!({"path": path, "count": count}),
        ))
    }
}

/// Restyle captions: all of them, or one with `clip`. Start from a preset
/// and override single properties.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsStyleArgs {
    /// Only this caption: id, id prefix or `lane:index`.
    #[arg(long)]
    pub clip: Option<String>,
    /// classic, karaoke, yellow, boxed or big.
    #[arg(long)]
    pub preset: Option<String>,
    /// A font family installed on this machine (`catalog fonts`).
    #[arg(long)]
    pub font: Option<String>,
    /// Size in canvas pixels.
    #[arg(long)]
    pub size: Option<f32>,
    /// Text colour: #rrggbb, #rrggbbaa or a name.
    #[arg(long)]
    pub color: Option<String>,
    /// Karaoke: the colour of the word being spoken, or "none".
    #[arg(long)]
    pub highlight: Option<String>,
    /// Bold on or off.
    #[arg(long)]
    pub bold: Option<bool>,
    /// Italic on or off.
    #[arg(long)]
    pub italic: Option<bool>,
    /// left, center or right.
    #[arg(long)]
    pub align: Option<String>,
    /// Outline width in pixels; 0 for none.
    #[arg(long)]
    pub stroke_width: Option<f32>,
    /// Outline colour.
    #[arg(long)]
    pub stroke_color: Option<String>,
    /// A box behind the text: a colour, or "none".
    #[arg(long)]
    pub background: Option<String>,
    /// top, middle or bottom, or a y position in canvas units (+1 is the top).
    #[arg(long, allow_hyphen_values = true)]
    pub position: Option<String>,
}

impl Operation for CaptionsStyleArgs {
    const NAME: &'static str = "captions_style";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let clip = self
            .clip
            .as_deref()
            .map(|c| session.with(|p| select::clip(p, c)))
            .transpose()?;
        let mut style = caption_commands::captions_style_of(&session.state, clip.as_deref())?;
        if let Some(name) = &self.preset {
            let placing = Placing {
                preset: Some(name.clone()),
                ..Default::default()
            };
            if let Some(preset) = session.with(|p| placing.style(p))? {
                style = preset;
            }
        }
        if let Some(f) = &self.font {
            style.font_family = f.clone();
        }
        if let Some(s) = self.size {
            style.font_size = s;
        }
        if let Some(c) = &self.color {
            style.color = parse_color(c)?;
        }
        if let Some(h) = &self.highlight {
            style.highlight = if h.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(parse_color(h)?)
            };
        }
        if let Some(b) = self.bold {
            style.bold = b;
        }
        if let Some(i) = self.italic {
            style.italic = i;
        }
        if let Some(a) = &self.align {
            style.align = enum_named::<TextAlign>("alignment", a, &["left", "center", "right"])?;
        }
        if let Some(w) = self.stroke_width {
            style.stroke_width = w;
        }
        if let Some(c) = &self.stroke_color {
            style.stroke_color = parse_color(c)?;
        }
        if let Some(bg) = &self.background {
            style.background = if bg.eq_ignore_ascii_case("none") {
                None
            } else {
                Some(parse_color(bg)?)
            };
        }
        if let Some(pos) = &self.position {
            style.position[1] =
                match enum_named::<Placement>("placement", pos, &["top", "middle", "bottom"]) {
                    Ok(p) => p.y(),
                    Err(_) => pos
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| {
                            CliError::usage("--position is top, middle, bottom or a number")
                        })?,
                };
        }
        caption_commands::captions_set_style(&session.state, clip.as_deref(), &style)?;
        Ok(Outcome::changed(
            format!(
                "restyled {}",
                if clip.is_some() {
                    "the caption"
                } else {
                    "every caption"
                }
            ),
            json!({
                "font": style.font_family, "size": style.font_size, "color": hex(style.color),
                "highlight": style.highlight.map(hex), "position": style.position,
            }),
        ))
    }
}

/// List every caption with its time and words.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct CaptionsListArgs {}

impl Operation for CaptionsListArgs {
    const NAME: &'static str = "captions_list";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let clips = caption_commands::captions_list(&session.state)?;
        let data: Vec<Value> = clips
            .iter()
            .map(|c| {
                json!({
                    "id": c.segment_id, "start": seconds(c.start), "end": seconds(c.end),
                    "text": c.text, "words": c.words,
                })
            })
            .collect();
        Ok(Outcome::read(
            format!("{} caption(s)", clips.len()),
            json!(data),
        ))
    }
}

// ---------------------------------------------------------------------------
// Silence
// ---------------------------------------------------------------------------

/// The knobs shared by detect and remove.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SilenceKnobs {
    /// The clip whose sound is measured: id, id prefix or `lane:index`.
    pub clip: String,
    /// Quieter than this is a pause, in dBFS. Defaults to a level suggested
    /// from the recording.
    #[arg(long, allow_hyphen_values = true)]
    pub threshold: Option<f32>,
    /// Shorter pauses are kept. Default 0.5 s.
    #[arg(long)]
    pub min_silence: Option<Time>,
    /// Kept on the speech side of every cut. Default 0.12 s.
    #[arg(long)]
    pub padding: Option<Time>,
    /// Also treat stretches without a voice as pauses (breaths, room tone).
    #[arg(long)]
    #[serde(default)]
    pub voice: bool,
}

struct Detected {
    analysis: silence_commands::Analysis,
    cuts: Vec<TimeRange>,
    threshold: f32,
}

impl SilenceKnobs {
    fn detect(&self, session: &Session, ctx: &Ctx) -> CliResult<Detected> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        ctx.progress("Measuring the sound", None);
        let analysis = silence_commands::silence_analyse(&session.state, id, self.voice, &NEVER)?;
        let fps = session.fps();
        let defaults = SilenceParams::default();
        let threshold = self.threshold.unwrap_or(analysis.suggested_threshold_db);
        let params = SilenceParams {
            method: if self.voice {
                Method::Voice
            } else {
                Method::Energy
            },
            threshold_db: threshold,
            min_silence: self
                .min_silence
                .map_or(defaults.min_silence, |t| t.resolve(fps)),
            padding: self.padding.map_or(defaults.padding, |t| t.resolve(fps)),
        };
        let cuts = silence_commands::silence_detect(&analysis, &params);
        Ok(Detected {
            analysis,
            cuts,
            threshold,
        })
    }
}

fn describe(detected: &Detected) -> Value {
    let a = &detected.analysis;
    let total: i64 = detected.cuts.iter().map(|c| c.duration).sum();
    json!({
        "clip": a.segment_id,
        "threshold_db": detected.threshold,
        "suggested_threshold_db": a.suggested_threshold_db,
        "removable": seconds(total),
        "cuts": detected.cuts.iter().map(|c| json!({
            "start": seconds(a.timeline_time(c.start)),
            "end": seconds(a.timeline_time(c.end())),
            "source_start": seconds(c.start),
            "source_end": seconds(c.end()),
        })).collect::<Vec<_>>(),
    })
}

/// Find the pauses in a clip's sound. Read-only: lists what `silence remove`
/// would cut, in timeline and source time.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SilenceDetectArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub knobs: SilenceKnobs,
}

impl Operation for SilenceDetectArgs {
    const NAME: &'static str = "silence_detect";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let detected = self.knobs.detect(session, ctx)?;
        let data = describe(&detected);
        Ok(Outcome::read(
            format!(
                "{} pause(s), {:.2} s removable at {:.1} dB",
                detected.cuts.len(),
                data["removable"].as_f64().unwrap_or(0.0),
                detected.threshold
            ),
            data,
        ))
    }
}

/// Cut the pauses out of a clip (and its linked sound) and close the gaps,
/// as one undo step. Other lanes ripple with it so captions stay on their
/// words, unless `no_sync`. With `fillers`, filler words ("um", "uh") found
/// in the clip's captions are cut too.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SilenceRemoveArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub knobs: SilenceKnobs,
    /// Leave the other lanes where they are.
    #[arg(long)]
    #[serde(default)]
    pub no_sync: bool,
    /// Also cut filler words, from the clip's captions.
    #[arg(long)]
    #[serde(default)]
    pub fillers: bool,
    /// The language of the filler words (ISO 639-1). Default en.
    #[arg(long)]
    pub language: Option<String>,
}

impl Operation for SilenceRemoveArgs {
    const NAME: &'static str = "silence_remove";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let detected = self.knobs.detect(session, ctx)?;
        let mut cuts = detected.cuts.clone();
        let fps = session.fps();
        if self.fillers {
            let padding = self
                .knobs
                .padding
                .map_or(SilenceParams::default().padding, |t| t.resolve(fps))
                .min(60_000);
            let fillers = silence_commands::silence_filler_cuts(
                &session.state,
                detected.analysis.segment_id.clone(),
                self.language.clone().unwrap_or_else(|| "en".into()),
                padding,
            )?;
            cuts.extend(fillers);
        }
        if cuts.is_empty() {
            return Ok(Outcome::read("no pauses to cut", describe(&detected)));
        }
        let data = describe(&Detected {
            cuts: cuts.clone(),
            ..detected
        });
        silence_commands::silence_remove(
            &session.state,
            data["clip"].as_str().unwrap_or_default().to_string(),
            cuts.clone(),
            if self.fillers {
                "Remove pauses and fillers".into()
            } else {
                "Remove pauses".into()
            },
            !self.no_sync,
        )?;
        Ok(Outcome::changed(
            format!(
                "cut {} pause(s), {:.2} s",
                cuts.len(),
                data["removable"].as_f64().unwrap_or(0.0)
            ),
            data,
        ))
    }
}

// ---------------------------------------------------------------------------
// Loudness and cleanup
// ---------------------------------------------------------------------------

/// The distinct clips that make sound, as `audible_segment` resolves them.
fn audible_clips(project: &Project) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for track in &project.tracks {
        for segment in &track.segments {
            if let Some(id) = audible_segment(project, &segment.id) {
                let is_sound = project.segment(&id).is_some_and(|(_, s)| {
                    let pool = &project.materials;
                    pool.audio(&s.material_id).is_some()
                        || pool.video(&s.material_id).is_some_and(|v| v.has_audio)
                });
                if is_sound && !out.contains(&id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

/// Bring a clip's speech to a loudness (LUFS), measured and capped so its
/// peaks do not clip. Without `clip`, every clip that makes sound. `off`
/// removes the normalisation. For the whole mix at export, use
/// `export --loudness`.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct NormalizeArgs {
    /// The clip: id, id prefix or `lane:index`. Every sounding clip when
    /// left out.
    #[arg(long)]
    pub clip: Option<String>,
    /// The target in LUFS, between -40 and -5. Default -14, the streaming
    /// platforms' level.
    #[arg(long, allow_hyphen_values = true)]
    pub target: Option<f32>,
    /// Remove the normalisation instead.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
}

impl Operation for NormalizeArgs {
    const NAME: &'static str = "normalize";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let clips = match &self.clip {
            Some(c) => vec![session.with(|p| select::clip(p, c))?],
            None => session.with(audible_clips),
        };
        if clips.is_empty() {
            return Err(CliError::refused("no clip on the timeline makes sound"));
        }
        let target = if self.off {
            None
        } else {
            Some(self.target.unwrap_or(-14.0))
        };
        let mut results = Vec::new();
        for (i, id) in clips.iter().enumerate() {
            ctx.progress(
                &format!("Measuring clip {} of {}", i + 1, clips.len()),
                Some(i as f32 / clips.len() as f32),
            );
            voice_commands::voice_normalize(&session.state, id.clone(), target, &NEVER)?;
            let cleanup = voice_commands::voice_cleanup(&session.state, id.clone())?;
            results.push(json!({"clip": id, "normalize": cleanup.and_then(|c| c.normalize)}));
        }
        Ok(Outcome::changed(
            match target {
                Some(t) => format!("normalised {} clip(s) to {t} LUFS", clips.len()),
                None => format!("removed normalisation from {} clip(s)", clips.len()),
            },
            json!(results),
        ))
    }
}

/// Reduce background noise in a clip's speech (RNNoise), or turn it off.
/// Renders a cleaned copy of the sound into the cache the first time.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct DenoiseArgs {
    /// The clip: id, id prefix or `lane:index`.
    pub clip: String,
    /// How strongly, 0..1. Default 1.
    #[arg(long)]
    pub strength: Option<f32>,
    /// Turn noise reduction off.
    #[arg(long)]
    #[serde(default)]
    pub off: bool,
}

impl Operation for DenoiseArgs {
    const NAME: &'static str = "denoise";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let id = session.with(|p| select::clip(p, &self.clip))?;
        let strength = if self.off {
            None
        } else {
            Some(self.strength.unwrap_or(1.0).clamp(0.0, 1.0))
        };
        let progress = |f: f32| ctx.progress("Reducing noise", Some(f));
        voice_commands::voice_set_denoise(&session.state, id.clone(), strength, &NEVER, &progress)?;
        let cleanup = voice_commands::voice_cleanup(&session.state, id.clone())?;
        Ok(Outcome::changed(
            if self.off {
                "noise reduction off"
            } else {
                "noise reduced"
            },
            json!({"clip": id, "cleanup": cleanup}),
        ))
    }
}

/// Measure loudness (EBU R128): one clip's sound, or with no clip the whole
/// mix as it would export.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct LoudnessArgs {
    /// The clip: id, id prefix or `lane:index`.
    #[arg(long)]
    pub clip: Option<String>,
}

impl Operation for LoudnessArgs {
    const NAME: &'static str = "loudness";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        ctx.progress("Measuring loudness", None);
        let (loudness, extra) = match &self.clip {
            Some(c) => {
                let id = session.with(|p| select::clip(p, c))?;
                let m = loudness_commands::loudness_measure_clip(&session.state, id, &NEVER)?;
                (
                    m.loudness,
                    json!({"clip": m.segment_id, "gain_db": m.gain_db}),
                )
            }
            None => (
                loudness_commands::loudness_measure_mix(&session.state, &NEVER)?,
                json!({"mix": true}),
            ),
        };
        let mut data = json!({
            "integrated_lufs": loudness.integrated,
            "range_lu": loudness.range,
            "true_peak_db": loudness.true_peak_db,
        });
        if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
            d.extend(e.clone());
        }
        Ok(Outcome::read(
            match loudness.integrated {
                Some(l) => format!("{l:.1} LUFS, true peak {:.1} dBTP", loudness.true_peak_db),
                None => "silent".into(),
            },
            data,
        ))
    }
}
