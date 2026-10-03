//! The Text tab of a selected title: its typeface, from the font picker
//! (system fonts and the library's Fontsource catalogue).
//!
//! The pick goes through `library_font_use_for_title`, one
//! `EditCommand::SetTextMaterial`, so it undoes like any edit.

use gpui::AnyElement;

use super::controls::*;
use super::*;
use crate::editor::font_picker::font_button;

pub(super) const TEXT_TAB: &str = "Text";

impl Editor {
    pub(super) fn text_style_tab(
        &mut self,
        segment: &Segment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let family = self
            .project
            .materials
            .text(&segment.material_id)
            .map(|m| m.font_family.clone())
            .unwrap_or_default();
        let picker = self.assets.library.title_fonts.clone();
        picker.update(cx, |picker, _| picker.current = family.clone());
        let label = if family.is_empty() || family == "sans-serif" {
            "Default (sans-serif)".to_string()
        } else {
            family
        };
        let rows = vec![label_row(
            "Font",
            font_button("title-font", label, &picker, cx),
        )];
        Section::new("Font").render(self.collapsed("Font"), rows, cx)
    }
}
