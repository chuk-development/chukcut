//! The Templates tab: the gallery by category, the user's own templates, and
//! for the open project its slots and "Save as template".
//!
//! Using a template makes a new project, as on the start screen: the editor
//! asks about unsaved changes first and hands the request to the shell.

use super::*;
use crate::editor::lifecycle::EditorEvent;
use crate::editor::templates::{self, FillRequest, GalleryEvent, Show};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::Sizable as _;

/// The category column: everything, the built-ins' groups, the user's own,
/// and the open project.
pub(super) const CATEGORIES: [&str; 7] = [
    "All",
    templates::CATEGORIES[0],
    templates::CATEGORIES[1],
    templates::CATEGORIES[2],
    templates::CATEGORIES[3],
    "My templates",
    "This project",
];

const MINE: usize = 5;
const THIS_PROJECT: usize = 6;

/// The gallery and the subscription that opens the fill dialog, made with
/// the panel.
pub(crate) fn gallery(
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> (Entity<templates::Gallery>, Subscription) {
    let gallery = cx.new(templates::Gallery::new);
    let subscription = cx.subscribe_in(&gallery, window, |_, _, event, window, cx| {
        let GalleryEvent::Chosen(info) = event;
        let editor_handle = cx.entity().downgrade();
        templates::open_fill(
            info.clone(),
            move |request, window, cx| {
                let _ = editor_handle.update(cx, |editor, cx| {
                    editor.use_template(request, window, cx);
                });
            },
            window,
            cx,
        );
    });
    (gallery, subscription)
}

impl Editor {
    /// Make a new project from a template: the shell's job, after the
    /// unsaved guard.
    fn use_template(&mut self, request: FillRequest, window: &mut Window, cx: &mut Context<Self>) {
        self.guard_unsaved(window, cx, move |editor, _, cx| {
            editor.pause();
            cx.emit(EditorEvent::FromTemplate(request.clone()));
        });
    }

    pub(super) fn render_templates_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if category == THIS_PROJECT {
            return self.render_this_project(cx);
        }
        let show = match category {
            0 => Show::All,
            MINE => Show::Mine,
            n => Show::Category(templates::CATEGORIES[n - 1]),
        };
        let query = self.assets.query(cx);
        let gallery = self.assets.templates.clone();
        let tiles = gallery.update(cx, |gallery, cx| {
            gallery.tiles(&show, query.as_deref(), TILE_W, cx)
        });
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(Self::hint(
                "Click a template to choose your clips; it opens as a new project.",
            ))
            .child(Self::tile_area("template-grid").child(Self::tile_grid(tiles)))
            .into_any_element()
    }

    fn render_this_project(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let slots = chukcut_engine::modules::template::slot::slots(&self.project);
        let filled = slots.iter().filter(|s| s.filled).count();
        let selection = self.selection();
        let editor = cx.entity().downgrade();
        let slots_button = {
            let editor = editor.clone();
            Button::new("template-slots")
                .label(format!(
                    "Slots ({filled} of {} filled)\u{2026}",
                    slots.len()
                ))
                .small()
                .on_click(move |_, window, cx| templates::open_slots(editor.clone(), window, cx))
        };
        let save_button = {
            let editor = editor.clone();
            let gallery = self.assets.templates.downgrade();
            let selection = self.selection_in_time_order(selection.clone());
            Button::new("template-save")
                .label("Save as template\u{2026}")
                .small()
                .primary()
                .on_click(move |_, window, cx| {
                    let gallery = gallery.clone();
                    templates::open_save(
                        editor.clone(),
                        selection.clone(),
                        move |cx| {
                            let _ = gallery.update(cx, |g, cx| g.reload(cx));
                        },
                        window,
                        cx,
                    )
                })
        };
        let explain = if selection.is_empty() && slots.is_empty() {
            "Select the clips that become slots, then save.".to_string()
        } else if selection.is_empty() {
            format!(
                "{} slot{}. Select clips to make other ones the slots.",
                slots.len(),
                if slots.len() == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "{} selected clip{} become the slots.",
                selection.len(),
                if selection.len() == 1 { "" } else { "s" }
            )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(Self::hint(explain))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .when(!slots.is_empty(), |row| row.child(slots_button))
                    .child(save_button),
            )
            .into_any_element()
    }

    /// The selected picture clips, by start time then lane: the fill order a
    /// user reading the timeline expects.
    fn selection_in_time_order(&self, selection: Vec<String>) -> Vec<String> {
        let mut placed: Vec<(Micros, usize, String)> = selection
            .into_iter()
            .filter_map(|id| {
                let lane = self
                    .project
                    .tracks
                    .iter()
                    .position(|t| t.segments.iter().any(|s| s.id == id))?;
                let (_, segment) = self.project.segment(&id)?;
                Some((segment.target_range.start, lane, id))
            })
            .collect();
        placed.sort();
        placed.into_iter().map(|(_, _, id)| id).collect()
    }
}
