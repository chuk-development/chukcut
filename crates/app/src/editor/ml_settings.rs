//! Settings › AI acceleration: what runs the models, the GPU bundles to
//! install or remove with their sizes, "Fast (fp16/TensorRT)" with the
//! TensorRT add-on and each model's provider and precision, the downloaded
//! models, the baked mattes and the remade frames (Remove object, Enhance
//! quality).
//!
//! Everything here calls `ml::commands`, `matting::commands` and
//! `enhance::commands`. Probing
//! starts the ML worker and loads ONNX Runtime (a second or two, longer with
//! CUDA), and an install downloads up to 2 GB, so both run off the UI thread;
//! the section polls their progress a few times a second while one runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chukcut_engine::modules::enhance::commands::{self as enhance, EnhanceCacheInfo};
use chukcut_engine::modules::matting::commands::{self as matting, MatteCacheInfo};
use chukcut_engine::modules::ml::commands::{
    self as ml, BundleInfo, MlItem, MlProgress, MlStatus, ModelInfo, RuntimeInfo,
};
use gpui::component::button::Button;
use gpui::component::switch::Switch;
use gpui::component::{Disableable as _, Sizable as _};
use gpui::AnyElement;

use super::settings::{bytes_label, dim, row, section};
use super::*;

/// The line a job's progress shows while the ML worker builds a TensorRT
/// engine (the first job of a model at a size in "Fast" mode), with how
/// long it has taken so far; `None` when no build runs.
pub(crate) fn tensorrt_notice() -> Option<String> {
    let (stage, so_far) = chukcut_engine::modules::ml::worker::building()?;
    let secs = so_far.as_secs();
    Some(format!("{stage}: {}:{:02} so far", secs / 60, secs % 60))
}

/// "1 min 12 s" for a build's duration.
fn build_time(millis: f32) -> String {
    let secs = (millis / 1000.0).round() as u64;
    if secs >= 60 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{secs} s")
    }
}

/// What the machine has, read off the UI thread.
#[derive(Default)]
struct Snapshot {
    status: Option<MlStatus>,
    bundles: Vec<BundleInfo>,
    runtimes: Vec<RuntimeInfo>,
    models: Vec<ModelInfo>,
    mattes: Option<MatteCacheInfo>,
    remade: Option<EnhanceCacheInfo>,
}

/// An install in progress.
struct Install {
    what: String,
    progress: Arc<Mutex<MlProgress>>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<String, String>>>>,
}

pub(crate) struct AiSettings {
    snapshot: Snapshot,
    /// The probe is running.
    probing: bool,
    install: Option<Install>,
    notice: Option<SharedString>,
}

impl AiSettings {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            snapshot: Snapshot::default(),
            probing: false,
            install: None,
            notice: None,
        };
        this.reload(true, cx);
        this
    }

    /// Read what is installed; with `probe`, also start the worker and ask
    /// it what runs.
    fn reload(&mut self, probe: bool, cx: &mut Context<Self>) {
        self.probing = probe;
        cx.spawn(async move |this, cx| {
            let snapshot = cx
                .background_executor()
                .spawn(async move {
                    Snapshot {
                        status: Some(ml::ml_status(probe)),
                        bundles: ml::ml_bundles(),
                        runtimes: ml::ml_runtimes(),
                        models: ml::ml_models(),
                        mattes: Some(matting::matting_cache_info()),
                        remade: Some(enhance::enhance_cache_info()),
                    }
                })
                .await;
            let _ = this.update(cx, |settings, cx| {
                // A probe that is still running keeps its "checking" line;
                // a quick re-read after a removal must not hide it.
                if probe || settings.snapshot.status.is_none() {
                    settings.probing = false;
                }
                settings.snapshot = snapshot;
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, item: MlItem, what: String, cx: &mut Context<Self>) {
        if self.install.is_some() {
            return;
        }
        let progress = Arc::new(Mutex::new(MlProgress { done: 0, total: 0 }));
        let cancel = Arc::new(AtomicBool::new(false));
        let result = Arc::new(Mutex::new(None));
        {
            let (progress, cancel, result) = (
                Arc::clone(&progress),
                Arc::clone(&cancel),
                Arc::clone(&result),
            );
            std::thread::Builder::new()
                .name("chukcut-ml-install".into())
                .spawn(move || {
                    let outcome = ml::ml_install(
                        item,
                        &|p| {
                            if let Ok(mut slot) = progress.lock() {
                                *slot = p;
                            }
                        },
                        &cancel,
                    );
                    if let Ok(mut slot) = result.lock() {
                        *slot = Some(outcome);
                    }
                })
                .ok();
        }
        self.install = Some(Install {
            what,
            progress,
            cancel,
            result,
        });
        self.notice = None;
        // Poll until the thread reports back.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let done = this
                .update(cx, |settings, cx| {
                    let finished = settings
                        .install
                        .as_ref()
                        .and_then(|i| i.result.lock().ok().and_then(|mut r| r.take()));
                    cx.notify();
                    let Some(finished) = finished else {
                        return settings.install.is_none();
                    };
                    settings.install = None;
                    settings.notice = Some(match finished {
                        Ok(message) => message.into(),
                        Err(error) => error.into(),
                    });
                    settings.reload(true, cx);
                    true
                })
                .unwrap_or(true);
            if done {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    fn remove(&mut self, item: MlItem, cx: &mut Context<Self>) {
        self.notice = Some(match ml::ml_remove(item) {
            Ok(message) => message.into(),
            Err(error) => error.into(),
        });
        self.reload(true, cx);
        cx.notify();
    }

    /// Turn "Fast (fp16/TensorRT)" on or off, off the UI thread (it stops
    /// a running worker), then probe again to show what runs where.
    fn set_fast(&mut self, fast: bool, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { ml::ml_set_acceleration(fast) })
                .await;
            let _ = this.update(cx, |settings, cx| {
                settings.notice = Some(match outcome {
                    Ok(message) => message.into(),
                    Err(error) => error.into(),
                });
                settings.reload(true, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The Fast switch, the TensorRT add-on and what each heavy model runs
    /// on.
    fn render_acceleration(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let Some(status) = self.snapshot.status.as_ref() else {
            return rows;
        };
        let fast = status.acceleration == ml::Acceleration::Fast;
        let switch = Switch::new("ml-fast")
            .checked(fast)
            .on_click(cx.listener(|this, checked: &bool, _, cx| this.set_fast(*checked, cx)));
        rows.push(row(
            "Fast (fp16/TensorRT)",
            Some(
                "Runs slow motion, Enhance quality and Remove object on TensorRT, 1.5–2.6x \
                 faster on NVIDIA GPUs. The first job of a model at a new size prepares it \
                 once (30 s to a few minutes). Needs TensorRT below.",
            ),
            switch,
        ));
        if let Some(trt) = &status.tensorrt {
            let busy = self.install.is_some();
            let hint = format!(
                "NVIDIA TensorRT {} and ONNX Runtime's TensorRT provider, for CUDA {}.",
                trt.version, trt.cuda
            );
            let id = trt.id.to_string();
            let name = trt.name.to_string();
            let control: AnyElement = if trt.installed {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(dim(bytes_label(trt.installed_bytes)))
                    .child(
                        Button::new("ml-remove-tensorrt")
                            .label("Remove")
                            .small()
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove(
                                    MlItem::Tensorrt {
                                        id: Some(id.clone()),
                                    },
                                    cx,
                                )
                            })),
                    )
                    .into_any_element()
            } else {
                Button::new("ml-install-tensorrt")
                    .label(format!("Install ({})", bytes_label(trt.missing_bytes)))
                    .small()
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.install(
                            MlItem::Tensorrt {
                                id: Some(id.clone()),
                            },
                            name.clone(),
                            cx,
                        )
                    }))
                    .into_any_element()
            };
            rows.push(row(trt.name, Some(hint.as_str()), control));
        }
        // What each model with a choice runs on now.
        for m in status.models.iter().filter(|m| m.has_fast_plan) {
            let mut text = format!("{} · {}", m.provider, m.precision);
            if let Some(build) = m.engines.last().filter(|_| m.provider == "TensorRT") {
                text.push_str(&format!(
                    // One engine serves a range of sizes (`accel::profile_for`).
                    " · prepared in {} ({} engine{})",
                    build_time(build.millis),
                    m.engines.len(),
                    if m.engines.len() == 1 { "" } else { "s" }
                ));
            }
            rows.push(row(m.name, Some("Runs on"), dim(text)));
        }
        rows
    }

    fn clear_mattes(&mut self, cx: &mut Context<Self>) {
        self.notice = Some(match matting::matting_cache_clear() {
            Ok(()) => "Baked mattes cleared; clips with Remove background on bake again".into(),
            Err(error) => error.into(),
        });
        self.reload(false, cx);
        cx.notify();
    }

    fn clear_remade(&mut self, cx: &mut Context<Self>) {
        self.notice = Some(match enhance::enhance_cache_clear() {
            Ok(()) => "Remade frames cleared; clips with Remove object or Enhance quality on \
                       bake again"
                .into(),
            Err(error) => error.into(),
        });
        self.reload(false, cx);
        cx.notify();
    }

    fn render_status(&self) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let status = self.snapshot.status.as_ref();
        let active = match status {
            _ if self.probing => "Checking\u{2026}".to_string(),
            Some(status) => status.active.clone(),
            None => "Checking\u{2026}".to_string(),
        };
        // The answer can be a sentence with a library path in it. As the
        // row's control it took the whole width and squeezed the label to one
        // letter per line, so it goes under the label here, where it wraps.
        let advice = status.and_then(|s| s.advice.clone());
        rows.push(
            div()
                .min_h(px(44.0))
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .border_b_1()
                .border_color(rgb(PANEL))
                .child(div().text_sm().child("Models run on"))
                .child(dim(active))
                .children(
                    advice.map(|advice| div().text_xs().text_color(rgb(TEXT_DIM)).child(advice)),
                )
                .into_any_element(),
        );
        if let Some(status) = status {
            let gpus = if status.gpus.is_empty() {
                "None found".to_string()
            } else {
                status.gpus.join(", ")
            };
            let driver = status
                .driver
                .as_ref()
                .map(|d| format!("NVIDIA driver {d}"))
                .unwrap_or_else(|| "No NVIDIA driver".into());
            rows.push(row("Graphics", Some(driver.as_str()), dim(gpus)));
        }
        rows
    }

    fn render_bundle(&self, bundle: &BundleInfo, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.install.is_some();
        let hint = format!(
            "ONNX Runtime with CUDA {}, the CUDA runtime, cuBLAS, cuRAND, NVRTC and cuDNN 9 from \
             NVIDIA. Needs driver {} or newer{}.",
            bundle.cuda,
            bundle.min_driver,
            if bundle.recommended {
                "; the one for this machine"
            } else if bundle.runnable {
                ""
            } else {
                "; this machine's driver is too old for it"
            }
        );
        let id = bundle.id.to_string();
        let name = bundle.name.to_string();
        let control: AnyElement = if !bundle.installed && bundle.installed_bytes > 0 {
            // Partly there (an older install, a cancelled download): finish
            // it or take back what is there.
            let remove_id = id.clone();
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("ml-finish-{id}")))
                        .label(format!("Finish ({})", bytes_label(bundle.missing_bytes)))
                        .small()
                        .disabled(busy || !bundle.runnable)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.install(
                                MlItem::Gpu {
                                    id: Some(id.clone()),
                                },
                                name.clone(),
                                cx,
                            )
                        })),
                )
                .child(
                    Button::new(SharedString::from(format!("ml-remove-{remove_id}")))
                        .label(format!("Remove ({})", bytes_label(bundle.installed_bytes)))
                        .small()
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove(
                                MlItem::Gpu {
                                    id: Some(remove_id.clone()),
                                },
                                cx,
                            )
                        })),
                )
                .into_any_element()
        } else if bundle.installed {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(dim(bytes_label(bundle.installed_bytes)))
                .child(
                    Button::new(SharedString::from(format!("ml-remove-{id}")))
                        .label("Remove")
                        .small()
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.remove(
                                MlItem::Gpu {
                                    id: Some(id.clone()),
                                },
                                cx,
                            )
                        })),
                )
                .into_any_element()
        } else {
            let label = format!("Install ({})", bytes_label(bundle.missing_bytes));
            Button::new(SharedString::from(format!("ml-install-{id}")))
                .label(label)
                .small()
                .disabled(busy || !bundle.runnable)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.install(
                        MlItem::Gpu {
                            id: Some(id.clone()),
                        },
                        name.clone(),
                        cx,
                    )
                }))
                .into_any_element()
        };
        row(bundle.name, Some(hint.as_str()), control)
    }

    /// Runtime packs installed outside a complete bundle (an older install,
    /// or one pack by hand), each removable on its own.
    fn render_loose_packs(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // A bundle's packs are listed (and removed) with the bundle.
        let in_bundle = |id: &str| {
            self.snapshot.bundles.iter().any(|b| b.packs.contains(&id))
                || self
                    .snapshot
                    .status
                    .as_ref()
                    .and_then(|s| s.tensorrt.as_ref())
                    .is_some_and(|t| t.packs.contains(&id))
        };
        self.snapshot
            .runtimes
            .iter()
            .filter(|r| r.installed && !in_bundle(r.id))
            .map(|r| {
                let id = r.id.to_string();
                let control = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(dim(bytes_label(r.installed_bytes)))
                    .child(
                        Button::new(SharedString::from(format!("ml-remove-pack-{id}")))
                            .label("Remove")
                            .small()
                            .disabled(self.install.is_some())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove(MlItem::Runtime { id: id.clone() }, cx)
                            })),
                    );
                row(r.name, Some("Runtime pack"), control)
            })
            .collect()
    }

    fn render_models(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.snapshot
            .models
            .iter()
            // A decoder comes and goes with its encoder.
            .filter(|m| {
                !self
                    .snapshot
                    .models
                    .iter()
                    .any(|other| other.companion == Some(m.id))
            })
            .map(|m| {
                let mut bytes = m.bytes;
                if let Some(companion) = m
                    .companion
                    .and_then(|c| self.snapshot.models.iter().find(|o| o.id == c))
                {
                    bytes += companion.bytes;
                }
                let hint = format!(
                    "{}{}",
                    m.licence,
                    if m.cpu_ok { "" } else { " · needs a GPU" }
                );
                let control: AnyElement = if m.downloaded {
                    let id = m.id.to_string();
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(dim(bytes_label(bytes)))
                        .child(
                            Button::new(SharedString::from(format!("ml-remove-model-{id}")))
                                .label("Remove")
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.remove(
                                        MlItem::Model {
                                            id: id.clone(),
                                            allow_noncommercial: false,
                                        },
                                        cx,
                                    )
                                })),
                        )
                        .into_any_element()
                } else {
                    dim(format!("Downloads on first use ({})", bytes_label(bytes)))
                        .into_any_element()
                };
                row(m.name, Some(hint.as_str()), control)
            })
            .collect()
    }

    fn render_mattes(&self, cx: &mut Context<Self>) -> AnyElement {
        let size = self
            .snapshot
            .mattes
            .as_ref()
            .map(|m| bytes_label(m.bytes))
            .unwrap_or_else(|| "\u{2026}".into());
        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(dim(size))
            .child(
                Button::new("ml-clear-mattes")
                    .label("Clear")
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.clear_mattes(cx))),
            );
        row(
            "Baked mattes",
            Some("Remove background's cut-outs, one picture per frame. Part of the cache limit; the open project's are kept."),
            control,
        )
    }
}

impl AiSettings {
    fn render_remade(&self, cx: &mut Context<Self>) -> AnyElement {
        let size = self
            .snapshot
            .remade
            .as_ref()
            .map(|m| bytes_label(m.bytes))
            .unwrap_or_else(|| "\u{2026}".into());
        let control = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(dim(size))
            .child(
                Button::new("ml-clear-remade")
                    .label("Clear")
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.clear_remade(cx))),
            );
        row(
            "Remade frames",
            Some("Remove object and Enhance quality, one picture per frame. Part of the cache limit; the open project's are kept."),
            control,
        )
    }
}

impl Render for AiSettings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut rows = self.render_status();
        if let Some(install) = &self.install {
            let progress = install
                .progress
                .lock()
                .map(|p| MlProgress {
                    done: p.done,
                    total: p.total,
                })
                .unwrap_or(MlProgress { done: 0, total: 0 });
            let fraction = progress.done as f32 / progress.total.max(1) as f32;
            let cancel = Arc::clone(&install.cancel);
            rows.push(row(
                &format!("Installing {}", install.what),
                Some(
                    format!(
                        "{} of {}",
                        bytes_label(progress.done),
                        bytes_label(progress.total)
                    )
                    .as_str(),
                ),
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div().w(px(140.0)).child(
                            gpui::component::progress::Progress::new("ml-install-progress")
                                .value(fraction * 100.0),
                        ),
                    )
                    .child(
                        Button::new("ml-install-cancel")
                            .label("Cancel")
                            .small()
                            .on_click(move |_, _, _| cancel.store(true, Ordering::Relaxed)),
                    ),
            ));
        }
        let bundles = self.snapshot.bundles.clone();
        let nvidia = self
            .snapshot
            .status
            .as_ref()
            .is_some_and(|s| s.gpus.iter().any(|g| g == "NVIDIA"));
        for bundle in bundles
            .iter()
            .filter(|b| nvidia || b.installed)
            // The recommended one first.
            .filter(|b| b.recommended)
            .chain(
                bundles
                    .iter()
                    .filter(|b| (nvidia || b.installed) && !b.recommended),
            )
        {
            rows.push(self.render_bundle(bundle, cx));
        }
        rows.extend(self.render_acceleration(cx));
        rows.extend(self.render_loose_packs(cx));
        rows.extend(self.render_models(cx));
        rows.push(self.render_mattes(cx));
        rows.push(self.render_remade(cx));
        if let Some(notice) = &self.notice {
            rows.push(row("", Some(notice.as_ref()), div()));
        }
        section("AI acceleration", rows)
    }
}
