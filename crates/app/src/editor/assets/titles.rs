//! The Text tab: "Default text", our title styles by category, and the text
//! templates. Every tile is drawn by the compositor (`text::presets`), so it
//! shows the title as it will render.
//!
//! The "+" on a tile, a click with no title selected, and a drag onto the
//! timeline all add a new title. A click while a title is selected restyles
//! that title instead, keeping its words — the way the Effects tab applies
//! an effect to the selected clip.

use chukcut_engine::modules::text::commands as text_commands;
use chukcut_engine::modules::text::presets::{StyleCategory, TextTemplate, TitleStyle};

use super::*;

/// The category column: the style groups, then the templates.
pub(super) const CATEGORIES: [&str; 6] = ["Basic", "Outline", "Box", "Glow", "Retro", "Templates"];

/// What a text tile carries while it is dragged towards the timeline.
#[derive(Clone)]
pub(crate) struct TitleDrag {
    kind: TitleKind,
    name: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TitleKind {
    Default,
    Style(&'static str),
    Template(&'static str),
}

impl Render for TitleDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(R_SM))
            .bg(rgb(OVERLAY))
            .border_1()
            .border_color(rgb(ACCENT))
            .shadow_md()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT))
            .child(self.name.clone())
    }
}

impl Editor {
    pub(super) fn render_text_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.assets.query(cx);
        let title_selected = self.selected_title_segment().is_some();
        let templates = category >= StyleCategory::ALL.len();
        let hint = match (templates, title_selected) {
            (false, true) => "Click restyles the selected title; + adds a new one.",
            (true, true) => "Click applies to the selected title; + adds a new one.",
            (_, false) => "Click to add at the playhead, or drag to the timeline.",
        };

        let mut tiles: Vec<AnyElement> = Vec::new();
        if templates {
            for template in text_commands::text_templates()
                .into_iter()
                .filter(|t| matches(t.name, &query))
            {
                tiles.push(self.template_tile(template, cx));
            }
        } else {
            let category = StyleCategory::ALL[category];
            if category == StyleCategory::Basic && matches("Default text", &query) {
                tiles.push(self.default_text_tile(cx));
            }
            for style in text_commands::text_styles()
                .into_iter()
                .filter(|s| s.category == category && matches(s.name, &query))
            {
                tiles.push(self.style_tile(style, cx));
            }
        }

        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(Self::hint(hint))
            .child(Self::tile_area("text-grid").child(Self::tile_grid(tiles)))
    }

    fn default_text_tile(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let picture = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(PANEL_RAISED))
            .text_size(px(TEXT_DISPLAY + 3.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(TEXT))
            .child("Aa")
            .into_any_element();
        self.title_tile(
            "text-default".into(),
            picture,
            "Default text".into(),
            TitleKind::Default,
            cx,
        )
    }

    fn style_tile(&mut self, style: TitleStyle, cx: &mut Context<Self>) -> AnyElement {
        let id = style.id;
        let picture = self.rendered_tile(
            format!("title:{id}"),
            move || text_commands::text_style_tile(id, super::effects::TILE_PX),
            cx,
        );
        self.title_tile(
            SharedString::from(format!("text-style-{id}")),
            picture,
            style.name.to_string(),
            TitleKind::Style(id),
            cx,
        )
    }

    fn template_tile(&mut self, template: TextTemplate, cx: &mut Context<Self>) -> AnyElement {
        let id = template.id;
        let picture = self.rendered_tile(
            format!("template:{id}"),
            move || text_commands::text_template_tile(id, super::effects::TILE_PX),
            cx,
        );
        self.title_tile(
            SharedString::from(format!("text-template-{id}")),
            picture,
            template.name.to_string(),
            TitleKind::Template(id),
            cx,
        )
    }

    fn title_tile(
        &mut self,
        id: SharedString,
        picture: AnyElement,
        name: String,
        kind: TitleKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let editor = cx.entity().downgrade();
        Self::tile(id, picture, name.clone(), false, move |_, _, cx| {
            let _ = editor.update(cx, |this, cx| this.add_title(kind, None, None, cx));
        })
        .on_click(
            cx.listener(move |this, _, _, cx| match this.selected_title_segment() {
                Some(segment_id) if kind != TitleKind::Default => {
                    this.restyle_title(kind, segment_id, cx)
                }
                _ => {
                    let at = this.clock.position();
                    this.add_title(kind, Some(at), None, cx)
                }
            }),
        )
        .on_drag(TitleDrag { kind, name }, |drag, _, _, cx| {
            cx.new(|_| drag.clone())
        })
        .into_any_element()
    }

    /// The selected clip, when it is a title (not a caption).
    fn selected_title_segment(&self) -> Option<String> {
        let (_, segment) = self.selected_segment()?;
        let material = self.project.materials.text(&segment.material_id)?;
        material.caption.is_none().then(|| segment.id.clone())
    }

    /// Add a title of `kind` at `at` (the playhead when `None`), on `lane`
    /// when that is a title lane, and select it.
    fn add_title(
        &mut self,
        kind: TitleKind,
        at: Option<Micros>,
        lane: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let at = at.unwrap_or_else(|| self.clock.position());
        let result = match kind {
            TitleKind::Default => text_commands::text_add(&self.state, at, None, None),
            TitleKind::Style(id) => text_commands::text_add_style(&self.state, at, id, None, lane),
            TitleKind::Template(id) => {
                text_commands::text_add_template(&self.state, at, id, None, lane)
            }
        };
        let result = result.map(|added| {
            self.selected = Some(added.segment_id);
            self.inspector_show_text();
        });
        self.refresh(cx);
        self.report(result, cx);
    }

    fn restyle_title(&mut self, kind: TitleKind, segment_id: String, cx: &mut Context<Self>) {
        let result = match kind {
            TitleKind::Style(id) => text_commands::text_apply_style(&self.state, &segment_id, id),
            TitleKind::Template(id) => {
                text_commands::text_apply_template(&self.state, &segment_id, id)
            }
            TitleKind::Default => return,
        }
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// A text tile dropped on the timeline: a new title where it landed.
    pub(crate) fn on_title_drop(
        &mut self,
        drag: &TitleDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((at, lane)) = self.drop_target(window.mouse_position()) else {
            return;
        };
        self.add_title(drag.kind, Some(at), lane, cx);
    }
}
