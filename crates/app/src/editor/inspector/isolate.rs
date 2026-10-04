//! The Audio tab's "Isolate voice": the speech without music and noise, or
//! the music without the voice.
//!
//! The switch is one undo step and takes effect at once; the model run that
//! makes the isolated sound (`voice::isolate`, HTDemucs in the ML worker)
//! follows in the background, and the clip plays as it was until it is
//! ready — then the preview is re-planned and plays the isolated file. An
//! export renders whatever is still missing itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use chukcut_engine::modules::voice::commands::{self as voice_commands, IsolationSetting};
use chukcut_engine::modules::voice::isolate::Keep;
use chukcut_engine::modules::voice::VoiceCleanup;
use gpui::AnyElement;

use super::controls::*;
use super::*;
use crate::ui::SegmentedTabs;

const ISOLATE: &str = "Isolate voice";
const STRENGTHS: [(&str, f32); 3] = [("Light", 0.5), ("Medium", 0.8), ("Full", 1.0)];
const KEEPS: [(&str, Keep); 2] = [("Voice", Keep::Voice), ("Background", Keep::Background)];

/// Renders running in the background, by clip, with their progress in
/// percent (shared with the worker thread).
#[derive(Default)]
pub(crate) struct IsolatePanel {
    running: HashMap<String, Arc<AtomicU32>>,
    /// Clips whose render failed (no worker, no model): not retried on
    /// every refresh, only after their setting changes.
    failed: std::collections::HashSet<String>,
}

impl Editor {
    pub(super) fn isolate_section(
        &mut self,
        segment_id: &str,
        cleanup: &VoiceCleanup,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let isolate = cleanup.isolate.clone();
        let mut rows = Vec::new();
        if let Some(i) = &isolate {
            let keep = KEEPS.iter().position(|(_, k)| *k == i.keep).unwrap_or(0);
            let editor = cx.entity().downgrade();
            let strength = i.strength;
            rows.push(label_row(
                "Keep",
                SegmentedTabs::new("isolate-keep", KEEPS.iter().map(|(l, _)| *l), keep).on_select(
                    move |index, _, cx| {
                        let keep = KEEPS[index].1;
                        let _ = editor.update(cx, |this, cx| {
                            this.set_isolation(Some(IsolationSetting { strength, keep }), cx)
                        });
                    },
                ),
            ));
            let selected = STRENGTHS
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    (a.1 .1 - i.strength)
                        .abs()
                        .total_cmp(&(b.1 .1 - i.strength).abs())
                })
                .map(|(n, _)| n)
                .unwrap_or(2);
            let editor = cx.entity().downgrade();
            let keep = i.keep;
            rows.push(label_row(
                "Strength",
                SegmentedTabs::new(
                    "isolate-strength",
                    STRENGTHS.iter().map(|(l, _)| *l),
                    selected,
                )
                .on_select(move |index, _, cx| {
                    let strength = STRENGTHS[index].1;
                    let _ = editor.update(cx, |this, cx| {
                        this.set_isolation(Some(IsolationSetting { strength, keep }), cx)
                    });
                }),
            ));
            let note = match self.inspector.isolate.running.get(segment_id) {
                Some(p) => format!(
                    "Separating the voice\u{2026} {} %. The clip plays as it was until then.",
                    p.load(Ordering::Relaxed)
                ),
                None => "Separated by a model (HTDemucs) on this machine; an export uses the same sound."
                    .into(),
            };
            rows.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(note)
                    .into_any_element(),
            );
        }
        Section {
            checkbox: Some(isolate.is_some()),
            on_check: Some(Box::new(|this: &mut Editor, on, cx| {
                let setting = on.then_some(IsolationSetting {
                    strength: 1.0,
                    keep: Keep::Voice,
                });
                this.set_isolation(setting, cx)
            })),
            ..Section::new(ISOLATE)
        }
        .render(self.inspector.collapsed.contains(ISOLATE), rows, cx)
    }

    /// Change the setting (one undo step), then render in the background.
    fn set_isolation(&mut self, setting: Option<IsolationSetting>, cx: &mut Context<Self>) {
        let Some(segment_id) = self.target_id(Prop::Volume) else {
            return;
        };
        self.inspector.isolate.failed.remove(&segment_id);
        let result =
            voice_commands::voice_set_isolation(&self.state, segment_id, setting).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
        self.queue_missing_isolation(cx);
    }

    /// Render every clip whose isolation is set but missing (just switched
    /// on, or its cache was cleared), one background job each.
    pub(crate) fn queue_missing_isolation(&mut self, cx: &mut Context<Self>) {
        let any = self.project.materials.extras.values().any(|v| {
            v.get("voice_cleanup")
                .and_then(|c| c.get("isolate"))
                .is_some()
        });
        if !any {
            return;
        }
        for segment_id in voice_commands::voice_isolation_missing(&self.state) {
            let panel = &self.inspector.isolate;
            if panel.running.contains_key(&segment_id) || panel.failed.contains(&segment_id) {
                continue;
            }
            let progress = Arc::new(AtomicU32::new(0));
            self.inspector
                .isolate
                .running
                .insert(segment_id.clone(), Arc::clone(&progress));
            self.render_isolation(segment_id, progress, cx);
        }
    }

    fn render_isolation(
        &mut self,
        segment_id: String,
        progress: Arc<AtomicU32>,
        cx: &mut Context<Self>,
    ) {
        let state = Arc::clone(&self.state);
        let done = Arc::new(AtomicBool::new(false));
        {
            // Redraw the panel's percentage while the render runs.
            let done = Arc::clone(&done);
            cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                if done.load(Ordering::Relaxed) || this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            })
            .detach();
        }
        let id = segment_id.clone();
        cx.spawn(async move |this, cx| {
            let worker_progress = Arc::clone(&progress);
            let result = cx
                .background_executor()
                .spawn(async move {
                    let never = AtomicBool::new(false);
                    voice_commands::voice_isolation_render(&state, id, &never, &|f| {
                        worker_progress.store((f * 100.0) as u32, Ordering::Relaxed)
                    })
                })
                .await;
            done.store(true, Ordering::Relaxed);
            let _ = this.update(cx, |editor, cx| {
                editor.inspector.isolate.running.remove(&segment_id);
                let segment_id = segment_id.clone();
                // The mixers resolve the sound per project snapshot: a fresh
                // one makes the preview play the isolated file.
                editor.refresh(cx);
                match result {
                    Ok(done) => {
                        editor.status =
                            Some(format!("Voice isolated ({:.0} s)", done.seconds.max(1.0)).into());
                        cx.notify();
                    }
                    Err(error) => {
                        editor.inspector.isolate.failed.insert(segment_id);
                        editor.report(Err(format!("Isolate voice: {error}")), cx)
                    }
                }
            });
        })
        .detach();
    }
}
