//! The Filters tab's "Looks": our generated colour looks, installed into
//! the LUT library on first use, each tile the sample picture graded by the
//! compositor through that look's `.cube`. A click puts the look on the
//! selected clip as its LUT, keeping the rest of the grade; the inspector's
//! LUT controls then show it like any imported one.

use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::library::commands as library;
use chukcut_engine::modules::library::looks::CATEGORIES;
use chukcut_engine::modules::project::LutRef;

use super::library_panel::notice;
use super::*;

/// The Filters tab's category column.
pub(super) const FILTER_CATEGORIES: [&str; 2] = ["Looks", "Basic"];

impl Editor {
    pub(super) fn render_looks_tab(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.load_looks(cx);
        let hint = if self.selected.is_some() {
            "Click a look to grade the selected clip."
        } else {
            "Select a clip on the timeline, then pick a look."
        };
        let looks = match &self.assets.library.looks {
            None => {
                return div()
                    .child(notice("Preparing the looks\u{2026}", TEXT_MUTED))
                    .into_any_element()
            }
            Some(Err(error)) => {
                return div()
                    .child(notice(error.clone(), DANGER))
                    .into_any_element()
            }
            Some(Ok(looks)) => looks.clone(),
        };
        let query = self.assets.query(cx);
        let current = self
            .selected_segment()
            .and_then(|(_, s)| self.project.materials.color_adjust_of(s))
            .and_then(|c| c.lut.as_ref().map(|l| l.path.clone()));
        let mut sections = Vec::new();
        for category in CATEGORIES {
            let mut tiles = Vec::new();
            for look in looks
                .iter()
                .filter(|l| l.category == category && matches(&l.name, &query))
            {
                let path = look.path.clone();
                let picture = self.rendered_tile(
                    format!("look:{path}"),
                    {
                        let path = path.clone();
                        move || library::library_look_tile(&path, super::effects::TILE_PX)
                    },
                    cx,
                );
                let editor = cx.entity().downgrade();
                let (add_path, click_path) = (path.clone(), path.clone());
                tiles.push(
                    Self::tile(
                        SharedString::from(format!("look-{}", look.name)),
                        picture,
                        look.name.clone(),
                        current.as_deref() == Some(path.as_str()),
                        move |_, _, cx| {
                            let path = add_path.clone();
                            let _ = editor.update(cx, |this, cx| this.apply_look(path, cx));
                        },
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.apply_look(click_path.clone(), cx)),
                    ),
                );
            }
            if tiles.is_empty() {
                continue;
            }
            sections.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT_DIM))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(category),
                    )
                    .child(Self::tile_grid(tiles)),
            );
        }
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(Self::hint(hint))
            .child(
                Self::tile_area("looks-grid")
                    .child(div().flex().flex_col().gap(px(16.0)).children(sections)),
            )
            .into_any_element()
    }

    fn load_looks(&mut self, cx: &mut Context<Self>) {
        let library = &mut self.assets.library;
        if library.looks.is_some() || library.looks_loading {
            return;
        }
        library.looks_loading = true;
        self.library_task(
            cx,
            || library::library_looks_install().map(|_| library::library_looks()),
            |editor, result, _| {
                editor.assets.library.looks_loading = false;
                editor.assets.library.looks = Some(result);
            },
        );
    }

    /// Put the look at `path` on the selected clip, or take it off when it
    /// is already there.
    fn apply_look(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.clone() else {
            self.report(Err("Select a clip first".into()), cx);
            return;
        };
        let already = self
            .project
            .segment(&selected)
            .and_then(|(_, s)| self.project.materials.color_adjust_of(s))
            .and_then(|c| c.lut.as_ref())
            .is_some_and(|l| l.path == path);
        let lut = (!already).then_some(LutRef {
            path,
            intensity: 1.0,
        });
        let result = inspector_commands::inspector_set_lut(&self.state, selected, lut).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }
}
