//! "Remove silences": the review panel for cutting pauses and filler words
//! out of a talking-head clip.
//!
//! Every plugin that does this shows the cuts before making them, because a
//! wrong cut halves a word (`docs/research/resolve-plugins.md` §6.2). So the
//! panel lists every cut — where it is, how long, a mini waveform with the
//! cut part marked — with a switch per cut, and the sliders re-run detection
//! live on the envelope the engine measured once when the panel opened.
//! Clicking a row moves the playhead there so the cut can be checked in the
//! player. "Remove" applies the chosen cuts as **one** undo step through
//! `silence_remove`.
//!
//! Filler words are the second mode: their cuts come from the clip's
//! transcript (`silence_filler_cuts`), and the panel says so when there is no
//! transcript yet.

use std::sync::atomic::{AtomicBool, Ordering};

use chukcut_engine::modules::project::TimeRange;
use chukcut_engine::modules::silence::commands::{
    silence_analyse, silence_detect, silence_filler_cuts, silence_remove, Analysis,
};
use chukcut_engine::modules::silence::filler::LANGUAGES;
use chukcut_engine::modules::silence::{Method, SilenceParams};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::checkbox::Checkbox;
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui::{AnyElement, Entity, Subscription, WeakEntity};

use super::*;
use crate::ui::{icons, Badge, EmptyState, PropertyRow, SegmentedTabs, Tone};

/// Which kind of cut the panel is reviewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CutMode {
    Pauses,
    Fillers,
}

/// Bars in each row's mini waveform.
const BARS: usize = 56;
/// Context shown either side of a cut in its mini waveform.
const CONTEXT: Micros = 600_000;
/// Default padding around a filler word, in microseconds.
const FILLER_PADDING: Micros = 40_000;

struct Cut {
    range: TimeRange,
    /// Whether this cut will be made. Switched off by the user to keep it.
    on: bool,
}

enum Phase {
    Analysing,
    Ready,
    Failed(String),
}

pub(crate) struct SilencePanel {
    editor: WeakEntity<Editor>,
    state: Arc<AppState>,
    segment_id: String,
    clip_name: String,
    mode: CutMode,
    phase: Phase,
    analysis: Option<Arc<Analysis>>,
    params: SilenceParams,
    filler_padding: Micros,
    language: &'static str,
    /// Why there are no filler cuts, when that is not "there are none".
    filler_note: Option<String>,
    /// Ripple every other lane (captions, music, overlays) across each cut
    /// too. On by default: captions made before the cut stay on their words.
    keep_in_sync: bool,
    cuts: Vec<Cut>,
    threshold: Entity<SliderState>,
    min_pause: Entity<SliderState>,
    padding: Entity<SliderState>,
    error: Option<SharedString>,
    cancel: Arc<AtomicBool>,
    _subscriptions: Vec<Subscription>,
}

impl SilencePanel {
    fn new(
        editor: WeakEntity<Editor>,
        state: Arc<AppState>,
        segment_id: String,
        clip_name: String,
        mode: CutMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let params = SilenceParams::default();
        let slider = |min: f32, max: f32, step: f32, value: f32, cx: &mut Context<Self>| {
            cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step)
                    .default_value(value)
            })
        };
        let threshold = slider(-70.0, -10.0, 1.0, params.threshold_db, cx);
        let min_pause = slider(
            100.0,
            3_000.0,
            50.0,
            (params.min_silence / 1_000) as f32,
            cx,
        );
        let padding = slider(0.0, 500.0, 10.0, (params.padding / 1_000) as f32, cx);

        let subscriptions = vec![
            cx.subscribe_in(&threshold, window, |this, _, event: &SliderEvent, _, cx| {
                let value = slider_value(event);
                this.params.threshold_db = value;
                this.redetect(cx);
            }),
            cx.subscribe_in(&min_pause, window, |this, _, event: &SliderEvent, _, cx| {
                this.params.min_silence = (slider_value(event) * 1_000.0) as Micros;
                this.redetect(cx);
            }),
            cx.subscribe_in(&padding, window, |this, _, event: &SliderEvent, _, cx| {
                let micros = (slider_value(event) * 1_000.0) as Micros;
                match this.mode {
                    CutMode::Pauses => this.params.padding = micros,
                    CutMode::Fillers => this.filler_padding = micros,
                }
                this.redetect(cx);
            }),
        ];

        let mut panel = Self {
            editor,
            state,
            segment_id,
            clip_name,
            mode,
            phase: Phase::Analysing,
            analysis: None,
            params,
            filler_padding: FILLER_PADDING,
            language: "auto",
            filler_note: None,
            keep_in_sync: true,
            cuts: Vec::new(),
            threshold,
            min_pause,
            padding,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
            _subscriptions: subscriptions,
        };
        panel.analyse(false, window, cx);
        panel
    }

    /// Measure the clip, off the UI thread. With `voice`, RNNoise's voice
    /// probability too, which roughly doubles the time.
    fn analyse(&mut self, voice: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.phase = Phase::Analysing;
        let state = Arc::clone(&self.state);
        let segment = self.segment_id.clone();
        let cancel = Arc::clone(&self.cancel);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { silence_analyse(&state, segment, voice, &cancel) })
                .await;
            let _ = this.update_in(cx, |panel, window, cx| {
                match result {
                    Ok(analysis) => {
                        let first = panel.analysis.is_none();
                        if first {
                            // Start from a threshold that fits this recording
                            // rather than a fixed −40 dB.
                            panel.params.threshold_db = analysis.suggested_threshold_db;
                            panel.threshold.update(cx, |slider, cx| {
                                slider.set_value(analysis.suggested_threshold_db, window, cx)
                            });
                        }
                        panel.analysis = Some(Arc::new(analysis));
                        panel.phase = Phase::Ready;
                        panel.redetect(cx);
                    }
                    Err(error) => panel.phase = Phase::Failed(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Recompute the cut list, keeping every cut the user switched off
    /// switched off where it still overlaps a new one.
    fn redetect(&mut self, cx: &mut Context<Self>) {
        let kept: Vec<TimeRange> = self
            .cuts
            .iter()
            .filter(|c| !c.on)
            .map(|c| c.range)
            .collect();
        let ranges = match self.mode {
            CutMode::Pauses => {
                self.filler_note = None;
                match &self.analysis {
                    Some(analysis) => silence_detect(analysis, &self.params),
                    None => Vec::new(),
                }
            }
            CutMode::Fillers => match silence_filler_cuts(
                &self.state,
                self.segment_id.clone(),
                self.language.to_string(),
                self.filler_padding,
            ) {
                Ok(ranges) => {
                    self.filler_note = None;
                    ranges
                }
                Err(note) => {
                    self.filler_note = Some(note);
                    Vec::new()
                }
            },
        };
        self.cuts = ranges
            .into_iter()
            .map(|range| Cut {
                on: !kept.iter().any(|k| k.overlaps(&range)),
                range,
            })
            .collect();
        cx.notify();
    }

    fn set_mode(&mut self, mode: CutMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.cuts.clear();
        let padding = match mode {
            CutMode::Pauses => self.params.padding,
            CutMode::Fillers => self.filler_padding,
        };
        self.padding.update(cx, |slider, cx| {
            slider.set_value((padding / 1_000) as f32, window, cx)
        });
        self.redetect(cx);
    }

    fn set_voice(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.params.method = if on { Method::Voice } else { Method::Energy };
        let measured = self
            .analysis
            .as_ref()
            .is_some_and(|a| a.envelope.voice.is_some());
        if on && !measured {
            self.analyse(true, window, cx);
        } else {
            self.redetect(cx);
        }
    }

    fn chosen(&self) -> Vec<TimeRange> {
        self.cuts.iter().filter(|c| c.on).map(|c| c.range).collect()
    }

    /// The timeline time of source time `t` in the clip.
    fn on_timeline(&self, t: Micros) -> Micros {
        match &self.analysis {
            Some(analysis) => analysis.timeline_time(t),
            None => t,
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cuts = self.chosen();
        if cuts.is_empty() {
            return;
        }
        let label = match self.mode {
            CutMode::Pauses => "Remove silences",
            CutMode::Fillers => "Remove filler words",
        };
        let result = silence_remove(
            &self.state,
            self.segment_id.clone(),
            cuts,
            label.to_string(),
            self.keep_in_sync,
        )
        .map(|_| ());
        match result {
            Ok(()) => {
                let _ = self.editor.update(cx, |editor, cx| {
                    editor.refresh(cx);
                    editor.report(Ok(()), cx);
                });
                window.close_dialog(cx);
            }
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn seek_to(&self, t: Micros, cx: &mut Context<Self>) {
        let at = self.on_timeline(t);
        let _ = self.editor.update(cx, |editor, cx| {
            editor.seek(at);
            cx.notify();
        });
    }

    // --- drawing ----------------------------------------------------------------

    fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = |label: &'static str, control: AnyElement, value: String| {
            PropertyRow::new(SharedString::from(format!("silence-{label}")), label)
                .no_actions()
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .child(div().flex_1().child(control))
                        .child(
                            div()
                                .w(px(64.0))
                                .text_right()
                                .font_family(FONT_MONO)
                                .text_size(px(TEXT_LABEL))
                                .text_color(rgb(TEXT))
                                .child(value),
                        ),
                )
                .into_any_element()
        };
        let mut rows = Vec::new();
        {
            let panel = cx.entity().downgrade();
            rows.push(
                PropertyRow::new("silence-sync", "Keep in sync")
                    .no_actions()
                    .child(
                        Checkbox::new("silence-sync-check")
                            .small()
                            .checked(self.keep_in_sync)
                            .label("Keep everything in sync: captions, music and overlays move too")
                            .on_click(move |checked, _, cx| {
                                let _ = panel.update(cx, |panel, cx| {
                                    panel.keep_in_sync = *checked;
                                    cx.notify();
                                });
                            }),
                    )
                    .into_any_element(),
            );
        }
        match self.mode {
            CutMode::Pauses => {
                rows.push(row(
                    "Threshold",
                    Slider::new(&self.threshold).into_any_element(),
                    format!("{:.0} dB", self.params.threshold_db),
                ));
                rows.push(row(
                    "Shortest pause",
                    Slider::new(&self.min_pause).into_any_element(),
                    format!("{:.2} s", self.params.min_silence as f64 / 1e6),
                ));
                rows.push(row(
                    "Padding",
                    Slider::new(&self.padding).into_any_element(),
                    format!("{} ms", self.params.padding / 1_000),
                ));
                let voice = self.params.method == Method::Voice;
                let panel = cx.entity().downgrade();
                rows.push(
                    PropertyRow::new("silence-voice", "Detect voice")
                        .no_actions()
                        .child(
                            Checkbox::new("silence-voice-check")
                                .small()
                                .checked(voice)
                                .label("Also cut breaths and noise that is not speech · slower")
                                .on_click(move |checked, window, cx| {
                                    let _ = panel.update(cx, |panel, cx| {
                                        panel.set_voice(*checked, window, cx)
                                    });
                                }),
                        )
                        .into_any_element(),
                );
            }
            CutMode::Fillers => {
                let selected = LANGUAGES
                    .iter()
                    .position(|(code, _)| *code == self.language)
                    .unwrap_or(0);
                let panel = cx.entity().downgrade();
                rows.push(
                    PropertyRow::new("silence-language", "Language")
                        .no_actions()
                        .child(
                            SegmentedTabs::new(
                                "silence-language-tabs",
                                LANGUAGES.iter().map(
                                    |(code, name)| {
                                        if *code == "auto" {
                                            "Any"
                                        } else {
                                            name
                                        }
                                    },
                                ),
                                selected,
                            )
                            .on_select(move |index, _, cx| {
                                let _ = panel.update(cx, |panel, cx| {
                                    panel.language = LANGUAGES[index].0;
                                    panel.redetect(cx);
                                });
                            }),
                        )
                        .into_any_element(),
                );
                rows.push(row(
                    "Padding",
                    Slider::new(&self.padding).into_any_element(),
                    format!("{} ms", self.filler_padding / 1_000),
                ));
            }
        }
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .px(px(20.0))
            .py(px(12.0))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .children(rows)
            .into_any_element()
    }

    fn render_summary(&self, cx: &mut Context<Self>) -> AnyElement {
        let chosen: Vec<&Cut> = self.cuts.iter().filter(|c| c.on).collect();
        let removed: Micros = chosen
            .iter()
            .map(|c| self.on_timeline(c.range.end()) - self.on_timeline(c.range.start))
            .sum();
        let total = self
            .analysis
            .as_ref()
            .map(|a| self.on_timeline(a.source.end()) - a.timeline_start)
            .unwrap_or(0);
        let text = format!(
            "{} of {} cuts · {:.1} s of {:.1} s removed",
            chosen.len(),
            self.cuts.len(),
            removed as f64 / 1e6,
            total as f64 / 1e6
        );
        let all = |on: bool, label: &'static str, cx: &mut Context<Self>| {
            Button::new(SharedString::from(format!("silence-all-{on}")))
                .xsmall()
                .ghost()
                .label(label)
                .disabled(self.cuts.is_empty())
                .on_click(cx.listener(move |this, _, _, cx| {
                    for cut in &mut this.cuts {
                        cut.on = on;
                    }
                    cx.notify();
                }))
        };
        div()
            .h(px(36.0))
            .flex_none()
            .px(px(20.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT_DIM))
            .child(text)
            .child(div().flex_1())
            .child(all(true, "Cut all", cx))
            .child(all(false, "Keep all", cx))
            .into_any_element()
    }

    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Phase::Analysing = self.phase {
            return EmptyState::new("silence-busy", icons::AUDIO, "Listening to the clip…")
                .hint("Measuring the level every 10 ms")
                .into_any_element();
        }
        if let Phase::Failed(error) = &self.phase {
            return EmptyState::new(
                "silence-failed",
                icons::AUDIO,
                "Could not read the clip's sound",
            )
            .hint(sentence(error))
            .into_any_element();
        }
        if let Some(note) = &self.filler_note {
            return EmptyState::new("silence-no-transcript", icons::TEXT, "No transcript yet")
                .hint(sentence(note))
                .into_any_element();
        }
        if self.cuts.is_empty() {
            let hint = match self.mode {
                CutMode::Pauses => "Raise the threshold or shorten the shortest pause",
                CutMode::Fillers => "The transcript has no filler words in this language",
            };
            return EmptyState::new("silence-nothing", icons::AUDIO, "Nothing to cut")
                .hint(hint)
                .into_any_element();
        }
        let rows: Vec<AnyElement> = self
            .cuts
            .iter()
            .enumerate()
            .map(|(index, cut)| self.render_cut(index, cut, cx))
            .collect();
        div()
            .id("silence-list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(rows)
            .into_any_element()
    }

    fn render_cut(&self, index: usize, cut: &Cut, cx: &mut Context<Self>) -> AnyElement {
        let start = self.on_timeline(cut.range.start);
        let length = self.on_timeline(cut.range.end()) - start;
        let wave = self.render_wave(cut);
        let on = cut.on;
        let source_start = cut.range.start;
        div()
            .id(SharedString::from(format!("silence-cut-{index}")))
            .h(px(40.0))
            .flex_none()
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .rounded(px(R_SM))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL_RAISED)))
            .on_click(cx.listener(move |this, _, _, cx| this.seek_to(source_start, cx)))
            .child(
                Checkbox::new(SharedString::from(format!("silence-cut-check-{index}")))
                    .checked(on)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if let Some(cut) = this.cuts.get_mut(index) {
                            cut.on = *checked;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(72.0))
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(if on { TEXT } else { TEXT_MUTED }))
                    .child(clock(start)),
            )
            .child(
                div()
                    .w(px(64.0))
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(if on { DANGER } else { TEXT_MUTED }))
                    .child(format!("−{:.2} s", length as f64 / 1e6)),
            )
            .child(wave)
            .into_any_element()
    }

    /// The cut and a little context either side; the part that will go is
    /// drawn in the danger colour, the rest dim.
    fn render_wave(&self, cut: &Cut) -> AnyElement {
        let Some(analysis) = &self.analysis else {
            return div().flex_1().into_any_element();
        };
        let from = (cut.range.start - CONTEXT).max(analysis.source.start);
        let to = (cut.range.end() + CONTEXT).min(analysis.source.end());
        let window = TimeRange::new(from, (to - from).max(1));
        let bars = analysis.envelope.bars(window, BARS);
        let bar_time =
            |i: usize| from + window.duration * (2 * i as Micros + 1) / (2 * BARS as Micros);
        div()
            .flex_1()
            .h(px(26.0))
            .flex()
            .flex_row()
            .items_center()
            .px(px(4.0))
            .rounded(px(R_XS))
            .overflow_hidden()
            .bg(rgb(WELL))
            .children(bars.into_iter().enumerate().map(|(i, height)| {
                let inside = cut.range.contains(bar_time(i));
                let color = match (inside, cut.on) {
                    (true, true) => DANGER,
                    (true, false) => TEXT_MUTED,
                    (false, _) => CLIP_AUDIO_WAVE,
                };
                // The part that goes is tinted behind its bars too, so a cut
                // through digital silence (bars of zero height) still shows.
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .when(inside && cut.on, |column| {
                        column.bg(with_alpha(DANGER, 0.16))
                    })
                    .child(
                        div()
                            .w_full()
                            .h(px((height * 22.0).max(2.0)))
                            .rounded(px(1.0))
                            .bg(rgb(color)),
                    )
            }))
            .into_any_element()
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.cuts.iter().filter(|c| c.on).count();
        let label = match (self.mode, count) {
            (_, 0) => "Nothing to remove".to_string(),
            (CutMode::Pauses, 1) => "Remove 1 pause".to_string(),
            (CutMode::Pauses, n) => format!("Remove {n} pauses"),
            (CutMode::Fillers, 1) => "Remove 1 filler".to_string(),
            (CutMode::Fillers, n) => format!("Remove {n} fillers"),
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
            .children(
                self.error
                    .clone()
                    .map(|error| Badge::new(error).tone(Tone::Danger)),
            )
            .when(self.error.is_none(), |row| {
                row.child(
                    div()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(TEXT_MUTED))
                        .child("One undo step brings everything back"),
                )
            })
            .child(div().flex_1())
            .child(
                Button::new("silence-cancel")
                    .small()
                    .label("Cancel")
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .child(
                Button::new("silence-apply")
                    .small()
                    .primary()
                    .label(label)
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
            )
            .into_any_element()
    }
}

impl Render for SilencePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = match self.mode {
            CutMode::Pauses => 0,
            CutMode::Fillers => 1,
        };
        let panel = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .h(px(52.0))
                    .flex_none()
                    .pl(px(20.0))
                    // Room for the dialog's own close button.
                    .pr(px(48.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .child(icons::glyph(icons::AUDIO, 18.0, rgb(ACCENT)))
                    .child(
                        div()
                            .text_size(px(TEXT_DISPLAY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Remove silences"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(TEXT_BODY))
                            .text_color(rgb(TEXT_MUTED))
                            .child(self.clip_name.clone()),
                    )
                    .child(
                        div().w(px(220.0)).child(
                            SegmentedTabs::new("silence-mode", ["Pauses", "Filler words"], mode)
                                .on_select(move |index, window, cx| {
                                    let mode = if index == 0 {
                                        CutMode::Pauses
                                    } else {
                                        CutMode::Fillers
                                    };
                                    let _ = panel
                                        .update(cx, |panel, cx| panel.set_mode(mode, window, cx));
                                }),
                        ),
                    ),
            )
            .child(self.render_controls(cx))
            .child(self.render_summary(cx))
            .child(
                div()
                    .h(px(330.0))
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(rgb(HAIRLINE))
                    .child(self.render_list(cx)),
            )
            .child(self.render_footer(cx))
    }
}

impl Drop for SilencePanel {
    fn drop(&mut self) {
        // Closing the panel mid-analysis stops the decode.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn slider_value(event: &SliderEvent) -> f32 {
    match event {
        SliderEvent::Change(value) | SliderEvent::Release(value) => value.end(),
    }
}

/// An engine message as a sentence: the command layer writes its prose to be
/// embedded mid-sentence, starting in lower case.
fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `m:ss.cc`, the precision a cut list needs.
fn clock(time: Micros) -> String {
    let centis = time.max(0) / 10_000;
    format!(
        "{}:{:02}.{:02}",
        centis / 6_000,
        (centis / 100) % 60,
        centis % 100
    )
}

impl Editor {
    /// Open the review panel for the selected clip.
    pub(super) fn open_silence_panel(
        &mut self,
        mode: CutMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(segment_id) = self.selected.clone() else {
            self.status = Some("Select a clip with sound first".into());
            cx.notify();
            return;
        };
        if self.clock.is_playing() {
            self.pause();
        }
        let clip_name = self
            .project
            .segment(&segment_id)
            .map(|(_, s)| self.material_name(&s.material_id))
            .unwrap_or_default();
        let editor = cx.entity().downgrade();
        let state = Arc::clone(&self.state);
        let panel =
            cx.new(|cx| SilencePanel::new(editor, state, segment_id, clip_name, mode, window, cx));
        window.open_dialog(cx, move |surface, _, _| {
            surface
                .w(px(760.0))
                .p_0()
                .bg(rgb(PANEL))
                .border_color(rgb(BORDER))
                .child(panel.clone())
        });
    }
}
