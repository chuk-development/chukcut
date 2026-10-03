//! The project as a whole: create, describe, configure, import, undo.

use std::path::PathBuf;

use chukcut_engine::modules::project::{commands as project_commands, ProjectConfig, Severity};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::gesture;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::session::{absolute, Session};
use crate::values::{parse_color, seconds};

/// Create a new, empty project file: one video lane and one audio lane. The
/// first video imported into it sets the canvas shape and frame rate, as in
/// the app, unless the canvas is changed first with `configure`.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct NewArgs {
    /// The project's name. Defaults to the file name.
    #[arg(long)]
    pub name: Option<String>,
    /// Canvas width in pixels.
    #[arg(long, default_value_t = 1080)]
    #[serde(default = "default_width")]
    pub width: u32,
    /// Canvas height in pixels.
    #[arg(long, default_value_t = 1920)]
    #[serde(default = "default_height")]
    pub height: u32,
    /// Frames per second.
    #[arg(long, default_value_t = 30.0)]
    #[serde(default = "default_fps")]
    pub fps: f64,
    /// Replace a file that already exists.
    #[arg(long)]
    #[serde(default)]
    pub force: bool,
}

fn default_width() -> u32 {
    1080
}
fn default_height() -> u32 {
    1920
}
fn default_fps() -> f64 {
    30.0
}

impl NewArgs {
    pub fn create(self, path: &std::path::Path) -> CliResult<(Session, Outcome)> {
        let path = absolute(path);
        if path.exists() && !self.force {
            return Err(CliError::project(format!(
                "{} already exists; pass --force to replace it",
                path.display()
            )));
        }
        let name = self.name.unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Untitled".into())
        });
        let session = Session::create(&path, name, self.width, self.height, self.fps)?;
        let outcome = Outcome::changed(
            format!(
                "created {} ({}x{}, {} fps)",
                path.display(),
                self.width,
                self.height,
                self.fps
            ),
            json!({"path": path, "width": self.width, "height": self.height, "fps": self.fps}),
        );
        Ok((session, outcome))
    }
}

/// Describe the project: canvas, every lane and clip (with the `ref` and `id`
/// other commands take), the imported media, validation issues and the undo
/// state.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct InfoArgs {
    /// Return the whole project document instead of the summary.
    #[arg(long)]
    #[serde(default)]
    pub full: bool,
}

impl Operation for InfoArgs {
    const NAME: &'static str = "info";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let project = session.project();
        let data = if self.full {
            serde_json::to_value(&project).map_err(|e| CliError::refused(e.to_string()))?
        } else {
            summary::project(&project, &session.state, &session.path)
        };
        let clips: usize = project.tracks.iter().map(|t| t.segments.len()).sum();
        Ok(Outcome::read(
            format!(
                "{}: {}x{} at {} fps, {:.2} s, {} lane(s), {} clip(s)",
                project.name,
                project.canvas.width,
                project.canvas.height,
                project.fps,
                seconds(project.duration()),
                project.tracks.len(),
                clips
            ),
            data,
        ))
    }
}

/// Check the project for problems: missing media (warnings) and
/// inconsistencies (errors). Fails with exit code 4 when there are errors.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ValidateArgs {}

impl Operation for ValidateArgs {
    const NAME: &'static str = "validate";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let issues = project_commands::project_validate(&session.state)?;
        let errors = issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count();
        let data = json!({
            "valid": errors == 0,
            "issues": issues.iter().map(|i| json!({"severity": i.severity, "message": i.message, "subject": i.subject_id})).collect::<Vec<_>>(),
        });
        if errors > 0 {
            let all: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
            return Err(CliError::new(
                crate::error::ErrorKind::Invalid,
                format!("the project has {errors} error(s): {}", all.join("; ")),
            ));
        }
        Ok(Outcome::read(
            format!("valid, {} warning(s)", issues.len()),
            data,
        ))
    }
}

/// Change the project's name, canvas size, frame rate or background colour,
/// as one undoable step. Changing the frame rate moves nothing: every time in
/// a project is in microseconds.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ConfigureArgs {
    /// The project's name.
    #[arg(long)]
    pub name: Option<String>,
    /// Canvas width in pixels; even.
    #[arg(long)]
    pub width: Option<u32>,
    /// Canvas height in pixels; even.
    #[arg(long)]
    pub height: Option<u32>,
    /// Frames per second.
    #[arg(long)]
    pub fps: Option<f64>,
    /// Canvas background: #rrggbb or r,g,b in 0..1.
    #[arg(long)]
    pub background: Option<String>,
}

impl Operation for ConfigureArgs {
    const NAME: &'static str = "configure";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let mut config = session.with(ProjectConfig::of);
        if let Some(name) = self.name {
            config.name = name;
        }
        if let Some(w) = self.width {
            config.width = w;
        }
        if let Some(h) = self.height {
            config.height = h;
        }
        if let Some(fps) = self.fps {
            config.fps = fps;
        }
        if let Some(bg) = self.background {
            let c = crate::values::srgb_to_linear(parse_color(&bg)?);
            config.background = c;
        }
        let before = session.with(ProjectConfig::of);
        let changed = before.name != config.name
            || before.width != config.width
            || before.height != config.height
            || before.fps != config.fps
            || before.background != config.background;
        project_commands::project_configure(&session.state, config.clone())?;
        Ok(Outcome {
            message: format!(
                "{}: {}x{} at {} fps",
                config.name, config.width, config.height, config.fps
            ),
            data: json!({"name": config.name, "width": config.width, "height": config.height, "fps": config.fps}),
            mutated: changed,
        })
    }
}

/// Add media files to the project's library. With `append`, also put each
/// one at the end of the first lane of its kind. Importing is not an undo
/// step, as in the app.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ImportArgs {
    /// Video, image or audio files.
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// Also append each file to the timeline.
    #[arg(long)]
    #[serde(default)]
    pub append: bool,
}

impl Operation for ImportArgs {
    const NAME: &'static str = "import";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.files.is_empty() {
            return Err(CliError::usage("import needs at least one file"));
        }
        let mut imported = Vec::new();
        for file in &self.files {
            let path = absolute(file);
            if !path.is_file() {
                return Err(CliError::refused(format!(
                    "there is no file at {}",
                    path.display()
                )));
            }
            // The project stores the path; `../media/a.mp4` resolved against
            // today's working directory is not one to keep.
            let path = path.canonicalize().unwrap_or(path);
            let material = pollster::block_on(project_commands::project_import_media(
                &session.state,
                path.to_string_lossy().into_owned(),
            ))
            .map_err(|e| CliError::refused(format!("{}: {e}", path.display())))?;
            let mut entry = serde_json::to_value(&material).unwrap_or(Value::Null);
            entry["duration"] = json!(seconds(material.duration));
            if self.append {
                let command = session.with(|p| gesture::append(p, &material.id))?;
                let segment_id = match &command {
                    chukcut_engine::modules::timeline::ops::EditCommand::InsertSegment {
                        segment,
                        ..
                    } => Some(segment.id.clone()),
                    _ => None,
                };
                timeline_commands::timeline_apply(&session.state, command)?;
                entry["clip"] = segment_id
                    .map(|id| session.with(|p| summary::clip_by_id(p, &id)))
                    .unwrap_or(Value::Null);
            }
            imported.push(entry);
        }
        let names: Vec<String> = imported
            .iter()
            .map(|m| m["name"].as_str().unwrap_or("?").to_string())
            .collect();
        Ok(Outcome::changed(
            format!(
                "imported {}{}",
                names.join(", "),
                if self.append {
                    " onto the timeline"
                } else {
                    ""
                }
            ),
            json!({"materials": imported}),
        ))
    }
}

/// Undo the last edit made in this session (a `batch` or an MCP
/// connection). A one-shot command starts with an empty history.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct UndoArgs {}

impl Operation for UndoArgs {
    const NAME: &'static str = "undo";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let label = session.state.history.read().undo_label();
        let Some(label) = label else {
            return Err(CliError::refused("there is nothing to undo"));
        };
        timeline_commands::timeline_undo(&session.state)?;
        Ok(Outcome::changed(
            format!("undid {label}"),
            json!({"undone": label}),
        ))
    }
}

/// Redo the last undone edit in this session.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct RedoArgs {}

impl Operation for RedoArgs {
    const NAME: &'static str = "redo";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let label = session.state.history.read().redo_label();
        let Some(label) = label else {
            return Err(CliError::refused("there is nothing to redo"));
        };
        timeline_commands::timeline_redo(&session.state)?;
        Ok(Outcome::changed(
            format!("redid {label}"),
            json!({"redone": label}),
        ))
    }
}

/// List what the engine offers: `effects`, `transitions`, `animations`,
/// `grade` controls, export `presets` and `hardware` encoders, caption
/// transcription `models`, `luts` in the library, `fonts`, `title_styles`,
/// `title_templates`, split-screen and picture-in-picture `layouts`,
/// `speed_presets`, cloud `accounts`, mask shapes (`masks`) or `blend` modes.
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct CatalogArgs {
    /// effects, audio, transitions, animations, grade, presets, hardware, models, luts, fonts,
    /// title_styles, title_templates, layouts, speed_presets, accounts, masks or blend.
    pub kind: String,
}

impl CatalogArgs {
    pub fn run(self) -> CliResult<Outcome> {
        use chukcut_engine::modules::{export, fx, inspector, motion, speech, text, transitions};
        let kind = self.kind.trim().to_ascii_lowercase();
        let data = match kind.as_str() {
            "effects" => json!(fx::commands::fx_catalog()),
            "audio" | "audio_effects" => {
                json!(chukcut_engine::modules::audiofx::commands::audiofx_catalog())
            }
            "transitions" => json!(transitions::commands::transitions_catalog()),
            "animations" => json!(motion::commands::motion_catalog()),
            "grade" => json!(summary::grade_controls()
                .into_iter()
                .map(|(name, control)| json!({"name": name, "rest": control.rest()}))
                .collect::<Vec<_>>()),
            "presets" => json!(export::commands::export_presets().presets),
            "hardware" => json!(export::commands::export_presets().hardware),
            "models" => json!({
                "local_available": speech::commands::speech_local_available(),
                "models": speech::commands::speech_models(),
            }),
            "luts" => json!(inspector::commands::inspector_lut_library()),
            "fonts" => json!(pollster::block_on(text::commands::text_fonts())?),
            "masks" => json!({
                "shapes": chukcut_engine::modules::project::compositing::MaskShape::NAMES,
                "ops": chukcut_engine::modules::project::compositing::MaskOp::NAMES,
                "params": chukcut_engine::modules::project::compositing::MASK_PARAMS,
            }),
            "blend" => json!(chukcut_engine::modules::project::compositing::BlendMode::NAMES),
            "title_styles" | "styles" => super::text::styles_catalog(),
            "title_templates" | "templates" => super::text::templates_catalog(),
            "layouts" => super::layout::layouts_catalog(),
            "speed_presets" => json!(chukcut_engine::modules::speed::commands::speed_presets()),
            "accounts" => super::cloud::accounts_catalog(),
            other => {
                return Err(CliError::usage(format!(
                    "there is no catalog called {other:?}; choose effects, audio, transitions, animations, grade, presets, hardware, models, luts, fonts, title_styles, title_templates, layouts, speed_presets, accounts, masks or blend"
                )))
            }
        };
        let count = data.as_array().map(Vec::len);
        Ok(Outcome::read(
            match count {
                Some(n) => format!("{n} {kind}"),
                None => kind.to_string(),
            },
            data,
        ))
    }
}
