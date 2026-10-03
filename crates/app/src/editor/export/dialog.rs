//! The export dialog: cover on the left, settings on the right, duration and
//! estimated size in the footer — CapCut's arrangement. Once the export
//! starts, the right side turns into its progress, and at the end into
//! "Done" with a way to the file.

use std::rc::Rc;
use std::time::Duration;

use chukcut_engine::modules::export::hwaccel::{self, HwEncoder};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::progress::Progress;
use gpui::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui::{AnyElement, Entity, Subscription, WeakEntity};

use super::settings::{self, Bitrate, Codec, ExportChoices, Format, Resolution};
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
                settings::remember_directory(&self.choices.directory);
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
                    apply(&mut dialog.choices, pick);
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
                    .children(frame.map(|frame| img(frame).size_full()))
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

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let c = &self.choices;
        let (width, height) = c.size(&self.project);
        let output = c.output_path();
        let exists = output.exists();

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
            Some(Some(_)) => Badge::new(self.encoder_label()).tone(Tone::Accent),
            Some(None) => Badge::new(self.encoder_label()),
        };

        let resolution = self.segments(
            "export-resolution",
            &Resolution::ALL,
            c.resolution,
            Resolution::label,
            |c, r| c.resolution = r,
            cx,
        );
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

        let audio_on = c.audio;
        let video = Section::new(
            "export-video",
            SectionHeader::new("export-video-header", "Video"),
        )
        .child(Self::row("Resolution", resolution))
        .child(Self::row("Bitrate", bitrate))
        .when(c.bitrate == Bitrate::Custom, |section| {
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
        .child(Self::row("Format", div().flex().child(format)))
        .child(Self::row(
            "Frame rate",
            self.picker("export-fps", rates, false, cx),
        ))
        .child(Self::row("Encoder", div().flex().child(encoder)))
        .child(Self::row(
            "Colour space",
            div()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(TEXT_MUTED))
                .child(format!("Rec. 709 SDR · {width}×{height}")),
        ));
        let dialog = cx.entity().downgrade();
        let audio = Section::new(
            "export-audio",
            SectionHeader::new("export-audio-header", "Audio").enable(
                audio_on,
                move |checked, _, cx| {
                    let _ = dialog.update(cx, |this, cx| {
                        this.choices.audio = checked;
                        cx.notify();
                    });
                },
            ),
        )
        .border_b_0()
        .child(Self::row(
            "Format",
            self.picker("export-audio-format", audio_rates, !audio_on, cx),
        ))
        .child(Self::row(
            "Loudness",
            self.picker(
                "export-loudness",
                super::loudness::picks(c.loudness_target),
                !audio_on,
                cx,
            ),
        ))
        .child(Self::row("Mix now", self.mix_meter.clone()));

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
            .child(div().h(px(4.0)))
            .child(video)
            .child(audio)
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

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = format!(
            "Duration: {} | Size: {}",
            settings::duration_label(self.project.duration()),
            settings::size_label(self.choices.estimated_bytes(&self.project))
        );
        let buttons: Vec<AnyElement> = match &self.phase {
            Phase::Settings => vec![
                Button::new("export-start")
                    .small()
                    .primary()
                    .label("Export")
                    .on_click(cx.listener(|this, _, _, cx| this.start(cx)))
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
            Phase::Settings => self.render_settings(cx).into_any_element(),
            _ => self.render_progress().into_any_element(),
        };
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
                    ),
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
