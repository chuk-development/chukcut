//! Editing a title's or a caption's words right on its clip.
//!
//! A double-click on a text clip opens a text box over the clip, filled with
//! its words. Enter commits, Shift+Enter starts a new line, Escape cancels, and
//! a click anywhere else commits — the box loses focus, and losing focus is a
//! commit, as in the inspector's fields.
//!
//! A commit is one engine command and so one undo step: a caption goes through
//! `captions_set_text`, which keeps the word timings it can, and a title
//! through `text_set_content`. Text that did not change sends nothing, so
//! opening and closing the box leaves no empty step in the history.
//!
//! The box is an ordinary text field, so the context `!Input` keeps the
//! timeline's and the editor's plain-key shortcuts off while it has focus.

use chukcut_engine::modules::captions::commands as caption_commands;
use chukcut_engine::modules::captions::edit as caption_edit;
use chukcut_engine::modules::project::{Project, TrackKind};
use chukcut_engine::modules::text::commands as text_commands;
use gpui::component::input::{Escape, InputEvent, Textarea, TextareaState};
use gpui::component::Sizable as _;
use gpui::{Entity, Subscription};

use super::*;

/// The narrowest the box gets, so a short clip still has room to type.
const MIN_W: f32 = 200.0;

/// The open box: which clip it edits and the field itself.
pub(crate) struct InlineText {
    segment_id: String,
    input: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

/// What a commit sends to the engine.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Commit {
    /// A caption's words, through `captions_set_text`.
    Caption { segment_id: String, text: String },
    /// A title's words, through `text_set_content`.
    Title {
        material_id: String,
        content: String,
    },
}

/// The edit that committing `typed` into the clip `segment_id` makes, or
/// `None` when there is nothing to do: the clip is gone, it holds no text, the
/// text is unchanged, or it is blank. A blank title would be an invisible clip,
/// so emptying the box keeps the old words, as the Captions panel does.
pub(super) fn commit_for(project: &Project, segment_id: &str, typed: &str) -> Option<Commit> {
    let (_, segment) = project.segment(segment_id)?;
    let material = project.materials.text(&segment.material_id)?;
    let text = typed.replace("\r\n", "\n");
    if text.trim().is_empty() || text == material.content {
        return None;
    }
    Some(if caption_edit::is_caption_segment(project, segment) {
        Commit::Caption {
            segment_id: segment_id.to_string(),
            text,
        }
    } else {
        Commit::Title {
            material_id: material.id.clone(),
            content: text,
        }
    })
}

impl Editor {
    /// Whether a double-click on `segment_id` opens the box: a clip on an
    /// unlocked text lane whose material is a title or a caption.
    pub(super) fn can_edit_inline(&self, segment_id: &str) -> bool {
        self.project
            .segment(segment_id)
            .is_some_and(|(track, segment)| {
                track.kind == TrackKind::Text
                    && !track.locked
                    && self.project.materials.text(&segment.material_id).is_some()
            })
    }

    /// Open the box on `segment_id`, committing one that is already open.
    pub(super) fn open_inline_text(
        &mut self,
        segment_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_inline_text(window, cx);
        let Some(content) = self
            .project
            .segment(&segment_id)
            .and_then(|(_, s)| self.project.materials.text(&s.material_id))
            .map(|m| m.content.clone())
        else {
            return;
        };
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 4)
                .submit_on_enter(true)
                .default_value(content)
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Editor, _, event: &InputEvent, window, cx| match event {
                // Shift+Enter has already put a new line in the text.
                InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur => {
                    this.commit_inline_text(window, cx)
                }
                _ => {}
            },
        );
        input.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        self.pause();
        self.timeline.drag = None;
        self.timeline.inline_text = Some(InlineText {
            segment_id,
            input,
            _subscriptions: vec![subscription],
        });
        cx.notify();
    }

    /// Close the box and send its text to the engine, if it changed.
    pub(super) fn commit_inline_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.timeline.inline_text.take() else {
            return;
        };
        let typed = open.input.read(cx).value().to_string();
        self.close_inline_text(window, cx);
        let result = match commit_for(&self.project, &open.segment_id, &typed) {
            None => return,
            Some(Commit::Caption { segment_id, text }) => {
                caption_commands::captions_set_text(&self.state, &segment_id, &text)
            }
            Some(Commit::Title {
                material_id,
                content,
            }) => text_commands::text_set_content(&self.state, &material_id, &content),
        };
        self.refresh(cx);
        self.report(result.map(|_| ()), cx);
    }

    /// Close the box and drop what was typed.
    pub(super) fn cancel_inline_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.timeline.inline_text = None;
        self.close_inline_text(window, cx);
    }

    /// The keyboard goes back to the editor, so the shortcuts work again.
    fn close_inline_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The box, in lanes coordinates over its clip, while one is open.
    pub(super) fn render_inline_text(
        &self,
        lanes_w: f32,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let open = self.timeline.inline_text.as_ref()?;
        let (track, segment) = self.project.segment(&open.segment_id)?;
        let row = self.row_of(&track.id)?;
        let x0 = self.time_to_x(segment.target_range.start);
        let x1 = self.time_to_x(segment.target_range.end());
        let width = (x1 - x0).max(MIN_W).min(lanes_w);
        let left = x0.clamp(0.0, (lanes_w - width).max(0.0));
        Some(
            div()
                .id("timeline-inline-text")
                .absolute()
                .left(px(left))
                .top(px(row.top))
                .w(px(width))
                .min_h(px(row.height))
                .rounded(px(4.0))
                .border_1()
                .border_color(rgb(ACCENT))
                .bg(rgb(PANEL_RAISED))
                .shadow_md()
                // The lanes under the box must not see its clicks: a press
                // there would select or drag the clip underneath.
                .occlude()
                // A click anywhere else commits. Losing focus commits too,
                // but the field's Blur did not arrive for a click on the
                // lanes on Xvfb, so the click itself is the reliable signal.
                .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.commit_inline_text(window, cx)
                }))
                .on_action(
                    cx.listener(|this, _: &Escape, window, cx| this.cancel_inline_text(window, cx)),
                )
                .child(
                    Textarea::new(&open.input)
                        .appearance(false)
                        .small()
                        .text_size(px(12.0)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chukcut_engine::modules::captions::edit::PlaceOptions;
    use chukcut_engine::modules::captions::Cue;
    use chukcut_engine::modules::project::document::CanvasConfig;
    use chukcut_engine::state::AppState;

    /// A project with one title and one caption, and their segment ids.
    fn project_with_text() -> (Project, String, String) {
        let state = AppState::new();
        *state.project.write() = Some(Project::new("t", CanvasConfig::default(), 30.0));
        let title = text_commands::text_add(&state, 0, Some("Hello".into()), None)
            .expect("title added")
            .segment_id;
        let cue = Cue::new(5_000_000, 7_000_000, "two words");
        let caption = caption_commands::captions_add(&state, &[cue], None, PlaceOptions::default())
            .expect("caption added")
            .segment_ids[0]
            .clone();
        let project = state.project.read().clone().expect("open");
        (project, title, caption)
    }

    /// A title commits through the text module with its material id; a
    /// caption through the captions module with its segment id, so its word
    /// timings are kept.
    #[test]
    fn a_title_and_a_caption_commit_through_their_own_command() {
        let (project, title, caption) = project_with_text();
        let material = project.segment(&title).unwrap().1.material_id.clone();
        assert_eq!(
            commit_for(&project, &title, "Hi\r\nthere"),
            Some(Commit::Title {
                material_id: material,
                content: "Hi\nthere".into(),
            })
        );
        assert_eq!(
            commit_for(&project, &caption, "three new words"),
            Some(Commit::Caption {
                segment_id: caption.clone(),
                text: "three new words".into(),
            })
        );
    }

    /// Opening and closing the box without a change, or emptying it, makes
    /// no edit, so the history gets no empty step and no clip loses its words.
    #[test]
    fn unchanged_or_blank_text_makes_no_edit() {
        let (project, title, caption) = project_with_text();
        assert_eq!(commit_for(&project, &title, "Hello"), None);
        assert_eq!(commit_for(&project, &title, "  \n "), None);
        let words = project
            .materials
            .text(&project.segment(&caption).unwrap().1.material_id)
            .unwrap()
            .content
            .clone();
        assert_eq!(commit_for(&project, &caption, &words), None);
        assert_eq!(commit_for(&project, "no such clip", "x"), None);
    }
}
