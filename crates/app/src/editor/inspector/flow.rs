//! The Speed tab's frame blending: None, Frame blend and "Optical flow (AI)",
//! the "Smooth slow-mo" button, and the optical-flow bakes' progress
//! (`speed::flow` in the engine).
//!
//! A bake runs on an engine thread. The panel follows the one it started
//! for the selected clip (progress, the CPU time warning, Stop); bakes the
//! engine starts on its own after an edit (`speed_flow_queue_missing`) are
//! only followed so the preview redraws as their frames land.

use chukcut_engine::modules::speed::blend::{frame_blend_of, FrameBlend};
use chukcut_engine::modules::speed::commands as speed;
use gpui::component::button::Button;
use gpui::component::Sizable as _;
use gpui::AnyElement;

use crate::ui::SegmentedTabs;

use super::*;

/// The tab's own state, one field on the inspector.
#[derive(Default)]
pub(crate) struct FlowPanel {
    /// The bake the panel shows progress for.
    bake: Option<FlowBake>,
    /// Bakes started after an edit, with the frames each had done.
    background: Vec<(u64, u32)>,
    redrawn: Option<std::time::Instant>,
}

struct FlowBake {
    job: u64,
    segment_id: String,
    done: u32,
    total: u32,
    warning: Option<String>,
}

fn caption(text: impl Into<SharedString>, colour: u32) -> AnyElement {
    div()
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(colour))
        .child(text.into())
        .into_any_element()
}

impl Editor {
    /// The frame-blending rows of the Speed tab for a video clip; `None`
    /// for a clip without frames.
    pub(super) fn frame_blend_rows(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.project.materials.kind_of(&segment.material_id)
            != Some(chukcut_engine::modules::project::MaterialKind::Video)
        {
            return None;
        }
        let mode = frame_blend_of(&self.project.materials, segment);
        let id = segment.id.clone();
        let editor = cx.entity().downgrade();
        let tabs = SegmentedTabs::new(
            "speed-frame-blend",
            FrameBlend::ALL.iter().map(|m| match m {
                FrameBlend::None => "None",
                FrameBlend::Blend => "Blend",
                FrameBlend::Flow => "Optical flow (AI)",
            }),
            FrameBlend::ALL.iter().position(|m| *m == mode).unwrap_or(0),
        )
        .on_select(move |index, _, cx| {
            let mode = FrameBlend::ALL[index];
            let id = id.clone();
            let _ = editor.update(cx, |this, cx| this.set_frame_blend(&id, mode, cx));
        });

        let mut rows: Vec<AnyElement> = vec![
            caption("Frame blending", TEXT_DIM),
            tabs.into_any_element(),
            caption(
                match mode {
                    FrameBlend::None => "Each frame holds until the next source frame is due.",
                    FrameBlend::Blend => {
                        "Smooths slow motion: each frame mixes the two source frames around it."
                    }
                    FrameBlend::Flow => {
                        "Frames made between the source frames by RIFE (MIT, 22 MB download), \
                         on this machine. The preview blends until they are baked; an export \
                         bakes what is missing first."
                    }
                },
                TEXT_MUTED,
            ),
        ];

        match &self.inspector.flow.bake {
            Some(bake) if bake.segment_id == segment.id => {
                rows.push(caption(
                    format!(
                        "Making slow-motion frames\u{2026} {} of {}",
                        bake.done, bake.total
                    ),
                    TEXT_DIM,
                ));
                rows.push(
                    gpui::component::progress::Progress::new("flow-progress")
                        .value(bake.done as f32 / bake.total.max(1) as f32 * 100.0)
                        .into_any_element(),
                );
                if let Some(warning) = &bake.warning {
                    rows.push(caption(warning.clone(), WARNING));
                }
                let job = bake.job;
                rows.push(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .child(
                            Button::new("flow-cancel")
                                .small()
                                .label("Stop")
                                .on_click(move |_, _, _| speed::speed_flow_cancel(job)),
                        )
                        .into_any_element(),
                );
            }
            _ if mode == FrameBlend::Flow => {
                let id = segment.id.clone();
                rows.push(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .child(
                            Button::new("flow-rebake")
                                .small()
                                .label("Finish missing frames")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let result = speed::speed_flow_bake(&this.state, id.clone());
                                    this.started_flow(&id, result, cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
            _ => {
                let id = segment.id.clone();
                rows.push(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .child(caption(
                            "Slows the clip to 0.5x (unless it is slowed already) with optical flow.",
                            TEXT_MUTED,
                        ))
                        .child(
                            Button::new("smooth-slow-mo")
                                .small()
                                .label("Smooth slow-mo")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.smooth_slow_mo(&id, cx)
                                })),
                        )
                        .into_any_element(),
                );
            }
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .children(rows)
                .into_any_element(),
        )
    }

    fn set_frame_blend(&mut self, segment_id: &str, mode: FrameBlend, cx: &mut Context<Self>) {
        let result =
            speed::speed_set_frame_blend(&self.state, segment_id.to_string(), mode).map(|_| ());
        let failed = result.is_err();
        self.refresh(cx);
        self.report(result, cx);
        if !failed && mode == FrameBlend::Flow {
            let job = speed::speed_flow_bake(&self.state, segment_id.to_string());
            self.started_flow(segment_id, job, cx);
        }
    }

    fn smooth_slow_mo(&mut self, segment_id: &str, cx: &mut Context<Self>) {
        match speed::speed_smooth_slow_mo(&self.state, segment_id.to_string(), None) {
            Ok(response) => {
                self.refresh(cx);
                self.started_flow(segment_id, Ok(response.job), cx);
            }
            Err(error) => self.report(Err(error), cx),
        }
    }

    fn started_flow(
        &mut self,
        segment_id: &str,
        job: Result<Option<u64>, String>,
        cx: &mut Context<Self>,
    ) {
        match job {
            Ok(Some(job)) => {
                self.inspector.flow.background.retain(|(j, _)| *j != job);
                self.inspector.flow.bake = Some(FlowBake {
                    job,
                    segment_id: segment_id.to_string(),
                    done: 0,
                    total: 0,
                    warning: None,
                });
            }
            Ok(None) => {}
            Err(error) => self.report(Err(error), cx),
        }
        cx.notify();
    }

    /// Called from `refresh` after any edit: bake the optical-flow frames a
    /// speed change, a trim or an undo left missing. Reads files, so it
    /// runs off the UI thread, and only when a clip has optical flow on.
    pub(crate) fn queue_missing_flow(&mut self, cx: &mut Context<Self>) {
        let any = self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.segments)
            .any(|s| chukcut_engine::modules::speed::flow::is_on(&self.project.materials, s));
        if !any {
            return;
        }
        let state = Arc::clone(&self.state);
        cx.spawn(async move |this, cx| {
            let jobs = cx
                .background_executor()
                .spawn(async move { speed::speed_flow_queue_missing(&state) })
                .await;
            let _ = this.update(cx, |editor, cx| match jobs {
                Ok(jobs) => {
                    let panel = editor.inspector.flow.bake.as_ref().map(|b| b.job);
                    for job in jobs {
                        let known = &mut editor.inspector.flow.background;
                        if Some(job) != panel && !known.iter().any(|(j, _)| *j == job) {
                            known.push((job, 0));
                        }
                    }
                    cx.notify();
                }
                Err(error) => tracing::debug!(%error, "no optical-flow re-bake"),
            });
        })
        .detach();
    }

    /// Called from the editor's tick: progress of the panel's bake, and a
    /// new picture whenever frames land. Returns whether anything changed.
    pub(crate) fn poll_flow(&mut self, cx: &mut Context<Self>) -> bool {
        let mut landed = false;
        let mut ended = false;
        self.inspector.flow.background.retain_mut(|(job, done)| {
            let Some(status) = speed::speed_flow_status(*job) else {
                return false;
            };
            if let Some(Err(error)) = &status.finished {
                tracing::warn!(%error, "an optical-flow re-bake failed");
            }
            if status.finished.is_some() {
                ended = true;
                return false;
            }
            landed |= status.progress.done != *done;
            *done = status.progress.done;
            true
        });
        let mut changed = false;
        if let Some(bake) = &mut self.inspector.flow.bake {
            match speed::speed_flow_status(bake.job) {
                None => {
                    self.inspector.flow.bake = None;
                    changed = true;
                }
                Some(status) => match status.finished {
                    Some(finished) => {
                        self.inspector.flow.bake = None;
                        ended = true;
                        changed = true;
                        match finished {
                            Ok(done) if done.cancelled => {
                                self.status = Some(
                                    format!(
                                        "Stopped making slow-motion frames after {}",
                                        done.written
                                    )
                                    .into(),
                                )
                            }
                            Ok(done) => {
                                self.status = Some(
                                    format!(
                                        "Optical flow: {} frames in {:.1} s on {}",
                                        done.written,
                                        done.seconds,
                                        done.provider.as_deref().unwrap_or("the CPU")
                                    )
                                    .into(),
                                )
                            }
                            Err(error) => self.report(Err(error), cx),
                        }
                    }
                    None => {
                        landed |= status.progress.done != bake.done;
                        changed |=
                            status.progress.done != bake.done || status.warning != bake.warning;
                        bake.done = status.progress.done;
                        bake.total = status.progress.total;
                        bake.warning = status.warning;
                    }
                },
            }
        }
        let due = self
            .inspector
            .flow
            .redrawn
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(300));
        if ended || (landed && due) {
            // A new picture of the same document: frames just baked.
            self.inspector.flow.redrawn = Some(std::time::Instant::now());
            self.generation += 1;
            changed = true;
        }
        changed
    }
}
