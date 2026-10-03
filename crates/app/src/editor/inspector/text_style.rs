//! The Text tab of a selected title: its words, face, size, weight, slant,
//! underline, spacing, alignment, colour and opacity, outline, shadow,
//! background box and place on the canvas.
//!
//! Every value is a field of the title's `TextMaterial`, and every change is
//! one `EditCommand::SetTextMaterial` through `text_set` (a position, which
//! also moves the clip, is one `Composite`). Sliders and colour drags use the
//! inspector's preview: the change is shown on a copy of the document while
//! the thumb moves and written once on release, so a drag is one undo step.
//! The words are previewed as they are typed and written when the field
//! loses focus.

use std::collections::HashMap;

use chukcut_engine::modules::project::{TextAlign, TextMaterial, TextShadow};
use chukcut_engine::modules::text::commands as text_commands;
use chukcut_engine::modules::text::edit as text_edit;
use chukcut_engine::modules::text::presets::TextPosition;
use gpui::component::input::{Textarea, TextareaState};
use gpui::component::Sizable as _;
use gpui::{AnyElement, MouseDownEvent};

use super::controls::*;
use super::*;
use crate::editor::font_picker::font_button;
use crate::ui::{self, ColorEvent, ColorPicker, Glyph, IconButton};

pub(super) const TEXT_TAB: &str = "Text";

/// Which colour of a title a picker edits.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum ColorSlot {
    Fill,
    Stroke,
    Shadow,
    Box,
}

/// The tab's own widgets: the words field and one colour picker per slot.
#[derive(Default)]
pub(crate) struct TextTab {
    content: Option<Entity<TextareaState>>,
    /// The words field has been typed in since it was last written.
    content_dirty: bool,
    pickers: HashMap<ColorSlot, Entity<ColorPicker>>,
    /// Whether the tab was drawn in the frame before this one. A popover
    /// whose trigger vanished (another clip selected, the tab switched)
    /// never hears the click that closed it, so the pickers are closed by
    /// hand when the tab comes back.
    shown: bool,
    was_shown: bool,
    _subscriptions: Vec<Subscription>,
}

impl TextTab {
    /// Called once per inspector frame, before the tabs are drawn.
    pub(super) fn begin_frame(&mut self) {
        self.was_shown = std::mem::take(&mut self.shown);
    }
}

mod glyphs {
    macro_rules! glyph {
        ($name:ident, $body:literal) => {
            pub const $name: crate::ui::Glyph = crate::ui::Glyph(
                concat!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
                    $body,
                    "</svg>"
                )
                .as_bytes(),
            );
        };
    }
    glyph!(
        BOLD,
        r#"<path d="M7 5h6a3.5 3.5 0 0 1 0 7H7zM7 12h7a3.5 3.5 0 0 1 0 7H7z" stroke-width="2.4"/>"#
    );
    glyph!(ITALIC, r#"<path d="M10 5h8M6 19h8M14.5 5l-5 14"/>"#);
    glyph!(UNDERLINE, r#"<path d="M7 4v7a5 5 0 0 0 10 0V4M5 20h14"/>"#);
    glyph!(TEXT_LEFT, r#"<path d="M4 6h16M4 10h10M4 14h16M4 18h10"/>"#);
    glyph!(
        TEXT_CENTER,
        r#"<path d="M4 6h16M7 10h10M4 14h16M7 18h10"/>"#
    );
    glyph!(
        TEXT_RIGHT,
        r#"<path d="M4 6h16M10 10h10M4 14h16M10 18h10"/>"#
    );
}

/// The default line spacing, in percent of the font size: about what a
/// font's own ascent, descent and gap add up to. A title at this value has
/// no line height of its own in the document.
const LINE_DEFAULT: f32 = 120.0;

impl Prop {
    /// Whether this row edits a field of the title's material.
    pub(crate) fn is_text(self) -> bool {
        matches!(
            self,
            Prop::TextSize
                | Prop::LetterSpacing
                | Prop::LineSpacing
                | Prop::TextOpacity
                | Prop::StrokeWidth
                | Prop::ShadowX
                | Prop::ShadowY
                | Prop::ShadowBlur
                | Prop::BoxPadding
                | Prop::BoxRadius
        )
    }

    /// This row's value on `m`, in the units the panel shows.
    pub(crate) fn text_value(self, m: &TextMaterial) -> f32 {
        match self {
            Prop::TextSize => m.font_size,
            Prop::LetterSpacing => m.letter_spacing,
            Prop::LineSpacing => m.line_height.map_or(LINE_DEFAULT, |h| h * 100.0),
            Prop::TextOpacity => m.color[3] * 100.0,
            Prop::StrokeWidth => m.stroke_width,
            Prop::ShadowX => m.shadow.map_or(0.0, |s| s.offset[0]),
            Prop::ShadowY => m.shadow.map_or(0.0, |s| s.offset[1]),
            Prop::ShadowBlur => m.shadow.map_or(0.0, |s| s.blur),
            Prop::BoxPadding => m.background_padding.unwrap_or((m.font_size * 0.2).round()),
            Prop::BoxRadius => m.background_radius,
            _ => self.spec().default,
        }
    }

    /// `m` with this row set to `value`.
    pub(crate) fn with_text_value(self, m: &TextMaterial, value: f32) -> TextMaterial {
        let mut m = m.clone();
        fn shadow(m: &mut TextMaterial) -> &mut TextShadow {
            m.shadow.get_or_insert(TextShadow {
                color: [0.0, 0.0, 0.0, 0.6],
                offset: [0.0, 0.0],
                blur: 0.0,
            })
        }
        match self {
            Prop::TextSize => m.font_size = value.max(1.0),
            Prop::LetterSpacing => m.letter_spacing = value,
            // The default is "the font's own", which is what every title had
            // before the field existed; any other value is the user's.
            Prop::LineSpacing if (value - LINE_DEFAULT).abs() < 0.5 => m.line_height = None,
            Prop::LineSpacing => m.line_height = Some(value / 100.0),
            Prop::TextOpacity => m.color[3] = (value / 100.0).clamp(0.0, 1.0),
            Prop::StrokeWidth => m.stroke_width = value.max(0.0),
            Prop::ShadowX => shadow(&mut m).offset[0] = value,
            Prop::ShadowY => shadow(&mut m).offset[1] = value,
            Prop::ShadowBlur => shadow(&mut m).blur = value.max(0.0),
            Prop::BoxPadding => m.background_padding = Some(value.max(0.0)),
            Prop::BoxRadius => m.background_radius = value.max(0.0),
            _ => {}
        }
        m
    }
}

impl ColorSlot {
    fn get(self, m: &TextMaterial) -> [f32; 4] {
        match self {
            ColorSlot::Fill => [m.color[0], m.color[1], m.color[2], 1.0],
            ColorSlot::Stroke => m.stroke_color,
            ColorSlot::Shadow => m.shadow.map_or([0.0, 0.0, 0.0, 0.6], |s| s.color),
            ColorSlot::Box => m.background.unwrap_or([0.0, 0.0, 0.0, 0.6]),
        }
    }

    /// Set the colour, switching the outline, shadow or box on when it was
    /// off: picking an outline colour means wanting an outline.
    fn set(self, m: &mut TextMaterial, color: [f32; 4]) {
        match self {
            // The fill's opacity has its own row; the picker sets the hue.
            ColorSlot::Fill => m.color = [color[0], color[1], color[2], m.color[3]],
            ColorSlot::Stroke => {
                m.stroke_color = color;
                if m.stroke_width <= 0.0 {
                    m.stroke_width = default_stroke(m);
                }
            }
            ColorSlot::Shadow => {
                m.shadow = Some(TextShadow {
                    color,
                    ..default_shadow(m)
                })
            }
            ColorSlot::Box => m.background = Some(color),
        }
    }
}

fn default_stroke(m: &TextMaterial) -> f32 {
    (m.font_size * 0.06).round().max(1.0)
}

fn default_shadow(m: &TextMaterial) -> TextShadow {
    let step = (m.font_size / 12.0).round();
    TextShadow {
        color: [0.0, 0.0, 0.0, 0.6],
        offset: [step, step],
        blur: (m.font_size / 8.0).round(),
    }
}

impl Editor {
    /// Show the Text tab, after a title was added.
    pub(crate) fn inspector_show_text(&mut self) {
        self.inspector.tab = Some(TEXT_TAB);
    }

    /// The selected title's material, as drawn now.
    fn selected_title(&self) -> Option<TextMaterial> {
        let (_, segment) = self.selected_segment()?;
        self.project.materials.text(&segment.material_id).cloned()
    }

    /// Change the selected title. `key` names the change for the preview,
    /// as a slider's property does: a preview shows the change on a copy of
    /// the document, a commit writes it as one undo step.
    pub(super) fn edit_title(
        &mut self,
        key: Prop,
        phase: Phase,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut TextMaterial),
    ) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let base = match &self.inspector.preview {
            Some(preview) if preview.prop == key => Arc::clone(&preview.base),
            _ => Arc::clone(&self.project),
        };
        let Some(before) = base
            .segment(&segment_id)
            .and_then(|(_, s)| base.materials.text(&s.material_id))
            .cloned()
        else {
            return;
        };
        let mut after = before.clone();
        change(&mut after);

        if phase == Phase::Preview {
            if self.inspector.preview.as_ref().map(|p| p.prop) != Some(key) {
                self.inspector.preview = Some(Preview {
                    prop: key,
                    base: Arc::clone(&base),
                });
            }
            let mut copy = (*base).clone();
            let command = EditCommand::SetTextMaterial { before, after };
            if command.apply(&mut copy).is_ok() {
                self.project = Arc::new(copy);
                self.generation += 1;
                cx.notify();
            }
            return;
        }

        if let Some(preview) = self.inspector.preview.take() {
            self.project = preview.base;
            self.generation += 1;
        }
        let result = text_commands::text_set(&self.state, after).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// The colour picker of `slot`, made on first use.
    fn text_picker(
        &mut self,
        slot: ColorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPicker> {
        if let Some(picker) = self.inspector.text.pickers.get(&slot) {
            return picker.clone();
        }
        let alpha = matches!(slot, ColorSlot::Shadow | ColorSlot::Box | ColorSlot::Stroke);
        let picker = cx.new(|cx| ColorPicker::new(window, cx).with_alpha(alpha));
        let subscription = cx.subscribe(&picker, move |this: &mut Editor, _, event, cx| {
            let (phase, color) = match *event {
                ColorEvent::Preview(color) => (Phase::Preview, color),
                ColorEvent::Commit(color) => (Phase::Commit, color),
            };
            this.edit_title(Prop::TextColor(slot), phase, cx, |m| slot.set(m, color));
        });
        self.inspector.text._subscriptions.push(subscription);
        self.inspector.text.pickers.insert(slot, picker.clone());
        picker
    }

    fn color_row(
        &mut self,
        label: &'static str,
        slot: ColorSlot,
        material: &TextMaterial,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let picker = self.text_picker(slot, window, cx);
        let color = slot.get(material);
        picker.update(cx, |picker, _| picker.sync(color));
        label_row(
            label,
            ui::color_button(format!("text-colour-{slot:?}"), &picker, None, true, cx),
        )
    }

    /// The words field, made on first use and kept in step with the title
    /// unless it is being typed in.
    fn content_field(
        &mut self,
        material: &TextMaterial,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let input = match &self.inspector.text.content {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| {
                    TextareaState::new(window, cx)
                        .auto_grow(2, 6)
                        .default_value(material.content.clone())
                });
                let subscription = cx.subscribe_in(
                    &input,
                    window,
                    |this: &mut Editor, input, event: &InputEvent, _, cx| match event {
                        InputEvent::Change => {
                            let text = input.read(cx).value().to_string();
                            this.inspector.text.content_dirty = true;
                            this.edit_title(Prop::TextContent, Phase::Preview, cx, |m| {
                                m.content = text
                            });
                        }
                        InputEvent::Blur => this.commit_content(cx),
                        _ => {}
                    },
                );
                self.inspector.text._subscriptions.push(subscription);
                self.inspector.text.content = Some(input.clone());
                input
            }
        };
        let focused = input.read(cx).focus_handle(cx).is_focused(window);
        if !focused
            && !self.inspector.text.content_dirty
            && input.read(cx).value().as_ref() != material.content
        {
            let text = material.content.clone();
            input.update(cx, |state, cx| state.set_value(text, window, cx));
        }
        div()
            .py(px(4.0))
            // A click anywhere else writes the words: the field's Blur does
            // not always arrive on Xvfb (see `timeline/inline_text.rs`).
            .on_mouse_down_out(
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.commit_content(cx)),
            )
            .child(
                Textarea::new(&input)
                    .small()
                    .text_size(px(TEXT_BODY))
                    .bg(rgb(WELL))
                    .border_color(rgb(BORDER)),
            )
            .into_any_element()
    }

    /// Write what was typed, as one undo step.
    fn commit_content(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.inspector.text.content_dirty) {
            return;
        }
        let Some(input) = self.inspector.text.content.clone() else {
            return;
        };
        let text = input.read(cx).value().to_string();
        self.edit_title(Prop::TextContent, Phase::Commit, cx, |m| m.content = text);
    }

    /// A toggle in the style row.
    fn style_toggle(
        &self,
        id: &'static str,
        glyph: Glyph,
        tooltip: &'static str,
        on: bool,
        change: fn(&mut TextMaterial),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        IconButton::new(id, glyph)
            .toggled(on)
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_title(Prop::TextFlags, Phase::Commit, cx, change)
            }))
            .into_any_element()
    }

    fn style_row(&self, m: &TextMaterial, cx: &mut Context<Self>) -> AnyElement {
        let bold = self.style_toggle(
            "text-bold",
            glyphs::BOLD,
            "Bold",
            m.bold,
            |m| m.bold = !m.bold,
            cx,
        );
        let italic = self.style_toggle(
            "text-italic",
            glyphs::ITALIC,
            "Italic",
            m.italic,
            |m| m.italic = !m.italic,
            cx,
        );
        let underline = self.style_toggle(
            "text-underline",
            glyphs::UNDERLINE,
            "Underline",
            m.underline,
            |m| m.underline = !m.underline,
            cx,
        );
        let align = |id: &'static str, glyph, tooltip, value: TextAlign| {
            IconButton::new(id, glyph)
                .toggled(m.align == value)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, |m| m.align = value)
                }))
        };
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(bold)
            .child(italic)
            .child(underline)
            .child(div().w(px(1.0)).h(px(16.0)).mx(px(6.0)).bg(rgb(HAIRLINE)))
            .child(align(
                "text-align-left",
                glyphs::TEXT_LEFT,
                "Align left",
                TextAlign::Left,
            ))
            .child(align(
                "text-align-centre",
                glyphs::TEXT_CENTER,
                "Centre",
                TextAlign::Center,
            ))
            .child(align(
                "text-align-right",
                glyphs::TEXT_RIGHT,
                "Align right",
                TextAlign::Right,
            ));
        label_row("Style", buttons)
    }

    /// The 3 × 3 grid of places on the canvas.
    fn position_grid(
        &self,
        segment: &Segment,
        m: &TextMaterial,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let here = TextPosition::of(segment.transform.position, m.align);
        let segment_id = segment.id.clone();
        let cells = TextPosition::ALL.into_iter().map(|position| {
            let on = here == position;
            let id = segment_id.clone();
            let (column, row) = position.cell();
            div()
                .id(SharedString::from(format!("text-place-{position:?}")))
                .size(px(26.0))
                .rounded(px(R_XS))
                .border_1()
                .border_color(rgb(if on { ACCENT } else { BORDER }))
                .bg(if on { accent_soft() } else { rgb(WELL).into() })
                .hover(|s| s.border_color(rgb(BORDER_STRONG)))
                .cursor_pointer()
                .flex()
                .flex_col()
                .justify_center()
                .when(row == -1, |d| d.justify_start())
                .when(row == 1, |d| d.justify_end())
                .p(px(5.0))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .when(column == 0, |d| d.justify_center())
                        .when(column == 1, |d| d.justify_end())
                        .child(div().w(px(9.0)).h(px(3.0)).rounded(px(1.0)).bg(rgb(if on {
                            ACCENT
                        } else {
                            TEXT_DIM
                        }))),
                )
                .tooltip(move |window, cx| {
                    gpui::component::tooltip::Tooltip::new(position.label()).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let result =
                        text_commands::text_set_position(&this.state, &id, position).map(|_| ());
                    this.refresh(cx);
                    this.report(result, cx);
                }))
        });
        let grid = div()
            .w(px(26.0 * 3.0 + 8.0))
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(4.0))
            .children(cells);
        label_row("Place", grid)
    }

    pub(super) fn text_style_tab(
        &mut self,
        segment: &Segment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(m) = self.selected_title() else {
            return div().into_any_element();
        };
        if !self.inspector.text.was_shown {
            for picker in self.inspector.text.pickers.values() {
                picker.update(cx, |picker, _| picker.open = false);
            }
            self.assets
                .library
                .title_fonts
                .update(cx, |picker, _| picker.open = false);
        }
        self.inspector.text.shown = true;
        let mut sections = Vec::new();

        // --- words -----------------------------------------------------------
        let content = self.content_field(&m, window, cx);
        sections.push(Section::new("Text").render(self.collapsed("Text"), vec![content], cx));

        // --- font ------------------------------------------------------------
        let picker = self.assets.library.title_fonts.clone();
        let family = m.font_family.clone();
        picker.update(cx, |picker, _| picker.current = family.clone());
        let label = if family.is_empty() || family == "sans-serif" {
            "Default (sans-serif)".to_string()
        } else {
            family
        };
        let font_rows = vec![
            label_row("Font", font_button("title-font", label, &picker, cx)),
            self.slider_row(Prop::TextSize, segment, window, cx),
            self.style_row(&m, cx),
            self.slider_row(Prop::LetterSpacing, segment, window, cx),
            self.slider_row(Prop::LineSpacing, segment, window, cx),
        ];
        sections.push(
            Section {
                on_reset: Some(Box::new(|this: &mut Editor, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, |m| {
                        m.bold = false;
                        m.italic = false;
                        m.underline = false;
                        m.align = TextAlign::Center;
                        m.letter_spacing = 0.0;
                        m.line_height = None;
                    })
                })),
                ..Section::new("Font")
            }
            .render(self.collapsed("Font"), font_rows, cx),
        );

        // --- colour ----------------------------------------------------------
        let colour_rows = vec![
            self.color_row("Colour", ColorSlot::Fill, &m, window, cx),
            self.slider_row(Prop::TextOpacity, segment, window, cx),
        ];
        sections.push(
            Section {
                on_reset: Some(Box::new(|this: &mut Editor, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, |m| {
                        m.color = [1.0, 1.0, 1.0, 1.0]
                    })
                })),
                ..Section::new("Fill")
            }
            .render(self.collapsed("Fill"), colour_rows, cx),
        );

        // --- outline ---------------------------------------------------------
        let stroked = m.stroke_width > 0.0;
        let stroke_rows = if stroked {
            vec![
                self.color_row("Colour", ColorSlot::Stroke, &m, window, cx),
                self.slider_row(Prop::StrokeWidth, segment, window, cx),
            ]
        } else {
            Vec::new()
        };
        sections.push(
            Section {
                checkbox: Some(stroked),
                on_check: Some(Box::new(|this: &mut Editor, on: bool, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, move |m| {
                        m.stroke_width = if on { default_stroke(m) } else { 0.0 };
                    })
                })),
                on_reset: Some(Box::new(|this: &mut Editor, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, |m| {
                        m.stroke_width = default_stroke(m);
                        m.stroke_color = [0.0, 0.0, 0.0, 1.0];
                    })
                })),
                ..Section::new("Outline")
            }
            .render(self.collapsed("Outline"), stroke_rows, cx),
        );

        // --- shadow ----------------------------------------------------------
        let shadowed = m.shadow.is_some();
        let shadow_rows = if shadowed {
            vec![
                self.color_row("Colour", ColorSlot::Shadow, &m, window, cx),
                self.field_row(
                    "Offset",
                    &[(Prop::ShadowX, Some("X")), (Prop::ShadowY, Some("Y"))],
                    segment,
                    window,
                    cx,
                ),
                self.slider_row(Prop::ShadowBlur, segment, window, cx),
            ]
        } else {
            Vec::new()
        };
        sections.push(
            Section {
                checkbox: Some(shadowed),
                on_check: Some(Box::new(|this: &mut Editor, on: bool, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, move |m| {
                        m.shadow = on.then(|| default_shadow(m));
                    })
                })),
                on_reset: Some(Box::new(|this: &mut Editor, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, |m| {
                        m.shadow = Some(default_shadow(m))
                    })
                })),
                ..Section::new("Shadow")
            }
            .render(self.collapsed("Shadow"), shadow_rows, cx),
        );

        // --- background ------------------------------------------------------
        let boxed = m.background.is_some();
        let box_rows = if boxed {
            vec![
                self.color_row("Colour", ColorSlot::Box, &m, window, cx),
                self.slider_row(Prop::BoxPadding, segment, window, cx),
                self.slider_row(Prop::BoxRadius, segment, window, cx),
            ]
        } else {
            Vec::new()
        };
        sections.push(
            Section {
                checkbox: Some(boxed),
                on_check: Some(Box::new(|this: &mut Editor, on: bool, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, move |m| {
                        m.background = on.then_some([0.0, 0.0, 0.0, 0.6]);
                    })
                })),
                on_reset: Some(Box::new(|this: &mut Editor, cx| {
                    this.edit_title(Prop::TextFlags, Phase::Commit, cx, |m| {
                        m.background = Some([0.0, 0.0, 0.0, 0.6]);
                        m.background_padding = None;
                        m.background_radius = 0.0;
                    })
                })),
                ..Section::new("Background")
            }
            .render(self.collapsed("Background"), box_rows, cx),
        );

        // --- place -----------------------------------------------------------
        let place = self.position_grid(segment, &m, cx);
        sections.push(Section::new("Position").render(self.collapsed("Position"), vec![place], cx));

        div()
            .flex()
            .flex_col()
            .pb(px(PAD))
            .children(sections)
            .into_any_element()
    }

    /// Reset of a text row: the size goes back to what a new title on this
    /// canvas gets, the padding back to "a fifth of the size".
    pub(super) fn reset_text_prop(&mut self, prop: Prop, cx: &mut Context<Self>) -> bool {
        match prop {
            Prop::TextSize => {
                let size = text_edit::default_material(&self.project, None).font_size;
                self.set_prop(prop, size, Phase::Commit, cx);
                true
            }
            Prop::BoxPadding => {
                self.edit_title(Prop::BoxPadding, Phase::Commit, cx, |m| {
                    m.background_padding = None
                });
                true
            }
            _ => false,
        }
    }
}
