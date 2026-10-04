//! The Audio tab's voice tools: Normalize loudness, Reduce noise, and the way
//! into the "Remove silences" review panel.
//!
//! Both switches run an engine command that does slow work first (a loudness
//! measurement, a denoise render), so they run off the UI thread and report
//! progress in the status line. Each lands as one undo step. They act on the
//! clip that is *heard* — the audio partner of a linked picture — which the
//! engine resolves itself (`voice::audible_segment`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use chukcut_engine::modules::loudness::TARGETS;
use chukcut_engine::modules::voice::commands as voice_commands;
use chukcut_engine::modules::voice::{cleanup_of, VoiceCleanup};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::Sizable as _;
use gpui::AnyElement;

use super::super::silence::CutMode;
use super::controls::*;
use super::*;
use crate::ui::SegmentedTabs;

/// Noise reduction strengths the panel offers. Three steps rather than a
/// continuous slider, because every new strength is a render of the whole
/// file (`docs/decisions/0015-voice-cleanup-engine.md`).
const STRENGTHS: [(&str, f32); 3] = [("Light", 0.5), ("Medium", 0.75), ("Strong", 1.0)];

const NORMALIZE: &str = "Normalize loudness";
const NOISE: &str = "Reduce noise";
const SILENCES: &str = "Remove silences";

/// A voice command to run in the background.
#[derive(Clone, Copy)]
enum VoiceJob {
    Normalize(Option<f32>),
    Denoise(Option<f32>),
}

impl VoiceJob {
    fn busy(self) -> &'static str {
        match self {
            VoiceJob::Normalize(Some(_)) => "Measuring loudness…",
            VoiceJob::Normalize(None) => "Removing normalisation…",
            VoiceJob::Denoise(Some(_)) => "Reducing noise…",
            VoiceJob::Denoise(None) => "Restoring the original sound…",
        }
    }
}

impl Editor {
    /// The sections under Basic in the Audio tab, for the sound clip
    /// `segment`.
    pub(super) fn voice_sections(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let cleanup = cleanup_of(&self.project, segment)
            .map(|(_, c)| c)
            .unwrap_or_default();
        vec![
            self.normalize_section(&cleanup, cx),
            self.noise_section(&cleanup, cx),
            self.silences_section(cx),
            self.isolate_section(&segment.id, &cleanup, cx),
        ]
    }

    fn normalize_section(&mut self, cleanup: &VoiceCleanup, cx: &mut Context<Self>) -> AnyElement {
        let normalize = cleanup.normalize.clone();
        let mut rows = Vec::new();
        if let Some(n) = &normalize {
            let selected = TARGETS
                .iter()
                .position(|(lufs, _)| (*lufs - n.target_lufs).abs() < 0.05)
                .unwrap_or(0);
            let editor = cx.entity().downgrade();
            rows.push(label_row(
                "Target",
                SegmentedTabs::new(
                    "voice-normalize-target",
                    TARGETS.iter().map(|(_, label)| *label),
                    selected,
                )
                .on_select(move |index, _, cx| {
                    let target = TARGETS[index].0;
                    let _ = editor.update(cx, |this, cx| {
                        this.run_voice(VoiceJob::Normalize(Some(target)), cx)
                    });
                }),
            ));
            let readout = match n.measured_lufs {
                Some(measured) => {
                    let result = measured + n.gain_db;
                    let mut text =
                        format!("{measured:.1} → {result:.1} LUFS · {:+.1} dB", n.gain_db);
                    if (result - n.target_lufs).abs() > 0.5 {
                        text.push_str(" · held back by peaks");
                    }
                    text
                }
                None => format!("{:+.1} dB", n.gain_db),
            };
            rows.push(label_row(
                "Loudness",
                div()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT))
                    .child(readout),
            ));
        }
        Section {
            checkbox: Some(normalize.is_some()),
            on_check: Some(Box::new(|this: &mut Editor, on, cx| {
                let target = on.then_some(TARGETS[0].0);
                this.run_voice(VoiceJob::Normalize(target), cx)
            })),
            ..Section::new(NORMALIZE)
        }
        .render(self.inspector.collapsed.contains(NORMALIZE), rows, cx)
    }

    fn noise_section(&mut self, cleanup: &VoiceCleanup, cx: &mut Context<Self>) -> AnyElement {
        let denoise = cleanup.denoise.clone();
        let mut rows = Vec::new();
        if let Some(d) = &denoise {
            let selected = STRENGTHS
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 .1 - d.strength)
                        .abs()
                        .total_cmp(&(b.1 .1 - d.strength).abs())
                })
                .map(|(i, _)| i)
                .unwrap_or(1);
            let editor = cx.entity().downgrade();
            rows.push(label_row(
                "Strength",
                SegmentedTabs::new(
                    "voice-noise-strength",
                    STRENGTHS.iter().map(|(label, _)| *label),
                    selected,
                )
                .on_select(move |index, _, cx| {
                    let strength = STRENGTHS[index].1;
                    let _ = editor.update(cx, |this, cx| {
                        this.run_voice(VoiceJob::Denoise(Some(strength)), cx)
                    });
                }),
            ));
        }
        Section {
            checkbox: Some(denoise.is_some()),
            on_check: Some(Box::new(|this: &mut Editor, on, cx| {
                let strength = on.then_some(STRENGTHS[1].1);
                this.run_voice(VoiceJob::Denoise(strength), cx)
            })),
            ..Section::new(NOISE)
        }
        .render(self.inspector.collapsed.contains(NOISE), rows, cx)
    }

    fn silences_section(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(
                Button::new("voice-open-pauses")
                    .small()
                    .primary()
                    .label("Review pauses…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_silence_panel(CutMode::Pauses, window, cx)
                    })),
            )
            .child(
                Button::new("voice-open-fillers")
                    .small()
                    .outline()
                    .label("Filler words…")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_silence_panel(CutMode::Fillers, window, cx)
                    })),
            )
            .into_any_element();
        Section::new(SILENCES).render(
            self.inspector.collapsed.contains(SILENCES),
            vec![buttons],
            cx,
        )
    }

    /// Run a voice command off the UI thread, with its progress in the status
    /// line, then take the new document.
    fn run_voice(&mut self, job: VoiceJob, cx: &mut Context<Self>) {
        let Some(segment_id) = self.target_id(Prop::Volume) else {
            return;
        };
        let state = Arc::clone(&self.state);
        let progress = Arc::new(AtomicU32::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let label = job.busy();
        self.status = Some(label.into());
        cx.notify();

        {
            let (progress, done) = (Arc::clone(&progress), Arc::clone(&done));
            cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                if done.load(Ordering::Relaxed) {
                    break;
                }
                let percent = progress.load(Ordering::Relaxed);
                let alive = this.update(cx, |editor, cx| {
                    if percent > 0 {
                        editor.status = Some(format!("{label} {percent} %").into());
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            })
            .detach();
        }

        cx.spawn(async move |this, cx| {
            let worker_progress = Arc::clone(&progress);
            let result = cx
                .background_executor()
                .spawn(async move {
                    let never = AtomicBool::new(false);
                    match job {
                        VoiceJob::Normalize(target) => {
                            voice_commands::voice_normalize(&state, segment_id, target, &never)
                        }
                        VoiceJob::Denoise(strength) => voice_commands::voice_set_denoise(
                            &state,
                            segment_id,
                            strength,
                            &never,
                            &|f| worker_progress.store((f * 100.0) as u32, Ordering::Relaxed),
                        ),
                    }
                })
                .await;
            done.store(true, Ordering::Relaxed);
            let _ = this.update(cx, |editor, cx| {
                editor.refresh(cx);
                editor.report(result.map(|_| ()), cx);
            });
        })
        .detach();
    }
}
