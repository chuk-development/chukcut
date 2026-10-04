//! Faces in the inspector: Video › Retouch, and "Follow a face" in the
//! Tracking tab.
//!
//! Both rest on the clip's face landmarks (`modules::landmarks`), found by a
//! background analysis that the panel starts and follows like the others in
//! the status line. Retouch is an ordinary effect (`fx::catalog::RETOUCH`):
//! the presets set its five values in one undo step, and its sliders are the
//! effect panel's own rows. Until the faces are found the clip is drawn as
//! it is; the preview picks the faces up as the analysis writes them.

use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::analysis::commands as analysis;
use chukcut_engine::modules::analysis::jobs::JobKind;
use chukcut_engine::modules::fx;
use chukcut_engine::modules::landmarks::commands::{self as landmarks, FollowFace, RetouchSetting};
use chukcut_engine::modules::landmarks::shape::Anchor;
use chukcut_engine::modules::project::EffectMaterial;
use chukcut_engine::shell::spawn_blocking;
use gpui::component::button::Button;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Disableable as _, Sizable as _};
use gpui::AnyElement;

use super::controls::*;
use super::*;
use crate::ui::{PropertyRow, SegmentedTabs};

const RETOUCH: &str = "Retouch";

/// The panel's face state between frames.
#[derive(Default)]
pub(crate) struct FacePanel {
    /// Where a new face-follow is pinned.
    anchor: Anchor,
    /// A face-follow running in the background.
    following: bool,
    /// When the preview was last redrawn for an analysis in progress.
    redrawn: Option<std::time::Instant>,
}

impl Editor {
    // --- Retouch ------------------------------------------------------------------

    pub(super) fn retouch_tab(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stack: Vec<EffectMaterial> = self
            .project
            .materials
            .effects_of(segment)
            .into_iter()
            .cloned()
            .collect();
        let found = stack
            .iter()
            .enumerate()
            .find(|(_, e)| e.kind == fx::catalog::RETOUCH);
        let mut rows = Vec::new();
        rows.push(
            div()
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_MUTED))
                .child(self.face_status(&segment.id))
                .into_any_element(),
        );
        if let Some((index, effect)) = found {
            let current = RetouchSetting::of(effect);
            let selected = fx::retouch::PRESETS
                .iter()
                .position(|(_, [s, e, t, l])| {
                    [*s, *e, *t, *l] == [current.smooth, current.eyes, current.teeth, current.slim]
                })
                // No preset matches: none is lit.
                .unwrap_or(usize::MAX);
            let editor = cx.entity().downgrade();
            let strength = current.strength;
            rows.push(label_row(
                "Look",
                div().w(px(260.0)).child(
                    SegmentedTabs::new(
                        "retouch-preset",
                        fx::retouch::PRESETS.iter().map(|(name, _)| *name),
                        selected,
                    )
                    .on_select(move |i, _, cx| {
                        let name = fx::retouch::PRESETS[i].0;
                        let _ = editor.update(cx, |this, cx| {
                            let setting = RetouchSetting::preset(name)
                                .map(|s| RetouchSetting { strength, ..s });
                            this.set_retouch(setting, cx)
                        });
                    }),
                ),
            ));
            if let Some(desc) = fx::descriptor(fx::catalog::RETOUCH) {
                for spec in desc.params {
                    rows.push(self.effect_param_row(segment, index, effect, spec, window, cx));
                }
            }
        }
        let on = found.is_some();
        div()
            .flex()
            .flex_col()
            .child(
                Section {
                    checkbox: Some(on),
                    on_check: Some(Box::new(|this: &mut Editor, on, cx| {
                        this.set_retouch(
                            on.then(|| RetouchSetting::preset("natural")).flatten(),
                            cx,
                        )
                    })),
                    ..Section::new(RETOUCH)
                }
                .render(self.inspector.collapsed.contains(RETOUCH), rows, cx),
            )
            .into_any_element()
    }

    /// What the panel says about the clip's faces: being found, found, or
    /// not yet.
    fn face_status(&self, segment_id: &str) -> String {
        if let Some(job) = analysis::analysis_jobs().into_iter().find(|j| {
            j.kind == JobKind::Landmarks && j.segment_id == segment_id && j.finished.is_none()
        }) {
            return format!(
                "Finding the faces\u{2026} {:.0} %. The clip shows unretouched until then.",
                job.fraction * 100.0
            );
        }
        match landmarks::landmarks_coverage(&self.state, segment_id.to_string()) {
            Ok(c) if c.analysed > 0 && c.done() => {
                "Faces found by a model (MediaPipe face mesh) on this machine.".into()
            }
            Ok(_) => "The faces are found when retouch is switched on.".into(),
            Err(error) => error,
        }
    }

    fn set_retouch(&mut self, setting: Option<RetouchSetting>, cx: &mut Context<Self>) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let result = landmarks::landmarks_set_retouch(&self.state, segment_id, setting).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// Start finding faces for every clip that needs them (a retouch added,
    /// a clip made longer, a cleared cache), and follow the jobs in the
    /// status line.
    pub(crate) fn queue_missing_landmarks(&mut self, cx: &mut Context<Self>) {
        let any = self
            .project
            .materials
            .effects
            .iter()
            .any(|e| e.kind == fx::catalog::RETOUCH);
        if !any {
            return;
        }
        match landmarks::landmarks_queue_missing(&self.state) {
            Ok(jobs) => {
                for job in jobs {
                    self.follow_analysis_job(job);
                }
                cx.notify();
            }
            Err(error) => tracing::debug!(%error, "no landmark analysis"),
        }
    }

    /// While faces are being found, redraw the preview now and then, so the
    /// retouch appears as the frames land. Returns whether it redrew.
    pub(crate) fn poll_landmarks(&mut self) -> bool {
        let running = analysis::analysis_jobs()
            .into_iter()
            .any(|j| j.kind == JobKind::Landmarks && j.finished.is_none());
        if !running {
            return false;
        }
        let due = self
            .inspector
            .face
            .redrawn
            .is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(500));
        if due {
            self.inspector.face.redrawn = Some(std::time::Instant::now());
            self.generation += 1;
        }
        due
    }

    // --- Follow a face --------------------------------------------------------------

    /// The Tracking tab's face rows for an overlay that follows nothing yet:
    /// where on the face, and the button.
    pub(super) fn face_follow_rows(
        &mut self,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let current = self.inspector.face.anchor;
        let editor = cx.entity().downgrade();
        let menu = Button::new("face-anchor")
            .small()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                Anchor::ALL.into_iter().fold(menu, |menu, anchor| {
                    let editor = editor.clone();
                    menu.item(
                        PopupMenuItem::new(anchor.label())
                            .checked(anchor == current)
                            .on_click(move |_, _, cx| {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.inspector.face.anchor = anchor;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });
        let busy = self.inspector.face.following;
        vec![
            div()
                .pt(px(8.0))
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_DIM))
                .child("Or follow a face: the clip rides with the person, found by a model.")
                .into_any_element(),
            PropertyRow::new("face-anchor-row", "Pin to")
                .no_actions()
                .child(menu)
                .into_any_element(),
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    Button::new("face-follow")
                        .small()
                        .label(if busy {
                            "Finding the face\u{2026}"
                        } else {
                            "Follow face"
                        })
                        .disabled(target.is_none() || busy)
                        .on_click(cx.listener(|this, _, _, cx| this.follow_face(cx))),
                )
                .into_any_element(),
        ]
    }

    fn follow_face(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let Some(target) = self.tracking_target(&overlay) else {
            self.report(Err("there is no video under this clip".into()), cx);
            return;
        };
        let request = FollowFace {
            overlay_id: overlay.id.clone(),
            target_segment_id: target,
            anchor: self.inspector.face.anchor,
            mode: self.tracking.mode,
            face: 0,
            at: self.clock.position(),
        };
        let state = Arc::clone(&self.state);
        self.inspector.face.following = true;
        self.status = Some("Finding the face\u{2026}".into());
        cx.notify();
        let future = spawn_blocking(move || {
            let never = AtomicBool::new(false);
            landmarks::landmarks_follow_face(&state, request, &never).map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = future
                .await
                .unwrap_or_else(|_| Err("following the face stopped".into()));
            let _ = this.update(cx, |editor, cx| {
                editor.inspector.face.following = false;
                editor.refresh(cx);
                match result {
                    Ok(()) => {
                        editor.status = Some("Following the face".into());
                        cx.notify();
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            });
        })
        .detach();
    }
}
