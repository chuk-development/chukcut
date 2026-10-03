//! The export dialog: cover on the left, settings on the right, duration and
//! estimated size in the footer — CapCut's arrangement. Once the export
//! starts, the right side turns into its progress, and at the end into
//! "Done" with a way to the file.

use std::rc::Rc;
use std::time::Duration;

use chukcut_engine::modules::export::hwaccel::{self, HwEncoder};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::checkbox::Checkbox;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::progress::Progress;
use gpui::component::{Disableable as _, Icon, Sizable as _, WindowExt as _};
use gpui::{AnyElement, Entity, Subscription, WeakEntity};

use super::settings::{self, Bitrate, Codec, ExportChoices, Format, Resolution};
use super::*;

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
    /// Progress for this dialog, written by the export thread.
    progress: Slot,
    /// The editor's progress slot, so the title bar keeps reporting when the
    /// dialog is closed after the export finishes.
    editor_progress: Slot,
    _subscriptions: Vec<Subscription>,
    _poll: Option<Task<()>>,
}

impl ExportDialog {
    pub(crate) fn new(
        editor: WeakEntity<Editor>,
        state: Arc<AppState>,
        project: Arc<Project>,
        editor_progress: Slot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let choices = ExportChoices::for_project(&project, settings::default_directory());
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
                        if mbps.is_finite() && mbps > 0.0 {
                            this.choices.custom_mbps = mbps;
                        }
                    }
                    cx.notify();
                }
            }),
        ];

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

        Self {
            editor,
            state,
            project,
            choices,
            name,
            custom_bitrate,
            encoders: None,
            phase: Phase::Settings,
            progress: Arc::new(parking_lot::Mutex::new(None)),
            editor_progress,
            _subscriptions: subscriptions,
            _poll: None,
        }
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
        let hardware = self.encoder().flatten().map(|hw| hw.id.clone());
        let encoder = self.encoder_label();
        let request = self.choices.request(&self.project, hardware);
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
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export here".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                if let Some(directory) = paths.into_iter().next() {
                    let _ = this.update(cx, |dialog, cx| {
                        dialog.choices.directory = directory;
                        cx.notify();
                    });
                }
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
                                        cx.notify();
                                    });
                                }),
                        )
                    })
            })
    }

    fn row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .min_h(px(32.0))
            .child(
                div()
                    .w(px(104.0))
                    .flex_none()
                    .text_sm()
                    .text_color(rgb(TEXT_DIM))
                    .child(label),
            )
            .child(div().flex_1().min_w(px(0.0)).child(control))
    }

    fn section(title: &'static str) -> gpui::Div {
        div()
            .pt_2()
            .mt_1()
            .border_t_1()
            .border_color(rgb(BORDER))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(rgb(TEXT))
            .child(title)
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
            .p_4()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .child(
                div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(rgb(0x000000))
                    .children(frame.map(|frame| img(frame).size_full()))
                    .child(
                        div()
                            .absolute()
                            .top(px(8.0))
                            .left(px(8.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.55))
                            .text_xs()
                            .text_color(rgb(TEXT))
                            .child(Icon::new(Lucide::Image).size(px(12.0)))
                            .child("Cover"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(TEXT_DIM))
                    .child("The frame at the playhead"),
            )
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.choices;
        let (width, height) = c.size(&self.project);
        let output = c.output_path();
        let exists = output.exists();

        let resolutions = Resolution::ALL
            .into_iter()
            .map(|r| -> Pick {
                let (w, h) = r.size_for(self.project.canvas.width, self.project.canvas.height);
                (
                    format!("{} · {w}×{h}", r.label()).into(),
                    r == c.resolution,
                    Rc::new(move |c: &mut ExportChoices| c.resolution = r),
                )
            })
            .collect();
        let bitrates = Bitrate::ALL
            .into_iter()
            .map(|b| -> Pick {
                (
                    b.label().into(),
                    b == c.bitrate,
                    Rc::new(move |c: &mut ExportChoices| c.bitrate = b),
                )
            })
            .collect();
        let codecs = Codec::ALL
            .into_iter()
            .map(|codec| -> Pick {
                (
                    codec.label().into(),
                    codec == c.codec,
                    Rc::new(move |c: &mut ExportChoices| c.codec = codec),
                )
            })
            .collect();
        let formats = Format::ALL
            .into_iter()
            .map(|f| -> Pick {
                (
                    f.label().into(),
                    f == c.format,
                    Rc::new(move |c: &mut ExportChoices| c.format = f),
                )
            })
            .collect();
        let rates = settings::FRAME_RATES
            .into_iter()
            .map(|(rate, label)| -> Pick {
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
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(rgb(PANEL_RAISED))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(settings::display_path(&c.directory)),
            )
            .child(
                Button::new("export-folder")
                    .icon(Lucide::FolderOpen)
                    .outline()
                    .small()
                    .tooltip("Choose a folder")
                    .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
            );

        let audio_on = c.audio;
        div()
            .id("export-settings")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::row("Name", Input::new(&self.name).small()))
            .child(Self::row("Export to", folder))
            .when(exists, |column| {
                column.child(
                    div()
                        .pl(px(116.0))
                        .text_xs()
                        .text_color(rgb(0xe0a84a))
                        .child(format!(
                            "{} exists and will be replaced",
                            file_name(&output.to_string_lossy())
                        )),
                )
            })
            .child(Self::section("Video"))
            .child(Self::row(
                "Resolution",
                self.picker("export-resolution", resolutions, false, cx),
            ))
            .child(Self::row(
                "Bitrate",
                self.picker("export-bitrate", bitrates, false, cx),
            ))
            .when(c.bitrate == Bitrate::Custom, |column| {
                column.child(Self::row(
                    "Mbit/s",
                    Input::new(&self.custom_bitrate).small(),
                ))
            })
            .child(Self::row(
                "Codec",
                self.picker("export-codec", codecs, false, cx),
            ))
            .child(Self::row(
                "Format",
                self.picker("export-format", formats, false, cx),
            ))
            .child(Self::row(
                "Frame rate",
                self.picker("export-fps", rates, false, cx),
            ))
            .child(Self::row(
                "Encoder",
                div()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(self.encoder_label()),
            ))
            .child(Self::row(
                "Colour space",
                div()
                    .text_sm()
                    .text_color(rgb(TEXT_DIM))
                    .child(format!("Rec. 709 SDR · {width}×{height}")),
            ))
            .child(
                Self::section("Audio").child(
                    Checkbox::new("export-audio")
                        .checked(audio_on)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.choices.audio = *checked;
                            cx.notify();
                        })),
                ),
            )
            .child(Self::row(
                "Format",
                self.picker("export-audio-format", audio_rates, !audio_on, cx),
            ))
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
                        .gap_2()
                        .child(Progress::new("export-progress").value(fraction * 100.0))
                        .child(div().text_sm().text_color(rgb(TEXT)).child(detail))
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(TEXT_DIM))
                                .child(format!("Encoder: {encoder}")),
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
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .text_color(rgb(ACCENT))
                            .child(Icon::new(Lucide::CircleCheck).size(px(18.0)))
                            .child(
                                div()
                                    .text_sm()
                                    .child(format!("{frames} frames in {seconds:.1} s")),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(settings::display_path(path)),
                    )
                    .into_any_element(),
            ),
            Phase::Failed(message) => (
                "Export failed".into(),
                div()
                    .text_sm()
                    .text_color(rgb(0xe5484d))
                    .child(message.clone())
                    .into_any_element(),
            ),
            Phase::Settings => (String::new(), div().into_any_element()),
        };
        div()
            .flex_1()
            .min_w(px(0.0))
            .p_4()
            .pt(px(48.0))
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child(title),
            )
            .child(body)
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = format!(
            "Duration: {} | Size: {}",
            settings::duration_label(self.project.duration()),
            settings::size_label(self.choices.estimated_bytes(&self.project))
        );
        let buttons: Vec<AnyElement> = match &self.phase {
            Phase::Settings => vec![
                Button::new("export-start")
                    .primary()
                    .label("Export")
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
                    .into_any_element(),
                Button::new("export-cancel")
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                    .into_any_element(),
            ],
            Phase::Starting | Phase::Running { .. } => vec![Button::new("export-cancel")
                .label("Cancel export")
                .on_click(cx.listener(|this, _, window, cx| this.cancel(window, cx)))
                .into_any_element()],
            Phase::Done { path, .. } => {
                let (folder, file) = (path.clone(), path.clone());
                vec![
                    Button::new("export-reveal")
                        .primary()
                        .icon(Lucide::FolderOpen)
                        .label("Show in folder")
                        .on_click(move |_, _, cx| cx.reveal_path(&folder))
                        .into_any_element(),
                    Button::new("export-play")
                        .icon(Lucide::Play)
                        .label("Play")
                        .on_click(move |_, _, cx| cx.open_with_system(&file))
                        .into_any_element(),
                    Button::new("export-close")
                        .label("Close")
                        .on_click(|_, window, cx| window.close_dialog(cx))
                        .into_any_element(),
                ]
            }
            Phase::Failed(_) => vec![
                Button::new("export-back")
                    .primary()
                    .label("Back to settings")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.phase = Phase::Settings;
                        cx.notify();
                    }))
                    .into_any_element(),
                Button::new("export-close")
                    .label("Close")
                    .on_click(|_, window, cx| window.close_dialog(cx))
                    .into_any_element(),
            ],
        };
        div()
            .h(px(56.0))
            .flex_none()
            .px_4()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .text_color(rgb(TEXT_DIM))
                    .child(Icon::new(Lucide::Film).size(px(16.0)))
                    .child(summary),
            )
            .child(div().flex_1())
            .children(buttons)
    }
}

impl Render for ExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let right = match self.phase {
            Phase::Settings => self.render_settings(cx).into_any_element(),
            _ => self.render_progress().into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .h(px(40.0))
                    .flex_none()
                    .px_4()
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("Export – {}", self.project.name)),
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
