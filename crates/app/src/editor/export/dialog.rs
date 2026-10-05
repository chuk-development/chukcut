//! The export dialog: cover on the left, settings on the right, duration and
//! estimated size in the footer — CapCut's arrangement. Once the export
//! starts, the right side turns into its progress, and at the end into
//! "Done" with a way to the file.
//!
//! A preset at the top fills in every setting; the dialog opens with the
//! settings of this project's last export (or of the last export anywhere).
//! "Add to queue" hands the export to the engine's queue instead of running
//! it here, and the queue view lists what is queued, running and done — it
//! keeps running after the dialog closes.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chukcut_engine::modules::export::commands::ExportPlan;
use chukcut_engine::modules::export::hwaccel::{self, HwEncoder};
use chukcut_engine::modules::export::{ExportPreset, ExportRequest, PresetCategory, SizeEstimate};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::progress::Progress;
use gpui::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui::{AnyElement, Entity, Subscription, WeakEntity};

use super::settings::{
    self, AudioFormat, Bitrate, Codec, ExportChoices, Format, OutputKind, Resolution,
};
use super::*;
use crate::ui::{
    icons, Badge, IconButton, IconSrc, PropertyRow, Section, SectionHeader, SegmentedTabs, Tone,
};

type Slot = Arc<parking_lot::Mutex<Option<ExportProgress>>>;

enum Phase {
    Settings,
    /// Waiting for hardware detection before the job can be started.
    Starting,
    Running {
        job_id: String,
        encoder: String,
        progress: Option<ExportProgress>,
    },
    Done {
        path: PathBuf,
        frames: u64,
        seconds: f64,
    },
    Failed(String),
}

/// One option of a dropdown: its label, whether it is the current one, and
/// what choosing it changes.
type Pick = (SharedString, bool, Rc<dyn Fn(&mut ExportChoices)>);

pub(crate) struct ExportDialog {
    editor: WeakEntity<Editor>,
    state: Arc<AppState>,
    project: Arc<Project>,
    choices: ExportChoices,
    name: Entity<InputState>,
    custom_bitrate: Entity<InputState>,
    /// Hardware encoders, once detection has answered.
    encoders: Option<Vec<HwEncoder>>,
    phase: Phase,
    /// One line above the settings, e.g. after a cancelled export.
    notice: Option<SharedString>,
    /// Progress for this dialog, written by the export thread.
    progress: Slot,
    /// The editor's progress slot, so the title bar keeps reporting when the
    /// dialog is closed after the export finishes.
    editor_progress: Slot,
    /// The mix's loudness, measured on request.
    mix_meter: Entity<super::loudness::MixMeter>,
    /// What the licences of online media on the timeline ask (`licences.rs`).
    licences: chukcut_engine::modules::cloud::credits::LicenceSummary,
    /// The credits file written after the export, if one was due.
    credits: Option<PathBuf>,
    /// Built-in presets, then the user's.
    presets: Vec<ExportPreset>,
    /// The name field for "Save as preset", while it is open.
    preset_name: Entity<InputState>,
    saving_preset: bool,
    /// The in/out marks of the timeline, when any is set.
    marks: Option<(Micros, Micros)>,
    /// The measured size: for which request, and the answer. `None` in the
    /// answer while the sample encode runs.
    measured: Option<(ExportRequest, Option<Result<SizeEstimate, String>>)>,
    measure_cancel: Option<Arc<AtomicBool>>,
    _measure: Option<Task<()>>,
    /// The right side shows the queue instead of the settings.
    show_queue: bool,
    queue: Vec<chukcut_engine::modules::export::QueueItem>,
    _subscriptions: Vec<Subscription>,
    _poll: Option<Task<()>>,
    _queue_poll: Task<()>,
}

impl ExportDialog {
    pub(crate) fn new(
        editor: WeakEntity<Editor>,
        state: Arc<AppState>,
        project: Arc<Project>,
        editor_progress: Slot,
        marks: (Option<Micros>, Option<Micros>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let memory = export_commands::export_memory_recall(Some(&project.id));
        let mut choices =
            ExportChoices::opening(&project, settings::default_directory(), memory.as_ref());
        // In/out marks: offered as a range, never chosen by themselves — an
        // export of part of the timeline has to be asked for.
        let marks = (marks.0.is_some() || marks.1.is_some()).then(|| {
            let start = marks.0.unwrap_or(0).max(0);
            let end = marks
                .1
                .unwrap_or(project.duration())
                .min(project.duration());
            (start, end)
        });
        let marks = marks.filter(|(start, end)| end > start);
        choices.range = None;
        let preset_name = cx.new(|cx| InputState::new(window, cx).placeholder("Preset name"));
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("File name")
                .default_value(choices.name.clone())
        });
        let custom_bitrate = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Mbit/s")
                .default_value(format!("{}", choices.custom_mbps))
        });
        let subscriptions = vec![
            cx.subscribe(&name, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.choices.name = input.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            cx.subscribe(&custom_bitrate, |this, input, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Ok(mbps) = input.read(cx).value().trim().parse::<f64>() {
                        if mbps.is_finite() && mbps > 0.0 && this.choices.custom_mbps != mbps {
                            this.choices.custom_mbps = mbps;
                            this.choices.preset = None;
                        }
                    }
                    cx.notify();
                }
            }),
            cx.subscribe(&preset_name, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_preset(cx);
                }
            }),
        ];

        // The queue's list, refreshed while the dialog is open. Cheap: a
        // lock and a clone of a few items.
        let queue_poll = cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let alive = this.update(cx, |dialog, cx| {
                let items = export_commands::export_queue_list();
                if !queue_equal(&items, &dialog.queue) {
                    dialog.queue = items;
                    cx.notify();
                }
            });
            if alive.is_err() {
                break;
            }
        });

        // Detection opens each device and trial-encodes a frame. It is
        // cached for the process and usually done by now (the editor starts
        // it at launch); either way it must not run on the UI thread.
        cx.spawn(async move |this, cx| {
            let encoders = cx
                .background_executor()
                .spawn(async { hwaccel::detect() })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.encoders = Some(encoders);
                cx.notify();
            });
        })
        .detach();

        let mix_meter = cx.new(|_| super::loudness::MixMeter::new(Arc::clone(&state)));
        Self {
            mix_meter,
            licences: chukcut_engine::modules::cloud::credits::summary(&project),
            credits: None,
            editor,
            state,
            project,
            choices,
            name,
            custom_bitrate,
            encoders: None,
            phase: Phase::Settings,
            notice: None,
            progress: Arc::new(parking_lot::Mutex::new(None)),
            editor_progress,
            presets: export_commands::export_preset_list(),
            preset_name,
            saving_preset: false,
            marks,
            measured: None,
            measure_cancel: None,
            _measure: None,
            show_queue: false,
            queue: export_commands::export_queue_list(),
            _subscriptions: subscriptions,
            _poll: None,
            _queue_poll: queue_poll,
        }
    }

    fn request(&self) -> ExportRequest {
        let hardware = self.encoder().flatten().map(|hw| hw.id.clone());
        self.choices.request(&self.project, hardware)
    }

    /// The request resolved against the dialog's snapshot: size, warnings,
    /// the instant size estimate — or why it cannot be exported.
    fn plan(&self) -> Result<ExportPlan, String> {
        export_commands::plan_for(&self.project, &self.request())
    }

    fn remember(&self) {
        let memory = self.choices.memory(&self.project);
        if let Err(error) = export_commands::export_memory_remember(Some(&self.project.id), &memory)
        {
            tracing::warn!(%error, "could not remember the export settings");
        }
        settings::remember_directory(&self.choices.directory);
    }

    /// Start measuring the size of the current request, unless that is
    /// already measured or being measured. Debounced: a sample encode is
    /// seconds of work, and the user may still be clicking.
    fn ensure_measured(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Settings) || self.encoders.is_none() {
            return;
        }
        let request = self.request();
        if self
            .measured
            .as_ref()
            .is_some_and(|(asked, _)| *asked == request)
        {
            return;
        }
        if let Some(cancel) = self.measure_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.measure_cancel = Some(Arc::clone(&cancel));
        self.measured = Some((request.clone(), None));
        let project = (*self.project).clone();
        self._measure = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(700))
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let asked = request.clone();
            let flag = Arc::clone(&cancel);
            let result = cx
                .background_executor()
                .spawn(async move { export_commands::estimate_sampled_for(project, &asked, flag) })
                .await;
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = this.update(cx, |dialog, cx| {
                if let Some((asked, answer)) = dialog.measured.as_mut() {
                    if *asked == request {
                        *answer = Some(result);
                        cx.notify();
                    }
                }
            });
        }));
    }

    fn stop_measuring(&mut self) {
        if let Some(cancel) = self.measure_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        // A cancelled measurement has no answer coming. Left in place, the
        // footer said "measuring…" for good (an export started before the
        // sample encode ended) and `ensure_measured` never asked again.
        if self
            .measured
            .as_ref()
            .is_some_and(|(_, answer)| answer.is_none())
        {
            self.measured = None;
        }
    }

    fn apply_preset(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(preset) = self.presets.iter().find(|p| p.id == id).cloned() {
            self.choices.apply_preset(&self.project, &preset);
            cx.notify();
        }
    }

    fn save_preset(&mut self, cx: &mut Context<Self>) {
        let label = self.preset_name.read(cx).value().trim().to_string();
        if label.is_empty() {
            return;
        }
        let request = self.request();
        let saved = chukcut_engine::modules::export::store::preset_from_request(
            &self.project,
            &request,
            &label,
        )
        .and_then(chukcut_engine::modules::export::store::save_user_preset);
        match saved {
            Ok(preset) => {
                self.presets = export_commands::export_preset_list();
                self.choices.preset = Some(preset.id.clone());
                self.notice = Some(format!("Saved the preset \"{}\"", preset.label).into());
                self.saving_preset = false;
            }
            Err(error) => self.notice = Some(format!("The preset was not saved: {error}").into()),
        }
        cx.notify();
    }

    fn delete_preset(&mut self, id: String, cx: &mut Context<Self>) {
        match export_commands::export_preset_remove(&id) {
            Ok(_) => {
                self.presets = export_commands::export_preset_list();
                if self.choices.preset.as_deref() == Some(id.as_str()) {
                    self.choices.preset = None;
                }
            }
            Err(error) => self.notice = Some(error.into()),
        }
        cx.notify();
    }

    /// Hand the export to the queue: it runs in the background, after
    /// whatever is queued before it, whether or not this dialog stays open.
    fn add_to_queue(&mut self, cx: &mut Context<Self>) {
        let request = self.request();
        match export_commands::export_queue_add_project((*self.project).clone(), request, None) {
            Ok(id) => {
                self.remember();
                let label = export_commands::export_queue_list()
                    .into_iter()
                    .find(|item| item.id == id)
                    .map(|item| item.label)
                    .unwrap_or_default();
                self.notice = Some(format!("Added to the queue: {label}").into());
                self.queue = export_commands::export_queue_list();
            }
            Err(error) => self.notice = Some(error.into()),
        }
        cx.notify();
    }

    /// Whether an export is starting or running, which keeps the dialog open.
    pub(crate) fn busy(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Running { .. })
    }

    /// The encoder the current codec would use: the GPU one when this
    /// machine has a working one, else software.
    fn encoder(&self) -> Option<Option<&HwEncoder>> {
        self.encoders
            .as_ref()
            .map(|encoders| settings::pick_hardware(encoders, self.choices.codec))
    }

    fn encoder_label(&self) -> String {
        match self.choices.kind {
            OutputKind::Audio => return "Audio encoder".into(),
            OutputKind::Gif => return "Software · gif".into(),
            OutputKind::Video if self.choices.codec == Codec::ProRes => {
                return "Software · prores_ks".into()
            }
            OutputKind::Video => {}
        }
        match self.encoder() {
            None => "Detecting…".into(),
            Some(Some(hw)) => format!("GPU · {}", hw.label),
            Some(None) => format!(
                "Software · {}",
                self.choices.codec.video_codec().software_encoder()
            ),
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.phase = Phase::Starting;
        self.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            // Normally instant: the answer is cached.
            let encoders = cx
                .background_executor()
                .spawn(async { hwaccel::detect() })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.encoders = Some(encoders);
                dialog.launch(cx);
            });
        })
        .detach();
    }

    fn launch(&mut self, cx: &mut Context<Self>) {
        self.stop_measuring();
        let encoder = self.encoder_label();
        let request = self.request();
        let (mine, editors) = (
            Arc::clone(&self.progress),
            Arc::clone(&self.editor_progress),
        );
        let channel = Channel::new(move |progress: ExportProgress| {
            *mine.lock() = Some(progress.clone());
            *editors.lock() = Some(progress);
            true
        });
        match export_commands::export_start(&self.state, request, channel) {
            Ok(job_id) => {
                self.remember();
                self.phase = Phase::Running {
                    job_id,
                    encoder,
                    progress: None,
                };
                self._poll = Some(cx.spawn(async move |this, cx| loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    let running = this.update(cx, |dialog, cx| dialog.poll(cx));
                    if !matches!(running, Ok(true)) {
                        break;
                    }
                }));
            }
            Err(error) => self.phase = Phase::Failed(error),
        }
        cx.notify();
    }

    /// Take the newest progress message. `false` once the job has ended.
    fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(progress) = self.progress.lock().take() else {
            return true;
        };
        cx.notify();
        match progress.stage {
            ExportStage::Done => {
                let path = progress
                    .output_path
                    .clone()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.choices.output_path());
                // Captions as a sidecar file, when the Captions tab asks for it.
                crate::editor::captions::after_export(&self.state, &path);
                // Credits for stock and generated media that need them.
                self.credits = super::licences::after_export(&self.state, &path);
                self.phase = Phase::Done {
                    path,
                    frames: progress.total_frames,
                    seconds: progress.elapsed_seconds,
                };
                false
            }
            ExportStage::Failed => {
                self.phase = Phase::Failed(
                    progress
                        .message
                        .unwrap_or_else(|| "the export failed".into()),
                );
                false
            }
            ExportStage::Cancelled => {
                self.phase = Phase::Settings;
                self.notice = Some("Export cancelled. Nothing was written.".into());
                false
            }
            _ => {
                if let Phase::Running { progress: slot, .. } = &mut self.phase {
                    *slot = Some(progress);
                }
                true
            }
        }
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.phase {
            Phase::Running { job_id, .. } => {
                let _ = export_commands::export_cancel(job_id.clone());
            }
            Phase::Starting => {}
            _ => window.close_dialog(cx),
        }
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let request =
            FileRequest::folder("Export here").starting_in(self.choices.directory.clone());
        let picked = files::choose_one(request, cx);
        cx.spawn(async move |this, cx| {
            if let Some(directory) = picked.await {
                let _ = this.update(cx, |dialog, cx| {
                    dialog.choices.directory = directory;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // --- drawing ------------------------------------------------------------------

    /// A field-like button that opens a menu of `picks`.
    fn picker(
        &self,
        id: &'static str,
        picks: Vec<Pick>,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = picks
            .iter()
            .find(|(_, current, _)| *current)
            .map(|(label, _, _)| label.clone())
            .unwrap_or_default();
        let picks = Rc::new(picks);
        let dialog = cx.entity().downgrade();
        Button::new(id)
            .w_full()
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .disabled(disabled)
            .dropdown_menu(move |menu, _, _| {
                picks
                    .iter()
                    .fold(menu.min_w(px(220.0)), |menu, (label, current, apply)| {
                        let (apply, dialog) = (Rc::clone(apply), dialog.clone());
                        menu.item(
                            PopupMenuItem::new(label.clone())
                                .checked(*current)
                                .on_click(move |_, _, cx| {
                                    let _ = dialog.update(cx, |dialog, cx| {
                                        apply(&mut dialog.choices);
                                        // Any change a preset decides makes
                                        // the settings the user's own.
                                        dialog.choices.preset = None;
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
    }

    fn row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
        PropertyRow::new(SharedString::from(format!("export-row-{label}")), label)
            .no_actions()
            .child(div().flex_1().min_w(px(0.0)).child(control))
    }

    /// A segmented choice that writes the pick into the dialog's choices.
    fn segments<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: &[T],
        current: T,
        label: impl Fn(T) -> &'static str,
        apply: impl Fn(&mut ExportChoices, T) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = options.iter().position(|o| *o == current).unwrap_or(0);
        let options = options.to_vec();
        let dialog = cx.entity().downgrade();
        SegmentedTabs::new(id, options.iter().map(|o| label(*o)), selected).on_select(
            move |index, _, cx| {
                let pick = options[index];
                let _ = dialog.update(cx, |dialog, cx| {
                    let preset = dialog.choices.preset.take();
                    apply(&mut dialog.choices, pick);
                    // The range is not something a preset decides.
                    if id == "export-range" {
                        dialog.choices.preset = preset;
                    }
                    cx.notify();
                });
            },
        )
    }

    fn render_cover(&self, cx: &App) -> impl IntoElement {
        let frame = self
            .editor
            .upgrade()
            .and_then(|editor| editor.read(cx).frame.clone());
        let (cw, ch) = (
            self.project.canvas.width.max(1) as f32,
            self.project.canvas.height.max(1) as f32,
        );
        let fit = (300.0 / cw).min(380.0 / ch);
        let (w, h) = (cw * fit, ch * fit);
        div()
            .w(px(332.0))
            .flex_none()
            .p(px(20.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(HAIRLINE))
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(R_MD))
                    .overflow_hidden()
                    .bg(rgb(VIEWER))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .children(frame.map(|frame| frame.element()))
                    .child(
                        div()
                            .absolute()
                            .top(px(8.0))
                            .left(px(8.0))
                            .child(Badge::new("Cover").tone(Tone::OnMedia).icon(Lucide::Image)),
                    ),
            )
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("The frame at the playhead"),
            )
    }

    /// The preset dropdown: grouped by category, the user's own last.
    fn preset_picker(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.choices.preset.clone();
        let label: SharedString = current
            .as_deref()
            .and_then(|id| self.presets.iter().find(|p| p.id == id))
            .map(|p| p.label.clone())
            .unwrap_or_else(|| "Custom settings".into())
            .into();
        let presets = Rc::new(self.presets.clone());
        let dialog = cx.entity().downgrade();
        Button::new("export-preset")
            .w_full()
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let mut menu = menu.min_w(px(300.0));
                let mut last: Option<(PresetCategory, bool)> = None;
                for preset in presets.iter() {
                    let group = (preset.category, preset.user);
                    if last != Some(group) {
                        if last.is_some() {
                            menu = menu.separator();
                        }
                        menu = menu.label(if preset.user {
                            "My presets"
                        } else {
                            preset.category.label()
                        });
                        last = Some(group);
                    }
                    let (id, dialog) = (preset.id.clone(), dialog.clone());
                    menu = menu.item(
                        PopupMenuItem::new(format!("{} — {}", preset.label, preset.description))
                            .checked(current.as_deref() == Some(preset.id.as_str()))
                            .on_click(move |_, _, cx| {
                                let _ =
                                    dialog.update(cx, |dialog, cx| dialog.apply_preset(&id, cx));
                            }),
                    );
                }
                menu
            })
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_measured(cx);
        let plan = self.plan();
        let c = &self.choices;
        let (width, height) = c.size(&self.project);
        let output = c.output_path();
        let exists = output.exists();
        let kind = c.kind;

        let rate_list: &[(f64, &str)] = if kind == OutputKind::Gif {
            &settings::GIF_RATES
        } else {
            &settings::FRAME_RATES
        };
        let rates = rate_list
            .iter()
            .map(|&(rate, label)| -> Pick {
                (
                    label.into(),
                    (rate - c.fps).abs() < 0.001,
                    Rc::new(move |c: &mut ExportChoices| c.fps = rate),
                )
            })
            .collect();
        let audio_rates = settings::AUDIO_BITRATES
            .into_iter()
            .map(|(bits, label)| -> Pick {
                (
                    label.into(),
                    bits == c.audio_bitrate,
                    Rc::new(move |c: &mut ExportChoices| c.audio_bitrate = bits),
                )
            })
            .collect();

        let folder = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(CONTROL_H))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(R_SM))
                    .bg(rgb(WELL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT))
                    .child(settings::display_path(&c.directory)),
            )
            .child(
                IconButton::new("export-folder", Lucide::FolderOpen)
                    .tooltip("Choose a folder")
                    .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
            );

        let encoder = match self.encoder() {
            None => Badge::new("Detecting…"),
            Some(Some(_)) if kind == OutputKind::Video && c.codec != Codec::ProRes => {
                Badge::new(self.encoder_label()).tone(Tone::Accent)
            }
            _ => Badge::new(self.encoder_label()),
        };

        // --- preset ---------------------------------------------------------
        let user_preset = c
            .preset
            .as_deref()
            .filter(|id| self.presets.iter().any(|p| p.id == *id && p.user))
            .map(str::to_string);
        let preset_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(div().flex_1().min_w(px(0.0)).child(self.preset_picker(cx)))
            .child(
                IconButton::new("export-preset-save", Lucide::Plus)
                    .tooltip("Save these settings as a preset")
                    .toggled(self.saving_preset)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.saving_preset = !this.saving_preset;
                        cx.notify();
                    })),
            )
            .children(user_preset.map(|id| {
                IconButton::new("export-preset-delete", Lucide::Trash)
                    .tooltip("Delete this preset")
                    .on_click(cx.listener(move |this, _, _, cx| this.delete_preset(id.clone(), cx)))
            }));
        let save_row = self.saving_preset.then(|| {
            Self::row(
                "Preset name",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div().flex_1().min_w(px(0.0)).child(
                            Input::new(&self.preset_name)
                                .small()
                                .text_size(px(TEXT_LABEL)),
                        ),
                    )
                    .child(
                        Button::new("export-preset-save-confirm")
                            .small()
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_preset(cx))),
                    ),
            )
        });

        let kind_tabs = self.segments(
            "export-kind",
            &OutputKind::ALL,
            kind,
            OutputKind::label,
            |c, k| c.set_kind(k),
            cx,
        );
        let range_row = self.marks.map(|(start, end)| {
            let options = [false, true];
            let ranged = c.range.is_some();
            let label = move |r: bool| -> &'static str {
                if r {
                    "In to out"
                } else {
                    "Whole timeline"
                }
            };
            Self::row(
                "Range",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(self.segments(
                        "export-range",
                        &options,
                        ranged,
                        label,
                        move |c, r| c.range = r.then_some((start, end)),
                        cx,
                    ))
                    .child(
                        div()
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT_MUTED))
                            .child(format!(
                                "{} – {}",
                                settings::duration_label(start),
                                settings::duration_label(end)
                            )),
                    ),
            )
        });

        // --- picture ---------------------------------------------------------
        let resolution = self.segments(
            "export-resolution",
            &Resolution::ALL,
            c.resolution,
            Resolution::label,
            |c, r| c.resolution = r,
            cx,
        );
        let video = match kind {
            OutputKind::Audio => None,
            OutputKind::Gif => Some(
                Section::new(
                    "export-video",
                    SectionHeader::new("export-video-header", "GIF"),
                )
                .child(Self::row("Resolution", resolution))
                .child(Self::row(
                    "Frame rate",
                    self.picker("export-fps", rates, false, cx),
                ))
                .child(Self::row(
                    "Size",
                    div()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_MUTED))
                        .child(format!("{width}×{height} · 252 colours · no sound")),
                )),
            ),
            OutputKind::Video => {
                let bitrate = self.segments(
                    "export-bitrate",
                    &Bitrate::ALL,
                    c.bitrate,
                    Bitrate::label,
                    |c, b| c.bitrate = b,
                    cx,
                );
                let codec = self.segments(
                    "export-codec",
                    &Codec::ALL,
                    c.codec,
                    Codec::label,
                    |c, codec| c.codec = codec,
                    cx,
                );
                let format = self.segments(
                    "export-format",
                    &Format::ALL,
                    c.format,
                    Format::label,
                    |c, f| c.format = f,
                    cx,
                );
                let prores = c.codec == Codec::ProRes;
                let ten_bit = c.ten_bit && c.codec.has_ten_bit();
                let depth = self.segments(
                    "export-depth",
                    &[false, true],
                    c.ten_bit,
                    |ten| if ten { "10-bit" } else { "8-bit" },
                    |c, ten| c.ten_bit = ten,
                    cx,
                );
                Some(
                    Section::new(
                        "export-video",
                        SectionHeader::new("export-video-header", "Video"),
                    )
                    .child(Self::row("Resolution", resolution))
                    .when(c.codec.has_quality(), |section| {
                        section.child(Self::row("Bitrate", bitrate))
                    })
                    .when(c.bitrate == Bitrate::Custom && !prores, |section| {
                        section.child(Self::row(
                            "Mbit/s",
                            div().w(px(120.0)).child(
                                Input::new(&self.custom_bitrate)
                                    .small()
                                    .font_family(FONT_MONO)
                                    .text_size(px(TEXT_LABEL)),
                            ),
                        ))
                    })
                    .child(Self::row("Codec", codec))
                    .when(!prores, |section| {
                        section.child(Self::row("Format", div().flex().child(format)))
                    })
                    .child(Self::row(
                        "Frame rate",
                        self.picker("export-fps", rates, false, cx),
                    ))
                    .child(Self::row("Encoder", div().flex().child(encoder)))
                    .when(c.codec.has_ten_bit(), |section| {
                        section.child(Self::row("Bit depth", depth))
                    })
                    .child(Self::row(
                        "Colour space",
                        div()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT_MUTED))
                            .child(settings::colour_space_label(prores, ten_bit, width, height)),
                    )),
                )
            }
        };

        // --- sound -----------------------------------------------------------
        let loudness = self.picker(
            "export-loudness",
            super::loudness::picks(c.loudness_target),
            !(c.audio || kind == OutputKind::Audio),
            cx,
        );
        let audio = match kind {
            OutputKind::Gif => None,
            OutputKind::Audio => {
                let formats = self.segments(
                    "export-audio-kind",
                    &AudioFormat::ALL,
                    c.audio_format,
                    AudioFormat::label,
                    |c, f| c.audio_format = f,
                    cx,
                );
                let wav = c.audio_format == AudioFormat::Wav;
                Some(
                    Section::new(
                        "export-audio",
                        SectionHeader::new("export-audio-header", "Audio"),
                    )
                    .border_b_0()
                    .child(Self::row("Format", formats))
                    .child(Self::row(
                        "Bitrate",
                        if wav {
                            div()
                                .text_size(px(TEXT_LABEL))
                                .text_color(rgb(TEXT_MUTED))
                                .child("16-bit PCM · 48 kHz · uncompressed")
                                .into_any_element()
                        } else {
                            self.picker("export-audio-format", audio_rates, false, cx)
                                .into_any_element()
                        },
                    ))
                    .child(Self::row("Loudness", loudness))
                    .child(Self::row("Mix now", self.mix_meter.clone())),
                )
            }
            OutputKind::Video => {
                let audio_on = c.audio;
                let dialog = cx.entity().downgrade();
                let prores = c.codec == Codec::ProRes;
                Some(
                    Section::new(
                        "export-audio",
                        SectionHeader::new("export-audio-header", "Audio").enable(
                            audio_on,
                            move |checked, _, cx| {
                                let _ = dialog.update(cx, |this, cx| {
                                    this.choices.audio = checked;
                                    this.choices.preset = None;
                                    cx.notify();
                                });
                            },
                        ),
                    )
                    .border_b_0()
                    .child(Self::row(
                        "Format",
                        if prores {
                            div()
                                .text_size(px(TEXT_LABEL))
                                .text_color(rgb(TEXT_MUTED))
                                .child("PCM 16-bit · 48 kHz")
                                .into_any_element()
                        } else {
                            self.picker("export-audio-format", audio_rates, !audio_on, cx)
                                .into_any_element()
                        },
                    ))
                    .child(Self::row("Loudness", loudness))
                    .child(Self::row("Mix now", self.mix_meter.clone())),
                )
            }
        };

        let problems: Vec<AnyElement> = match &plan {
            Ok(plan) => plan
                .warnings
                .iter()
                .map(|w| Badge::new(w.clone()).tone(Tone::Warning).into_any_element())
                .collect(),
            Err(error) => vec![Badge::new(error.clone())
                .tone(Tone::Danger)
                .into_any_element()],
        };

        div()
            .id("export-settings")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .px(px(20.0))
            .py(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(
                self.notice
                    .clone()
                    .map(|notice| div().flex().child(Badge::new(notice).tone(Tone::Warning))),
            )
            .children(super::licences::summary(&self.licences))
            .child(Self::row("Preset", preset_row))
            .children(save_row)
            .children(problems.into_iter().map(|badge| div().flex().child(badge)))
            .child(Self::row(
                "Name",
                Input::new(&self.name).small().text_size(px(TEXT_LABEL)),
            ))
            .child(Self::row("Export to", folder))
            .when(exists, |column| {
                column.child(
                    div().pl(px(crate::ui::LABEL_W + 12.0)).flex().child(
                        Badge::new(format!(
                            "{} exists and will be replaced",
                            file_name(&output.to_string_lossy())
                        ))
                        .tone(Tone::Warning),
                    ),
                )
            })
            .child(Self::row("Export as", kind_tabs))
            .children(range_row)
            .child(div().h(px(4.0)))
            .children(video)
            .children(audio)
    }

    fn render_progress(&self) -> impl IntoElement {
        let (title, body): (String, AnyElement) = match &self.phase {
            Phase::Starting => (
                "Starting…".into(),
                Progress::new("export-progress")
                    .loading(true)
                    .into_any_element(),
            ),
            Phase::Running {
                encoder, progress, ..
            } => {
                let fraction = progress.as_ref().map(|p| p.fraction).unwrap_or(0.0);
                let stage = progress.as_ref().map(|p| p.stage);
                let detail = match progress {
                    Some(p) => {
                        let left = p
                            .remaining_seconds
                            .map(|s| {
                                format!(
                                    " · {} left",
                                    settings::duration_label((s * 1_000_000.0) as Micros)
                                )
                            })
                            .unwrap_or_default();
                        format!(
                            "{:.0} % · frame {} of {} · {:.0} fps{left}",
                            p.fraction * 100.0,
                            p.frame,
                            p.total_frames,
                            p.fps
                        )
                    }
                    None => "Preparing…".into(),
                };
                let title = match stage {
                    Some(ExportStage::MixingAudio) => "Mixing audio…",
                    Some(ExportStage::Finalizing) => "Finishing the file…",
                    _ => "Exporting…",
                };
                (
                    title.into(),
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(28.0))
                                .text_color(rgb(TEXT))
                                .child(format!("{:.0}%", fraction * 100.0)),
                        )
                        .child(
                            Progress::new("export-progress")
                                .color(rgb(ACCENT))
                                .value(fraction * 100.0),
                        )
                        .child(
                            div()
                                .font_family(FONT_MONO)
                                .text_size(px(TEXT_LABEL))
                                .text_color(rgb(TEXT_DIM))
                                .child(detail),
                        )
                        .child(
                            div()
                                .flex()
                                .child(Badge::new(encoder.clone()).tone(Tone::Accent)),
                        )
                        .into_any_element(),
                )
            }
            Phase::Done {
                path,
                frames,
                seconds,
            } => (
                "Done".into(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(IconSrc::from(Lucide::CircleCheck).svg(18.0, rgb(SUCCESS)))
                            .child(
                                div()
                                    .font_family(FONT_MONO)
                                    .text_size(px(TEXT_LABEL))
                                    .text_color(rgb(TEXT_DIM))
                                    .child(format!("{frames} frames in {seconds:.1} s")),
                            ),
                    )
                    .child(
                        div()
                            .px(px(10.0))
                            .py(px(8.0))
                            .rounded(px(R_SM))
                            .bg(rgb(WELL))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT))
                            .child(settings::display_path(path)),
                    )
                    .children(self.credits.as_deref().map(super::licences::credits_line))
                    .into_any_element(),
            ),
            Phase::Failed(message) => (
                "Export failed".into(),
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(R_SM))
                    .bg(with_alpha(DANGER, 0.1))
                    .border_1()
                    .border_color(with_alpha(DANGER, 0.4))
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(DANGER))
                    .child(message.clone())
                    .into_any_element(),
            ),
            Phase::Settings => (String::new(), div().into_any_element()),
        };
        div()
            .flex_1()
            .min_w(px(0.0))
            .px(px(28.0))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(16.0))
            .child(
                div()
                    .text_size(px(TEXT_DISPLAY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child(title),
            )
            .child(body)
    }

    /// "Duration: 0:42 | Size: 12.4 MB (measured)".
    fn summary(&self) -> String {
        let plan = self.plan();
        let duration = match &plan {
            Ok(plan) => plan.duration,
            Err(_) => self.project.duration(),
        };
        let measured = self
            .measured
            .as_ref()
            .filter(|(asked, _)| *asked == self.request())
            .map(|(_, answer)| answer);
        let size = match (&plan, measured) {
            (_, Some(Some(Ok(estimate)))) => {
                format!("{} (measured)", settings::bytes_label(estimate.bytes))
            }
            (Ok(plan), Some(None)) if plan.estimate.is_rough() => {
                format!("{} · measuring…", settings::size_label(plan.estimate.bytes))
            }
            (Ok(plan), _) => settings::size_label(plan.estimate.bytes),
            (Err(_), _) => "—".into(),
        };
        format!(
            "Duration: {} | Size: {size}",
            settings::duration_label(duration)
        )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.summary();
        let valid = self.plan().is_ok();
        let buttons: Vec<AnyElement> = match &self.phase {
            Phase::Settings if self.show_queue => vec![
                Button::new("export-queue-clear")
                    .small()
                    .label("Clear finished")
                    .disabled(!self.queue.iter().any(|i| i.status.is_finished()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        export_commands::export_queue_clear_finished();
                        this.queue = export_commands::export_queue_list();
                        cx.notify();
                    }))
                    .into_any_element(),
                Button::new("export-queue-back")
                    .small()
                    .primary()
                    .label("Back to settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_queue = false;
                        cx.notify();
                    }))
                    .into_any_element(),
            ],
            Phase::Settings => vec![
                Button::new("export-start")
                    .small()
                    .primary()
                    .label("Export")
                    .disabled(!valid)
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                    .into_any_element(),
                Button::new("export-queue-add")
                    .small()
                    .icon(Lucide::Plus)
                    .label("Add to queue")
                    .disabled(!valid)
                    .on_click(cx.listener(|this, _, _, cx| this.add_to_queue(cx)))
                    .into_any_element(),
                Button::new("export-cancel")
                    .small()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                    .into_any_element(),
            ],
            Phase::Starting | Phase::Running { .. } => vec![Button::new("export-cancel")
                .small()
                .label("Cancel export")
                .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                .into_any_element()],
            Phase::Done { path, .. } => {
                let (folder, file) = (path.clone(), path.clone());
                vec![
                    Button::new("export-reveal")
                        .small()
                        .primary()
                        .icon(Lucide::FolderOpen)
                        .label("Show in folder")
                        .on_click(move |_, _, cx| cx.reveal_path(&folder))
                        .into_any_element(),
                    Button::new("export-play")
                        .small()
                        .icon(Lucide::Play)
                        .label("Play")
                        .on_click(move |_, _, cx| cx.open_with_system(&file))
                        .into_any_element(),
                    Button::new("export-close")
                        .small()
                        .label("Close")
                        .on_click(|_, window, cx| window.close_dialog(cx))
                        .into_any_element(),
                ]
            }
            Phase::Failed(_) => vec![
                Button::new("export-back")
                    .small()
                    .primary()
                    .label("Back to settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.phase = Phase::Settings;
                        cx.notify();
                    }))
                    .into_any_element(),
                Button::new("export-close")
                    .small()
                    .label("Close")
                    .on_click(|_, window, cx| window.close_dialog(cx))
                    .into_any_element(),
            ],
        };
        div()
            .h(px(56.0))
            .flex_none()
            .px(px(20.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .border_t_1()
            .border_color(rgb(HAIRLINE))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .child(icons::glyph(icons::MEDIA, 16.0, rgb(TEXT_MUTED)))
                    .child(summary),
            )
            .child(div().flex_1())
            .children(buttons)
    }
}

impl Render for ExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let right = match self.phase {
            Phase::Settings if self.show_queue => {
                super::queue::render_list(&self.queue, cx).into_any_element()
            }
            Phase::Settings => self.render_settings(cx).into_any_element(),
            _ => self.render_progress().into_any_element(),
        };
        let active = self
            .queue
            .iter()
            .filter(|i| !i.status.is_finished())
            .count();
        let queue_button = (!self.queue.is_empty()).then(|| {
            Button::new("export-queue-show")
                .small()
                .outline()
                .label(if active > 0 {
                    format!("Queue · {active}")
                } else {
                    format!("Queue · {} done", self.queue.len())
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_queue = !this.show_queue;
                    cx.notify();
                }))
        });
        div()
            .flex()
            .flex_col()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .h(px(52.0))
                    .flex_none()
                    .px(px(20.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .child(icons::glyph(icons::EXPORT, 18.0, rgb(ACCENT)))
                    .child(
                        div()
                            .text_size(px(TEXT_DISPLAY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Export"),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_BODY))
                            .text_color(rgb(TEXT_MUTED))
                            .child(self.project.name.clone()),
                    )
                    .child(div().flex_1())
                    .children(queue_button)
                    // Room for the dialog's own close button in the corner.
                    .child(div().w(px(28.0))),
            )
            .child(
                div()
                    .h(px(480.0))
                    .flex()
                    .flex_row()
                    .child(self.render_cover(cx))
                    .child(right),
            )
            .child(self.render_footer(cx))
    }
}

/// Whether two queue lists would draw the same.
fn queue_equal(
    a: &[chukcut_engine::modules::export::QueueItem],
    b: &[chukcut_engine::modules::export::QueueItem],
) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.id == y.id
                && x.status == y.status
                && x.progress.as_ref().map(|p| p.frame) == y.progress.as_ref().map(|p| p.frame)
        })
}

impl Drop for ExportDialog {
    fn drop(&mut self) {
        // A sample encode for a dialog nobody sees is wasted CPU.
        self.stop_measuring();
    }
}
