//! `chukcut-cli ml`: the ML worker's models and runtime packs, and a speed
//! measurement. Not tied to a project.

use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::ml::commands::{self as ml, MlItem};
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::Outcome;
use crate::error::{CliError, CliResult};

/// `status` (with `--probe`: start the worker and list the providers that
/// work), `models`, `runtimes`, `install ITEM`, `remove ITEM` or `bench MODEL`.
/// An ITEM is a model id (`yunet`, `vittrack`, `rvm`) or a runtime pack
/// (`runtime:cpu`, `runtime:cuda12`, `runtime:cuda13`).
#[derive(Debug, Clone, Args, Deserialize, JsonSchema)]
pub struct MlArgs {
    pub action: String,
    /// The item or model the action is about.
    #[serde(default)]
    pub item: Option<String>,
    /// For `status`: start the worker and ask it which providers work.
    #[arg(long)]
    #[serde(default)]
    pub probe: bool,
    /// For `install`: allow a model whose licence forbids commercial use.
    #[arg(long)]
    #[serde(default)]
    pub allow_noncommercial: bool,
    /// For `bench`: the frame size, WxH (default 640x360).
    #[arg(long)]
    #[serde(default)]
    pub size: Option<String>,
    /// For `bench`: timed runs (default 100).
    #[arg(long)]
    #[serde(default)]
    pub iterations: Option<u32>,
}

fn item(raw: Option<&str>, allow_noncommercial: bool) -> CliResult<MlItem> {
    let raw =
        raw.ok_or_else(|| CliError::usage("name a model (yunet) or a runtime (runtime:cpu)"))?;
    Ok(match raw.strip_prefix("runtime:") {
        Some(id) => MlItem::Runtime { id: id.into() },
        None => MlItem::Model {
            id: raw.into(),
            allow_noncommercial,
        },
    })
}

impl MlArgs {
    pub fn run(self) -> CliResult<Outcome> {
        let cancel = AtomicBool::new(false);
        match self.action.as_str() {
            "status" => {
                let status = ml::ml_status(self.probe);
                let message = status.problem.clone().unwrap_or_else(|| {
                    match &status.probe {
                        Some(p) => format!("ONNX Runtime {} · {}", p.runtime_version, p.providers.join(", ")),
                        None => format!("ready ({})", status.runtime.clone().unwrap_or_default()),
                    }
                });
                Ok(Outcome::read(message, json!(status)))
            }
            "models" => Ok(Outcome::read("models", json!(ml::ml_models()))),
            "runtimes" => Ok(Outcome::read("runtime packs", json!(ml::ml_runtimes()))),
            "install" => {
                let item = item(self.item.as_deref(), self.allow_noncommercial)?;
                let message = ml::ml_install(
                    item.clone(),
                    &|p| eprint!("\r{} of {} MB", p.done >> 20, p.total >> 20),
                    &cancel,
                )
                .map_err(CliError::refused)?;
                eprintln!();
                Ok(Outcome::read(message, json!(item)))
            }
            "remove" => {
                let item = item(self.item.as_deref(), false)?;
                let message = ml::ml_remove(item.clone()).map_err(CliError::refused)?;
                Ok(Outcome::read(message, json!(item)))
            }
            "bench" => {
                let model = self.item.as_deref().unwrap_or("yunet");
                let (w, h) = match self.size.as_deref() {
                    None => (640, 360),
                    Some(s) => s
                        .split_once('x')
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                        .ok_or_else(|| CliError::usage("--size is WxH, e.g. 1280x720"))?,
                };
                // Make sure the model and a runtime are there, as a feature
                // would on first use.
                chukcut_engine::modules::ml::prepare(model, model, &|_, _| {}, &cancel)
                    .map_err(|e| CliError::refused(e.to_string()))?;
                let result = ml::ml_benchmark(model, w, h, self.iterations.unwrap_or(100), &cancel)
                    .map_err(CliError::refused)?;
                Ok(Outcome::read(
                    format!(
                        "{} on {} at {}x{}: {:.2} ms per frame (best {:.2})",
                        result.model, result.provider, w, h, result.mean_millis, result.min_millis
                    ),
                    json!(result),
                ))
            }
            other => Err(CliError::usage(format!(
                "there is no ml action {other:?}; choose status, models, runtimes, install, remove or bench"
            ))),
        }
    }
}
