//! "Follow a body part" in the Tracking tab: a title or sticker rides with a
//! person's hand, head, hips or another part.
//!
//! It rests on the clip's body landmarks (`modules::body`), found by a model
//! in the ML worker the first time a clip is followed. The part's pose
//! becomes an ordinary motion track, so the Tracking tab's modes, smoothing,
//! bake and detach work on it as on any other.

use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::body::commands::{self as body, FollowBody};
use chukcut_engine::modules::body::shape::BodyPart;
use chukcut_engine::shell::spawn_blocking;
use gpui::component::button::Button;
use gpui::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui::component::{Disableable as _, Sizable as _};
use gpui::AnyElement;

use super::*;
use crate::ui::PropertyRow;

/// People a follower can choose between: the analysis keeps three.
const PEOPLE: u8 = 3;

/// The panel's body state between frames.
#[derive(Default)]
pub(crate) struct BodyPanel {
    /// The part a new follower is pinned to.
    part: BodyPart,
    /// Which person, 0 the first one seen.
    person: u8,
    /// A follow running in the background (the first one analyses the clip).
    following: bool,
}

impl Editor {
    /// The Tracking tab's body rows for an overlay that follows nothing yet:
    /// which part, which person, and the button.
    pub(super) fn body_follow_rows(
        &mut self,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let current = self.inspector.body.part;
        let editor = cx.entity().downgrade();
        let part_menu = Button::new("body-part")
            .small()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                BodyPart::ALL.into_iter().fold(menu, |menu, part| {
                    let editor = editor.clone();
                    menu.item(
                        PopupMenuItem::new(part.label())
                            .checked(part == current)
                            .on_click(move |_, _, cx| {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.inspector.body.part = part;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });
        let person = self.inspector.body.person;
        let editor = cx.entity().downgrade();
        let person_menu = Button::new("body-person")
            .small()
            .label(format!("Person {}", person + 1))
            .dropdown_caret(true)
            .dropdown_menu_with_anchor(gpui::Anchor::TopRight, move |menu, _, _| {
                (0..PEOPLE).fold(menu, |menu, n| {
                    let editor = editor.clone();
                    menu.item(
                        PopupMenuItem::new(format!("Person {}", n + 1))
                            .checked(n == person)
                            .on_click(move |_, _, cx| {
                                let _ = editor.update(cx, |editor, cx| {
                                    editor.inspector.body.person = n;
                                    cx.notify();
                                });
                            }),
                    )
                })
            });
        let busy = self.inspector.body.following;
        vec![
            div()
                .pt(px(8.0))
                .text_size(px(TEXT_CAPTION))
                .text_color(rgb(TEXT_DIM))
                .child(
                    "Or follow a body part: a hand, the head, the hips. People are numbered \
                     in the order they appear; left and right are their own.",
                )
                .into_any_element(),
            PropertyRow::new("body-part-row", "Body part")
                .no_actions()
                .child(part_menu)
                .into_any_element(),
            PropertyRow::new("body-person-row", "Person")
                .no_actions()
                .child(person_menu)
                .into_any_element(),
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    Button::new("body-follow")
                        .small()
                        .label(if busy {
                            "Finding people\u{2026}"
                        } else {
                            "Follow body part"
                        })
                        .disabled(target.is_none() || busy)
                        .on_click(cx.listener(|this, _, _, cx| this.follow_body(cx))),
                )
                .into_any_element(),
        ]
    }

    fn follow_body(&mut self, cx: &mut Context<Self>) {
        let Some(overlay) = self.tracking_overlay().cloned() else {
            return;
        };
        let Some(target) = self.tracking_target(&overlay) else {
            self.report(Err("there is no video under this clip".into()), cx);
            return;
        };
        let request = FollowBody {
            overlay_id: overlay.id.clone(),
            target_segment_id: target,
            part: self.inspector.body.part,
            mode: self.tracking.mode,
            person: self.inspector.body.person,
            at: self.clock.position(),
        };
        let part = request.part.label().to_lowercase();
        let state = Arc::clone(&self.state);
        self.inspector.body.following = true;
        self.status = Some("Finding people\u{2026}".into());
        cx.notify();
        let future = spawn_blocking(move || {
            let never = AtomicBool::new(false);
            body::body_follow(&state, request, &never).map(|_| ())
        });
        cx.spawn(async move |this, cx| {
            let result = future
                .await
                .unwrap_or_else(|_| Err("following the body part stopped".into()));
            let _ = this.update(cx, |editor, cx| {
                editor.inspector.body.following = false;
                editor.refresh(cx);
                match result {
                    Ok(()) => {
                        editor.status = Some(format!("Following the {part}").into());
                        cx.notify();
                    }
                    Err(error) => editor.report(Err(error), cx),
                }
            });
        })
        .detach();
    }
}
