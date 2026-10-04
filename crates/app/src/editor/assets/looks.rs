//! The Filters tab's "Looks": our generated colour looks, installed into
//! the LUT library on first use, each tile the sample picture graded by the
//! compositor through that look's `.cube`. A click puts the look on the
//! selected clip as its LUT, keeping the rest of the grade; the inspector's
//! LUT controls then show it like any imported one.

use chukcut_engine::modules::grading::commands as grading;
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
        if let Some(presets) = self.grade_presets_section(&query, cx) {
            sections.push(presets);
        }
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
                    .child(Self::tile_grid(tiles))
                    .into_any_element(),
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

    /// "My presets": the grades saved with the Adjust tab's "Save as
    /// preset", each tile the sample picture with that grade. A click gives
    /// the selected clip the whole grade.
    fn grade_presets_section(
        &mut self,
        query: &Option<String>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let presets = self
            .assets
            .library
            .grade_presets
            .get_or_insert_with(grading::grading_presets)
            .clone();
        let mut tiles = Vec::new();
        for preset in presets.iter().filter(|p| matches(&p.name, query)) {
            // The file's time is in the key, so a preset saved again under
            // the same name gets a fresh tile.
            let stamp = std::fs::metadata(&preset.path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis());
            let name = preset.name.clone();
            let picture = self.rendered_tile(
                format!("preset:{}:{stamp}", preset.path),
                {
                    let name = name.clone();
                    move || grading::grading_preset_tile(name, super::effects::TILE_PX)
                },
                cx,
            );
            let editor = cx.entity().downgrade();
            let (add_name, click_name) = (name.clone(), name.clone());
            tiles.push(
                Self::tile(
                    SharedString::from(format!("preset-{name}")),
                    picture,
                    name.clone(),
                    false,
                    move |_, _, cx| {
                        let name = add_name.clone();
                        let _ = editor.update(cx, |this, cx| this.apply_grade_preset(name, cx));
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.apply_grade_preset(click_name.clone(), cx)
                })),
            );
        }
        if tiles.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_DIM))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("My presets"),
                )
                .child(Self::tile_grid(tiles))
                .into_any_element(),
        )
    }

    fn apply_grade_preset(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.clone() else {
            self.report(Err("Select a clip first".into()), cx);
            return;
        };
        let result = grading::grading_apply_preset(&self.state, selected, name).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
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
