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

/// `status` (with `--probe`: start the worker and say what runs models;
/// both print a table of each model's provider and precision), `models`,
/// `runtimes`, `bundles`, `install ITEM`, `remove ITEM`, `acceleration
/// fast|standard` (Settings › AI acceleration › Fast) or `bench [MODEL]`
/// (no model: the heavy models in both modes, with the quality of Fast
/// against fp32). An ITEM is a model id (`yunet`, `vittrack`, `rvm`,
/// `birefnet-lite`, `mobilesam`), a runtime pack (`runtime:cpu`,
/// `runtime:cuda13`, `runtime:cudnn9-cu12`, …), a GPU bundle: `gpu` (the
/// one for this machine's NVIDIA driver), `gpu:nvidia-cu12`,
/// `gpu:nvidia-cu13`, or the TensorRT add-on: `tensorrt` (the one for the
/// installed bundle), `tensorrt:tensorrt-cu13`, `tensorrt:tensorrt-cu12`.
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
    /// For `bench`: timed runs (default 100; 20 for the table).
    #[arg(long)]
    #[serde(default)]
    pub iterations: Option<u32>,
    /// For `bench MODEL`: the mode to time, `standard` or `fast` (default:
    /// the configured one).
    #[arg(long)]
    #[serde(default)]
    pub accel: Option<String>,
    /// For `bench MODEL`: also report how far the output is from the
    /// standard (fp32) session's (PSNR, IoU for mattes).
    #[arg(long)]
    #[serde(default)]
    pub compare: bool,
}

/// The models `ml bench` with no model times, at the sizes the features
/// use them: a 720p slow-motion pair, a 640×360 frame to enhance, LaMa's
/// square, a 960×540 frame for objects, a portrait frame for people and a
/// segment of sound.
const BENCH_TABLE: &[(&str, u32, u32)] = &[
    ("rife", 1280, 720),
    ("realesr-general-x4v3", 640, 360),
    ("lama", 512, 512),
    ("birefnet-lite", 960, 540),
    ("rvm", 540, 960),
    ("htdemucs-vocals", 1, 1),
];

/// The per-model table `ml status` prints.
fn status_table(models: &[ml::ModelAcceleration]) -> String {
    let mut out = format!(
        "{:<22} {:<9} {:<9} engine\n",
        "model", "runs on", "precision"
    );
    for m in models {
        let engine = m
            .engines
            .iter()
            .map(|b| format!("{} built in {:.0} s", b.shapes, b.millis / 1000.0))
            .collect::<Vec<_>>()
            .join("; ");
        let engine = if engine.is_empty() && m.provider == "TensorRT" {
            "built on first use".to_string()
        } else {
            engine
        };
        out.push_str(&format!(
            "{:<22} {:<9} {:<9} {}\n",
            m.id, m.provider, m.precision, engine
        ));
    }
    out
}

fn quality_text(q: Option<&ml::Quality>) -> String {
    match q {
        Some(q) => match q.iou {
            Some(iou) => format!("{:.1} dB, IoU {:.4}", q.psnr_db, iou),
            None => format!("{:.1} dB, max {:.0}", q.psnr_db, q.max_diff),
        },
        None => "—".into(),
    }
}

fn item(raw: Option<&str>, allow_noncommercial: bool) -> CliResult<MlItem> {
    let raw = raw.ok_or_else(|| {
        CliError::usage("name a model (yunet), a runtime (runtime:cpu) or a GPU bundle (gpu)")
    })?;
    if raw == "gpu" {
        return Ok(MlItem::Gpu { id: None });
    }
    if raw == "tensorrt" {
        return Ok(MlItem::Tensorrt { id: None });
    }
    if let Some(id) = raw.strip_prefix("tensorrt:") {
        return Ok(MlItem::Tensorrt {
            id: Some(id.into()),
        });
    }
    if let Some(id) = raw.strip_prefix("gpu:") {
        return Ok(MlItem::Gpu {
            id: Some(id.into()),
        });
    }
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
                let mut message = match &status.probe {
                    Some(p) => format!(
                        "Models run on: {} (ONNX Runtime {}; providers {})",
                        status.active,
                        p.runtime_version,
                        p.providers.join(", ")
                    ),
                    None => format!("Models run on: {}", status.active),
                };
                if let Some(advice) = &status.advice {
                    message = format!("{message}. {advice}");
                }
                message = format!(
                    "{message}\nAcceleration: {}{}\n{}",
                    status.acceleration.as_str(),
                    match &status.tensorrt {
                        Some(t) if t.installed => format!(" (TensorRT {} installed)", t.version),
                        Some(t) => format!(
                            " (TensorRT not installed: `ml install tensorrt`, {} MB)",
                            t.missing_bytes >> 20
                        ),
                        None => String::new(),
                    },
                    status_table(&status.models)
                );
                let mut value = json!(status);
                value["mattes"] = json!(
                    chukcut_engine::modules::matting::commands::matting_cache_info()
                );
                Ok(Outcome::read(message, value))
            }
            "models" => Ok(Outcome::read("models", json!(ml::ml_models()))),
            "runtimes" => Ok(Outcome::read("runtime packs", json!(ml::ml_runtimes()))),
            "bundles" => Ok(Outcome::read("GPU bundles", json!(ml::ml_bundles()))),
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
            "acceleration" | "accel" => {
                let fast = match self.item.as_deref() {
                    Some("fast") => true,
                    Some("standard") => false,
                    _ => return Err(CliError::usage("ml acceleration fast|standard")),
                };
                let message = ml::ml_set_acceleration(fast).map_err(CliError::refused)?;
                Ok(Outcome::read(message, json!({ "fast": fast })))
            }
            "bench" if self.item.is_none() => {
                let iterations = self.iterations.unwrap_or(20);
                let fast_ready = {
                    let status = ml::ml_status(false);
                    status.tensorrt.as_ref().is_some_and(|t| t.installed)
                };
                let mut rows = Vec::new();
                let mut table = format!(
                    "{:<22} {:<10} {:>14} {:>16} {:>9} {:>22}\n",
                    "model", "size", "standard ms", "fast ms", "speed-up", "fast vs fp32"
                );
                for &(model, w, h) in BENCH_TABLE {
                    chukcut_engine::modules::ml::prepare(model, model, &|_, _| {}, &cancel)
                        .map_err(|e| CliError::refused(e.to_string()))?;
                    let standard = ml::ml_benchmark_with(
                        model,
                        w,
                        h,
                        iterations,
                        Some(ml::Acceleration::Standard),
                        false,
                        &cancel,
                    )
                    .map_err(CliError::refused)?;
                    let fast = if fast_ready {
                        Some(
                            ml::ml_benchmark_with(
                                model,
                                w,
                                h,
                                iterations,
                                Some(ml::Acceleration::Fast),
                                true,
                                &cancel,
                            )
                            .map_err(CliError::refused)?,
                        )
                    } else {
                        None
                    };
                    let size = if model == "htdemucs-vocals" {
                        "7.8 s".to_string()
                    } else {
                        format!("{w}x{h}")
                    };
                    let (fast_ms, speedup, quality) = match &fast {
                        Some(f) => (
                            format!("{:.1} {} {}", f.mean_millis, f.provider, f.precision),
                            format!("{:.2}x", standard.mean_millis / f.mean_millis.max(0.001)),
                            quality_text(f.quality.as_ref()),
                        ),
                        None => ("not installed".into(), "—".into(), "—".into()),
                    };
                    table.push_str(&format!(
                        "{:<22} {:<10} {:>14} {:>16} {:>9} {:>22}\n",
                        model,
                        size,
                        format!("{:.1} {}", standard.mean_millis, standard.provider),
                        fast_ms,
                        speedup,
                        quality
                    ));
                    rows.push(json!({ "standard": standard, "fast": fast }));
                }
                Ok(Outcome::read(table, json!(rows)))
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
                let accel = match self.accel.as_deref() {
                    None => None,
                    Some(a) => Some(
                        ml::Acceleration::parse(a)
                            .ok_or_else(|| CliError::usage("--accel is standard or fast"))?,
                    ),
                };
                let result = ml::ml_benchmark_with(
                    model,
                    w,
                    h,
                    self.iterations.unwrap_or(100),
                    accel,
                    self.compare,
                    &cancel,
                )
                .map_err(CliError::refused)?;
                let mut message = format!(
                    "{} on {} ({}) at {}x{}: {:.2} ms per frame (best {:.2}; first run {:.0} ms)",
                    result.model,
                    result.provider,
                    result.precision,
                    w,
                    h,
                    result.mean_millis,
                    result.min_millis,
                    result.first_millis
                );
                if self.compare {
                    message.push_str(&format!(
                        "; against fp32: {}",
                        quality_text(result.quality.as_ref())
                    ));
                }
                Ok(Outcome::read(message, json!(result)))
            }
            other => Err(CliError::usage(format!(
                "there is no ml action {other:?}; choose status, models, runtimes, bundles, install, remove, acceleration or bench"
            ))),
        }
    }
}
