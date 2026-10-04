//! The inspector's Tracking tab: make the selected text, sticker or image
//! follow an object in the video under it.
//!
//! Two states. Not following yet: choose the clip to track and how to follow,
//! then "Select object" puts a box on the player. Following: the track's
//! mode, smoothing, re-track, bake and remove. A running analysis shows its
//! progress and a Cancel in place of either.

use chukcut_engine::modules::tracking::validate::link_status;
use chukcut_engine::modules::tracking::{FollowMode, TrackerKind, LOW_CONFIDENCE};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::progress::Progress;
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::{Disableable as _, Sizable as _};
use gpui::AnyElement;

use super::*;
use crate::ui::{PropertyRow, Section as KitSection, SectionHeader};

impl Editor {
    pub(super) fn tracking_tab(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body: Vec<AnyElement> = if let Some(job) = &self.tracking.job {
            let fraction = if job.total > 0 {
                job.done as f32 / job.total as f32
            } else {
                0.0
            };
            vec![
                hint(format!(
                    "Tracking… {} of {} frames",
                    job.done,
                    job.total.max(job.done)
                )),
                Progress::new("tracking-progress")
                    .value(fraction * 100.0)
                    .into_any_element(),
                actions(vec![Button::new("tracking-cancel")
                    .small()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_tracking(cx)))
                    .into_any_element()]),
            ]
        } else if self.tracking.select.is_some() {
            self.selecting_rows(cx)
        } else if self.follow_is_broken(segment) {
            self.target_missing_rows(cx)
        } else if let Some((track_id, target_id)) = self.followed(segment) {
            self.following_rows(&track_id, &target_id, window, cx)
        } else {
            self.unlinked_rows(segment, cx)
        };

        div()
            .flex()
            .flex_col()
            .px(px(PAD))
            .child(
                KitSection::new(
                    "tracking",
                    SectionHeader::new("tracking-header", "Tracking"),
                )
                .children(body),
            )
            .into_any_element()
    }

    /// Before a track exists: what to track, how to follow, and the start.
    fn unlinked_rows(&mut self, segment: &Segment, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let candidates = self.tracking_candidates(segment);
        let target = self.tracking_target(segment);
        let target_name = target
            .as_ref()
            .and_then(|id| candidates.iter().find(|(c, _)| c == id))
            .map(|(_, name)| name.clone())
            .unwrap_or_else(|| "No video under this clip".into());

        let editor = cx.entity().downgrade();
        let overlay_id = segment.id.clone();
        let menu_items = candidates.clone();
        let current = target.clone();
        let target_menu = Button::new("tracking-target")
            .small()
            .label(target_name)
            .dropdown_caret(true)
            .disabled(candidates.is_empty())
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                menu_items.iter().fold(menu, |menu, (id, name)| {
                    let editor = editor.clone();
                    let (id, overlay_id) = (id.clone(), overlay_id.clone());
                    let checked = current.as_deref() == Some(id.as_str());
                    menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click(
                        move |_, _, cx| {
                            let _ = editor.update(cx, |editor, cx| {
                                editor.tracking.target = Some((overlay_id.clone(), id.clone()));
                                cx.notify();
                            });
                        },
                    ))
                })
            });

        vec![
            hint("Draw a box around an object in the video, and this clip follows it.".to_string()),
            PropertyRow::new("tracking-target-row", "Track in")
                .no_actions()
                .child(target_menu)
                .into_any_element(),
            self.mode_row(cx),
            self.tracker_row(cx),
            actions(vec![Button::new("tracking-select")
                .small()
                .primary()
                .label("Select object")
                .disabled(target.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.begin_box_select(false, cx)))
                .into_any_element()]),
        ]
    }

    /// While the box is being drawn on the player.
    fn selecting_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let select = self
            .tracking
            .select
            .as_ref()
            .expect("checked by the caller");
        let has_box = select.rect.is_some();
        let retrack = select.retrack.is_some();
        vec![
            hint(if retrack {
                "Adjust the box on the player if the track drifted, then track again from here."
                    .to_string()
            } else {
                "Drag a box around the object on the player.".to_string()
            }),
            self.tracker_row(cx),
            actions(vec![
                Button::new("tracking-start")
                    .small()
                    .primary()
                    .label(if retrack {
                        "Re-track"
                    } else {
                        "Start tracking"
                    })
                    .disabled(!has_box)
                    .on_click(cx.listener(|this, _, _, cx| this.start_tracking(cx)))
                    .into_any_element(),
                Button::new("tracking-select-cancel")
                    .small()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_box_select(cx)))
                    .into_any_element(),
            ]),
        ]
    }

    /// Whether the clip carries a follow link with nothing left to follow
    /// (`tracking::validate::link_status`).
    fn follow_is_broken(&self, segment: &Segment) -> bool {
        self.project
            .materials
            .follow_of(segment)
            .is_some_and(|link| link_status(&self.project, link).is_broken())
    }

    /// When the tracked video (or its track) was deleted: the link stays, so
    /// undoing the delete brings the motion back, but it moves nothing.
    fn target_missing_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        vec![
            hint("Target missing".to_string()),
            hint(
                "The tracked video is no longer in the project, so this clip stays still. \
                 Undo the delete to bring the motion back, or stop following."
                    .to_string(),
            ),
            actions(vec![Button::new("tracking-detach-missing")
                .small()
                .label("Stop following")
                .on_click(cx.listener(|this, _, _, cx| this.stop_following(cx)))
                .into_any_element()]),
        ]
    }

    /// When the clip follows a track.
    fn following_rows(
        &mut self,
        track_id: &str,
        target_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(track) = self.project.materials.tracking(track_id).cloned() else {
            return Vec::new();
        };
        let target_name = self
            .project
            .segment(target_id)
            .map(|(_, s)| self.material_name(&s.material_id))
            .unwrap_or_else(|| "a clip that is gone".into());
        let frames = track.samples.len();
        let doubtful = track
            .samples
            .iter()
            .filter(|s| s.is_lost() || s.c < LOW_CONFIDENCE)
            .count();
        let made_by = TrackerKind::from_id(&track.settings.tracker).label();
        let summary = if doubtful > 0 {
            format!(
                "Follows an object in {target_name} · {frames} frames, {doubtful} doubtful · {made_by}"
            )
        } else {
            format!("Follows an object in {target_name} · {frames} frames · {made_by}")
        };

        let slider = self.smoothing_slider(window, cx);
        let smoothing = track.settings.smoothing.clamp(0.0, 1.0) * 100.0;
        if !self.tracking.smoothing_drag && (slider.read(cx).value().end() - smoothing).abs() > 0.5
        {
            slider.update(cx, |state, cx| state.set_value(smoothing, window, cx));
        }

        vec![
            hint(summary),
            self.mode_row(cx),
            self.tracker_row(cx),
            PropertyRow::new("tracking-smoothing", "Smoothing")
                .no_actions()
                .child(div().flex_1().child(Slider::new(&slider)))
                .child(
                    div()
                        .w(px(36.0))
                        .text_right()
                        .font_family(FONT_MONO)
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT))
                        .child(format!("{smoothing:.0}")),
                )
                .into_any_element(),
            actions(vec![
                Button::new("tracking-retrack")
                    .small()
                    .label("Re-track from here")
                    .on_click(cx.listener(|this, _, _, cx| this.begin_box_select(true, cx)))
                    .into_any_element(),
                Button::new("tracking-bake")
                    .small()
                    .label("Bake to keyframes")
                    .on_click(cx.listener(|this, _, _, cx| this.bake_track(cx)))
                    .into_any_element(),
            ]),
            actions(vec![
                Button::new("tracking-detach")
                    .small()
                    .label("Stop following")
                    .on_click(cx.listener(|this, _, _, cx| this.stop_following(cx)))
                    .into_any_element(),
                Button::new("tracking-remove")
                    .small()
                    .danger()
                    .label("Remove track")
                    .on_click(cx.listener(|this, _, _, cx| this.remove_track(cx)))
                    .into_any_element(),
            ]),
        ]
    }

    /// The follow mode menu. Before tracking it sets how the new track is
    /// followed; afterwards it changes the link.
    fn mode_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let current = self
            .tracking_overlay()
            .and_then(|overlay| self.project.materials.follow_of(overlay))
            .map_or(self.tracking.mode, |link| link.mode);
        let editor = cx.entity().downgrade();
        let menu = Button::new("tracking-mode")
            .small()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                FollowMode::ALL.into_iter().fold(menu, |menu, mode| {
                    let editor = editor.clone();
                    menu.item(
                        PopupMenuItem::new(mode.label())
                            .checked(mode == current)
                            .on_click(move |_, _, cx| {
                                let _ = editor
                                    .update(cx, |editor, cx| editor.set_follow_mode(mode, cx));
                            }),
                    )
                })
            });
        PropertyRow::new("tracking-mode-row", "Follow")
            .no_actions()
            .child(menu)
            .into_any_element()
    }

    /// Which tracker runs: the standard one (KLT, in the editor, no model)
    /// or VitTrack for fast motion (in the ML worker, downloaded on first
    /// use, falls back to the standard one when it cannot run).
    fn tracker_row(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.tracking.tracker;
        let editor = cx.entity().downgrade();
        let menu = Button::new("tracking-tracker")
            .small()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                [TrackerKind::Klt, TrackerKind::VitTrack]
                    .into_iter()
                    .fold(menu, |menu, tracker| {
                        let editor = editor.clone();
                        menu.item(
                            PopupMenuItem::new(tracker.label())
                                .checked(tracker == current)
                                .on_click(move |_, _, cx| {
                                    let _ = editor
                                        .update(cx, |editor, cx| editor.set_tracker(tracker, cx));
                                }),
                        )
                    })
            });
        PropertyRow::new("tracking-tracker-row", "Tracker")
            .no_actions()
            .child(menu)
            .into_any_element()
    }

    /// 0..100, committed on release as one undo step.
    fn smoothing_slider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<SliderState> {
        if let Some((slider, _)) = &self.tracking.smoothing {
            return slider.clone();
        }
        let slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(1.0)
                .default_value(0.0)
        });
        let subscription = cx.subscribe_in(
            &slider,
            window,
            |this: &mut Editor, _, event: &SliderEvent, _, cx| match event {
                SliderEvent::Change(_) => this.tracking.smoothing_drag = true,
                SliderEvent::Release(value) => {
                    this.tracking.smoothing_drag = false;
                    this.set_track_smoothing(value.end() / 100.0, cx);
                }
            },
        );
        self.tracking.smoothing = Some((slider.clone(), subscription));
        slider
    }
}

/// A line of explanation in the panel's caption style.
fn hint(text: String) -> AnyElement {
    div()
        .py(px(4.0))
        .text_size(px(TEXT_CAPTION))
        .text_color(rgb(TEXT_DIM))
        .child(text)
        .into_any_element()
}

/// A row of buttons, right-aligned like a dialog's.
fn actions(buttons: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .justify_end()
        .gap(px(8.0))
        .pt(px(4.0))
        .children(buttons)
        .into_any_element()
}
