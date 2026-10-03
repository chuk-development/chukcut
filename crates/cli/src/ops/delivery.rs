//! Delivery: export presets, the size estimate and the export queue.
//!
//! `export` (in `render.rs`) writes one file and waits for it. These
//! operations are around it: list the presets as they fit this project,
//! save the current settings as a preset of your own, measure how big an
//! export will be, and queue several exports — other presets, other ranges,
//! other project files — that the engine runs one after another.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use chukcut_engine::modules::export::commands as export_commands;
use chukcut_engine::modules::export::{QueueEvent, QueueStatus};
use chukcut_engine::shell::Channel;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::render::ExportSettingsArgs;
use super::{Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::session::{absolute, Session};

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1e6
}

/// List the export presets as they fit this project: the size each one
/// would export at on this canvas, its loudness target, its warnings (a
/// canvas the platform shows with bars, a video longer than it takes) and
/// an instant size estimate. Built-in presets first, then your own.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct PresetsArgs {}

impl Operation for PresetsArgs {
    const NAME: &'static str = "presets";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let project = session.project();
        let mut rows = Vec::new();
        for preset in export_commands::export_preset_list() {
            let request = chukcut_engine::modules::export::ExportRequest {
                output_path: format!("/estimate.{}", preset.extension()),
                preset_id: Some(preset.id.clone()),
                overrides: None,
                hardware: None,
                include_audio: true,
                range: None,
            };
            let plan = export_commands::plan_for(&project, &request);
            let mut row = json!({
                "id": preset.id,
                "label": preset.label,
                "description": preset.description,
                "category": preset.category,
                "user": preset.user,
            });
            match plan {
                Ok(plan) => {
                    row["width"] = json!(plan.width);
                    row["height"] = json!(plan.height);
                    row["fps"] = json!(plan.fps.as_f64());
                    row["container"] = json!(plan.container);
                    row["loudness_target"] = json!(plan.loudness_target);
                    row["audio_only"] = json!(plan.audio_only);
                    row["warnings"] = json!(plan.warnings);
                    row["estimated_bytes"] = json!(plan.estimate.bytes);
                    row["estimate_method"] = json!(plan.estimate.method);
                }
                Err(error) => row["error"] = json!(error),
            }
            rows.push(row);
        }
        Ok(Outcome::read(
            format!("{} presets", rows.len()),
            json!({ "presets": rows }),
        ))
    }
}

/// Save export settings as a preset of your own. The settings are the
/// export options (a preset to start from, then any overrides); the preset
/// keeps their resolution class, so "1080p" stays 1080p on a canvas of
/// another shape. Saving the same name again replaces it.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct PresetSaveArgs {
    /// The preset's name. Its id is `user_` plus the name in lower case.
    pub name: String,
    #[command(flatten)]
    #[serde(flatten)]
    pub settings: ExportSettingsArgs,
}

impl Operation for PresetSaveArgs {
    const NAME: &'static str = "preset_save";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let request = self
            .settings
            .request(session, &PathBuf::from("/preset.mp4"))?;
        let preset = export_commands::export_preset_save(&session.state, &request, &self.name)
            .map_err(CliError::refused)?;
        Ok(Outcome::read(
            format!("saved the preset {} ({})", preset.id, preset.description),
            json!({ "preset": preset }),
        ))
    }
}

/// Delete one of your own presets.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct PresetRemoveArgs {
    /// The preset id, for example user_my_reel.
    pub id: String,
}

impl Operation for PresetRemoveArgs {
    const NAME: &'static str = "preset_remove";
    fn run(self, _: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if !export_commands::export_preset_remove(&self.id).map_err(CliError::refused)? {
            return Err(CliError::refused(format!(
                "there is no preset of your own called {}",
                self.id
            )));
        }
        Ok(Outcome::read(
            format!("deleted the preset {}", self.id),
            json!({ "id": self.id }),
        ))
    }
}

/// Say what an export would produce and how big it would be, without
/// writing it. The size of a CRF export depends on the picture, so with
/// `sample` a few seconds of the timeline are encoded with the real
/// settings and the size is measured (within a few percent); without it the
/// answer is arithmetic and, for CRF, a rough guess.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct EstimateArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub settings: ExportSettingsArgs,
    /// Measure by encoding samples (seconds of work; needs a GPU).
    #[arg(long)]
    #[serde(default)]
    pub sample: bool,
}

impl Operation for EstimateArgs {
    const NAME: &'static str = "estimate";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let output = PathBuf::from("/estimate.mp4");
        let request = self.settings.request(session, &output)?;
        let plan =
            export_commands::export_plan(&session.state, &request).map_err(CliError::refused)?;
        let mut estimate = plan.estimate;
        if self.sample && estimate.is_rough() {
            ctx.progress("Encoding samples", None);
            estimate = export_commands::export_estimate_sampled(
                &session.state,
                &request,
                Arc::new(AtomicBool::new(false)),
            )
            .map_err(CliError::render)?;
        }
        let mut data = serde_json::to_value(&plan).unwrap_or(Value::Null);
        data["estimate"] = json!(estimate);
        data.as_object_mut().map(|m| m.remove("output_path"));
        Ok(Outcome::read(
            format!(
                "{}x{} {} · {:.1} MB ({:?}){}",
                plan.width,
                plan.height,
                plan.container.extension(),
                mb(estimate.bytes),
                estimate.method,
                if plan.warnings.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", plan.warnings.join("; "))
                }
            ),
            data,
        ))
    }
}

/// One export in a queue: the export options, the file to write, and
/// optionally another project file to export instead of this one.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct QueueJob {
    /// The file to write. Its extension is corrected to the container.
    pub output: PathBuf,
    /// Another project file to export. Default: the project of this call.
    #[serde(default)]
    pub project: Option<PathBuf>,
    /// A name for the progress lines. Default: preset and project name.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(flatten)]
    pub settings: ExportSettingsArgs,
}

/// Queue several exports and run them one after another: the same project
/// in several presets (`presets` + `out_dir`), and/or a list of `jobs` with
/// their own presets, ranges, outputs or project files. Waits until all are
/// written; progress goes to stderr per item. Fails (exit 5) when any item
/// failed, after the others are done.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct ExportQueueArgs {
    /// Export this project once per preset: a comma-separated list, or the
    /// flag repeated. (`preset` alone queues one export.)
    #[arg(long, value_delimiter = ',')]
    #[serde(default)]
    pub presets: Vec<String>,
    /// Where the per-preset files go, named `<project>-<preset>.<ext>`.
    /// Default: the project's folder.
    #[arg(long)]
    #[serde(default)]
    pub out_dir: Option<PathBuf>,
    /// A JSON file with a list of jobs (see `jobs`), or - for stdin.
    #[arg(long = "jobs")]
    #[serde(skip)]
    pub jobs_file: Option<PathBuf>,
    /// Exports to queue: each an object with `output` and the export options
    /// (`preset`, `from`, `to`, `crf`, …), and optionally `project` and
    /// `label`.
    #[arg(skip)]
    #[serde(default)]
    pub jobs: Vec<QueueJob>,
    /// The options every per-preset export shares, e.g. a range.
    #[command(flatten)]
    #[serde(flatten)]
    pub common: ExportSettingsArgs,
}

impl Operation for ExportQueueArgs {
    const NAME: &'static str = "export_queue";
    fn run(mut self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        if let Some(file) = &self.jobs_file {
            let raw = crate::read_input(file)?;
            let jobs: Vec<QueueJob> = serde_json::from_str(&raw)
                .map_err(|e| CliError::usage(format!("the jobs file is not a job list: {e}")))?;
            self.jobs.extend(jobs);
        }
        let stem = session
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("export")
            .to_string();
        let out_dir = self
            .out_dir
            .clone()
            .map(|d| absolute(&d))
            .or_else(|| session.path.parent().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        if self.presets.is_empty() && self.jobs.is_empty() && self.jobs_file.is_none() {
            self.presets.extend(self.common.preset.clone());
        }
        let mut jobs: Vec<QueueJob> = self
            .presets
            .iter()
            .map(|preset| QueueJob {
                output: out_dir.join(format!("{stem}-{preset}.mp4")),
                project: None,
                label: None,
                settings: ExportSettingsArgs {
                    preset: Some(preset.clone()),
                    ..self.common.clone()
                },
            })
            .collect();
        jobs.append(&mut self.jobs);
        if jobs.is_empty() {
            return Err(CliError::usage(
                "nothing to queue: give --presets a,b,c or --jobs FILE",
            ));
        }

        // Every request is built and checked before anything starts: a typo
        // in the third job must not surface after the first two exports.
        let mut ids = Vec::new();
        for (index, job) in jobs.iter().enumerate() {
            let output = absolute(&job.output);
            if let Some(dir) = output.parent() {
                std::fs::create_dir_all(dir).map_err(|e| {
                    CliError::render(format!("cannot create {}: {e}", dir.display()))
                })?;
            }
            let (project, request) = match &job.project {
                None => (session.project(), job.settings.request(session, &output)?),
                Some(path) => {
                    let other = Session::open(path)?;
                    (other.project(), job.settings.request(&other, &output)?)
                }
            };
            export_commands::plan_for(&project, &request)
                .map_err(|e| CliError::refused(format!("job {index}: {e}")))?;
            ids.push((index, project, request, job.label.clone()));
        }
        // Checked; now queued.
        let mut queued = Vec::new();
        for (index, project, request, label) in ids {
            let id = export_commands::export_queue_add_project(project, request, label)
                .map_err(|e| CliError::refused(format!("job {index}: {e}")))?;
            queued.push(id);
        }

        let (tx, rx) = mpsc::channel::<QueueEvent>();
        let subscription = export_commands::export_queue_subscribe(Channel::new(move |event| {
            tx.send(event).is_ok()
        }));
        let total = queued.len();
        let mut finished = 0usize;
        while finished < total {
            let Ok(event) = rx.recv() else { break };
            match event {
                QueueEvent::Finished { item } if queued.contains(&item.id) => {
                    finished += 1;
                    ctx.progress(
                        &format!(
                            "{finished} of {total} finished: {} ({:?})",
                            item.label, item.status
                        ),
                        Some(finished as f32 / total as f32),
                    );
                }
                QueueEvent::Changed => {
                    let items = export_commands::export_queue_list();
                    if let Some((n, item)) = items
                        .iter()
                        .filter(|i| queued.contains(&i.id))
                        .enumerate()
                        .find(|(_, i)| i.status == QueueStatus::Running)
                    {
                        let fraction = item.progress.as_ref().map_or(0.0, |p| p.fraction);
                        ctx.progress(
                            &format!("{} of {total}: {}", n + 1, item.label),
                            Some((n as f32 + fraction) / total as f32),
                        );
                    }
                }
                _ => {}
            }
        }
        export_commands::export_queue_unsubscribe(subscription);

        let items: Vec<_> = export_commands::export_queue_list()
            .into_iter()
            .filter(|i| queued.contains(&i.id))
            .collect();
        let failed: Vec<String> = items
            .iter()
            .filter(|i| i.status != QueueStatus::Done)
            .map(|i| {
                format!(
                    "{}: {}",
                    i.label,
                    i.error.clone().unwrap_or_else(|| format!("{:?}", i.status))
                )
            })
            .collect();
        if !failed.is_empty() {
            return Err(CliError::render(format!(
                "{} of {total} exports failed: {}",
                failed.len(),
                failed.join("; ")
            )));
        }
        let results: Vec<Value> = items
            .iter()
            .map(|i| {
                json!({
                    "label": i.label,
                    "path": i.output_path,
                    "bytes": i.bytes,
                    "seconds": i.elapsed_seconds,
                })
            })
            .collect();
        let bytes: u64 = items.iter().filter_map(|i| i.bytes).sum();
        Ok(Outcome::read(
            format!("exported {total} file(s), {:.1} MB in all", mb(bytes)),
            json!({ "exports": results }),
        ))
    }
}
