//! The Effects tab of the inspector: the selected clip's effect stack, each
//! effect with its parameters.
//!
//! Every value goes through an `fx_*` engine command, so it is one undo
//! step. A slider drag previews the way the rest of the inspector does: the
//! edit is built against the document as it was when the drag started and
//! applied to a copy that the preview renders; on release the copy is dropped
//! and the edit is committed once.

use std::collections::HashMap;

use chukcut_engine::modules::fx::commands as fx_commands;
use chukcut_engine::modules::fx::{self, edit as fx_edit, ParamKind};
use chukcut_engine::modules::project::{EffectMaterial, EffectValue};
use gpui::assets::IconName;
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::{AnyElement, Subscription};

use crate::ui::{EmptyState, IconButton};

use super::controls::*;
use super::*;

/// The tab's label in the inspector's top row.
pub(super) const EFFECTS: &str = "Effects";

/// Swatches a colour parameter offers, as sRGB hex. These are values the
/// user picks for the effect, not chrome, so they are not theme tokens.
const SWATCHES: [u32; 9] = [
    0xffffff, 0x000000, 0xff3b30, 0xff9500, 0xffcc00, 0x34c759, 0x32d6ff, 0x3478f6, 0xaf52de,
];

/// What a header button does.
type Action = Box<dyn Fn(&mut Editor, &mut Context<Editor>)>;

/// One slider of one parameter of one effect in the stack.
struct ParamSlider {
    state: Entity<SliderState>,
    _subscription: Subscription,
}

/// A slider drag in progress.
struct Drag {
    index: usize,
    param: &'static str,
    base: Arc<Project>,
}

/// The Effects tab's own state, one field on the inspector.
#[derive(Default)]
pub(crate) struct EffectsPanel {
    /// By position in the stack and parameter id. Positions, not effect ids,
    /// because every edit mints a new id for the effect it changes.
    sliders: HashMap<(usize, &'static str), ParamSlider>,
    drag: Option<Drag>,
}

fn srgb_to_linear(hex: u32) -> [f32; 4] {
    let channel = |shift: u32| {
        let e = ((hex >> shift) & 0xff) as f32 / 255.0;
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(16), channel(8), channel(0), 1.0]
}

fn close_colour(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01)
}

impl Editor {
    /// Bring the inspector to the Effects tab, after an effect was added.
    pub(crate) fn inspector_show_effects(&mut self) {
        self.inspector.tab = Some(EFFECTS);
    }

    /// Where an effect sits among the clip's `extras` effects, which is the
    /// order `fx_move` speaks: an effect clip's own effect is not one of them.
    fn extras_position(&self, segment: &Segment, index: usize) -> Option<usize> {
        let offset = usize::from(self.project.materials.is_effect_clip(segment));
        index.checked_sub(offset)
    }

    pub(super) fn effects_tab(
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
        if stack.is_empty() {
            return div()
                .p(px(PAD))
                .child(
                    EmptyState::new(
                        "fx-empty",
                        crate::ui::icons::EFFECTS,
                        "No effects on this clip",
                    )
                    .hint("Pick one in the Effects tab of the asset panel."),
                )
                .into_any_element();
        }
        let effect_clip = self.project.materials.is_effect_clip(segment);
        let extras_count = stack.len() - usize::from(effect_clip);
        let mut sections = Vec::new();
        for (index, effect) in stack.iter().enumerate() {
            let own = effect_clip && index == 0;
            let position = self.extras_position(segment, index);
            sections.push(self.effect_section(
                segment,
                index,
                effect,
                own,
                position,
                extras_count,
                window,
                cx,
            ));
        }
        div()
            .flex()
            .flex_col()
            .children(sections)
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn effect_section(
        &mut self,
        segment: &Segment,
        index: usize,
        effect: &EffectMaterial,
        own: bool,
        position: Option<usize>,
        extras_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let desc = fx::descriptor(&effect.kind);
        let label = desc.map_or(effect.kind.as_str(), |d| d.label).to_string();
        let segment_id = segment.id.clone();
        let effect_id = effect.id.clone();
        let enabled = effect.enabled;

        let action = |name: &str, data: &'static [u8], on: bool, f: Action| {
            icon_button(
                SharedString::from(format!("fx-{name}-{index}")),
                data,
                TEXT_DIM,
                on,
                cx.listener(move |this, _, _, cx| f(this, cx)),
            )
        };
        let (s, e) = (segment_id.clone(), effect_id.clone());
        let move_by = move |delta: isize| {
            let (s, e) = (s.clone(), e.clone());
            Box::new(move |this: &mut Editor, cx: &mut Context<Editor>| {
                let Some(position) = position else { return };
                let to = (position as isize + delta).max(0) as usize;
                let result =
                    fx_commands::fx_move(&this.state, s.clone(), e.clone(), to).map(|_| ());
                this.refresh(cx);
                this.report(result, cx);
            }) as Action
        };
        let (s, e) = (segment_id.clone(), effect_id.clone());
        let reset = Box::new(move |this: &mut Editor, cx: &mut Context<Editor>| {
            let result = fx_commands::fx_reset(&this.state, s.clone(), e.clone()).map(|_| ());
            this.refresh(cx);
            this.report(result, cx);
        });

        let (s, e) = (segment_id.clone(), effect_id.clone());
        let eye = IconButton::new(
            SharedString::from(format!("fx-eye-{index}")),
            if enabled {
                IconName::Eye
            } else {
                IconName::EyeOff
            },
        )
        .small()
        .tint(if enabled { TEXT_DIM } else { TEXT_MUTED })
        .tooltip(if enabled { "Turn off" } else { "Turn on" })
        .on_click(cx.listener(move |this, _, _, cx| {
            let result = fx_commands::fx_set_enabled(&this.state, s.clone(), e.clone(), !enabled)
                .map(|_| ());
            this.refresh(cx);
            this.report(result, cx);
        }));
        let (s, e) = (segment_id.clone(), effect_id.clone());
        // An effect clip's own effect is the clip; it goes with the clip.
        let delete = IconButton::new(
            SharedString::from(format!("fx-delete-{index}")),
            IconName::Trash,
        )
        .small()
        .tint(TEXT_DIM)
        .disabled(own)
        .tooltip("Remove effect")
        .on_click(cx.listener(move |this, _, _, cx| {
            let result = fx_commands::fx_remove(&this.state, s.clone(), e.clone()).map(|_| ());
            this.refresh(cx);
            this.report(result, cx);
        }));

        let can_up = position.is_some_and(|p| p > 0);
        let can_down = position.is_some_and(|p| p + 1 < extras_count);
        let header = div()
            .h(px(PANEL_HEADER_H - 4.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(eye)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(TEXT_BODY))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(rgb(if enabled { TEXT } else { TEXT_DIM }))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .child(action("up", icons::UP, can_up, move_by(-1)))
            .child(action("down", icons::DOWN, can_down, move_by(1)))
            .child(action("reset", icons::RESET, desc.is_some(), reset))
            .child(delete);

        let mut rows = Vec::new();
        if let Some(desc) = desc {
            for spec in desc.params {
                rows.push(self.effect_param_row(segment, index, effect, spec, window, cx));
            }
        } else {
            rows.push(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Made by a newer version of chukcut; kept as it is.")
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .px(px(PAD))
            .pb(px(PAD))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .when(!enabled, |this| this.opacity(0.5))
                    .children(rows),
            )
            .into_any_element()
    }

    fn effect_param_row(
        &mut self,
        segment: &Segment,
        index: usize,
        effect: &EffectMaterial,
        spec: &'static fx::ParamSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let segment_id = segment.id.clone();
        let effect_id = effect.id.clone();
        let label = div()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT_DIM))
            .child(spec.label);
        match spec.kind {
            ParamKind::Number {
                min,
                max,
                step,
                default,
                unit,
            } => {
                let playhead = self.clock.position();
                let source = fx_edit::source_time(segment, playhead);
                let value = effect.number_at(spec.id, source, default);
                let slider = self.effect_slider(index, spec, min, max, step, value, window, cx);
                let keys = effect.keyframes.get(spec.id);
                let tolerance = (500_000.0 / self.project.fps.max(1.0)) as Micros;
                let at_key =
                    keys.is_some_and(|k| k.iter().any(|k| (k.time - source).abs() <= tolerance));
                let animated = keys.is_some_and(|k| !k.is_empty());
                let decimals = if step < 1.0 || (max - min) <= 10.0 {
                    1
                } else {
                    0
                };
                let (s, e) = (segment_id.clone(), effect_id.clone());
                let param = spec.id;
                let reset = icon_button(
                    SharedString::from(format!("fx-reset-{index}-{param}")),
                    icons::RESET,
                    TEXT_DIM,
                    true,
                    cx.listener(move |this, _, _, cx| {
                        let result = fx_commands::fx_set_param(
                            &this.state,
                            s.clone(),
                            e.clone(),
                            param.to_string(),
                            EffectValue::Number(default),
                            Some(this.clock.position()),
                        )
                        .map(|_| ());
                        this.refresh(cx);
                        this.report(result, cx);
                    }),
                );
                let (s, e) = (segment_id.clone(), effect_id.clone());
                let diamond = icon_button(
                    SharedString::from(format!("fx-kf-{index}-{param}")),
                    if at_key {
                        icons::DIAMOND_FILLED
                    } else {
                        icons::DIAMOND
                    },
                    if at_key || animated { ACCENT } else { TEXT_DIM },
                    true,
                    cx.listener(move |this, _, _, cx| {
                        let result = fx_commands::fx_toggle_keyframe(
                            &this.state,
                            s.clone(),
                            e.clone(),
                            param.to_string(),
                            this.clock.position(),
                        )
                        .map(|_| ());
                        this.refresh(cx);
                        this.report(result, cx);
                    }),
                );
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(label)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .child(div().flex_1().px(px(4.0)).child(Slider::new(&slider)))
                            .child(
                                div()
                                    .w(px(56.0))
                                    .text_right()
                                    .font_family(FONT_MONO)
                                    .text_size(px(TEXT_LABEL))
                                    .text_color(rgb(TEXT))
                                    .child(format!("{value:.decimals$}{unit}")),
                            )
                            .child(reset)
                            .child(diamond),
                    )
                    .into_any_element()
            }
            ParamKind::Choice { options, .. } => {
                let current = effect
                    .stored(spec.id)
                    .and_then(|v| v.number())
                    .unwrap_or(spec.default_number())
                    .round() as usize;
                let pills = options.iter().enumerate().map(|(i, option)| {
                    let (s, e) = (segment_id.clone(), effect_id.clone());
                    let param = spec.id;
                    let selected = i == current;
                    div()
                        .id(SharedString::from(format!("fx-choice-{index}-{param}-{i}")))
                        .px(px(8.0))
                        .h(px(CONTROL_H - 2.0))
                        .flex()
                        .items_center()
                        .rounded(px(R_SM))
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(if selected { ACCENT } else { TEXT_DIM }))
                        .border_1()
                        .border_color(rgb(if selected { ACCENT } else { BORDER }))
                        .bg(if selected {
                            accent_soft()
                        } else {
                            rgb(WELL).into()
                        })
                        .cursor_pointer()
                        .hover(|style| style.text_color(rgb(TEXT)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = fx_commands::fx_set_param(
                                &this.state,
                                s.clone(),
                                e.clone(),
                                param.to_string(),
                                EffectValue::Number(i as f32),
                                None,
                            )
                            .map(|_| ());
                            this.refresh(cx);
                            this.report(result, cx);
                        }))
                        .child(*option)
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(label)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(4.0))
                            .children(pills),
                    )
                    .into_any_element()
            }
            ParamKind::Color { default } => {
                let current = effect.color(spec.id, default);
                let swatches = SWATCHES.iter().map(|&hex| {
                    let (s, e) = (segment_id.clone(), effect_id.clone());
                    let param = spec.id;
                    let colour = srgb_to_linear(hex);
                    let selected = close_colour(current, colour);
                    div()
                        .id(SharedString::from(format!(
                            "fx-colour-{index}-{param}-{hex:06x}"
                        )))
                        .size(px(18.0))
                        .rounded(px(R_XS))
                        .bg(rgb(hex))
                        .border_2()
                        .border_color(rgb(if selected { ACCENT } else { BORDER }))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let result = fx_commands::fx_set_param(
                                &this.state,
                                s.clone(),
                                e.clone(),
                                param.to_string(),
                                EffectValue::Color(colour),
                                None,
                            )
                            .map(|_| ());
                            this.refresh(cx);
                            this.report(result, cx);
                        }))
                });
                label_row(
                    spec.label,
                    div().flex().flex_row().gap(px(4.0)).children(swatches),
                )
            }
        }
    }

    /// The slider of one numeric parameter, made on first use and kept in
    /// step with the document unless it is being dragged.
    #[allow(clippy::too_many_arguments)]
    fn effect_slider(
        &mut self,
        index: usize,
        spec: &'static fx::ParamSpec,
        min: f32,
        max: f32,
        step: f32,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        let key = (index, spec.id);
        if let std::collections::hash_map::Entry::Vacant(slot) =
            self.inspector.effects.sliders.entry(key)
        {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(min)
                    .max(max)
                    .step(step.min((max - min) / 100.0).max(0.01))
                    .default_value(value)
            });
            let param = spec.id;
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this: &mut Editor, _, event: &SliderEvent, _, cx| match event {
                    SliderEvent::Change(v) => {
                        this.drag_effect_param(index, param, v.end(), false, cx)
                    }
                    SliderEvent::Release(v) => {
                        this.drag_effect_param(index, param, v.end(), true, cx)
                    }
                },
            );
            slot.insert(ParamSlider {
                state,
                _subscription: subscription,
            });
        }
        let state = self.inspector.effects.sliders[&key].state.clone();
        let dragging = self
            .inspector
            .effects
            .drag
            .as_ref()
            .is_some_and(|d| d.index == index && d.param == spec.id);
        if !dragging && (state.read(cx).value().end() - value).abs() > 1e-4 {
            state.update(cx, |s, cx| s.set_value(value, window, cx));
        }
        state
    }

    /// A slider moved (`commit` false) or was let go (`commit` true).
    fn drag_effect_param(
        &mut self,
        index: usize,
        param: &'static str,
        value: f32,
        commit: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(segment_id) = self.selected.clone() else {
            return;
        };
        let base = match &self.inspector.effects.drag {
            Some(drag) if drag.index == index && drag.param == param => Arc::clone(&drag.base),
            _ => {
                let base = Arc::clone(&self.project);
                self.inspector.effects.drag = Some(Drag {
                    index,
                    param,
                    base: Arc::clone(&base),
                });
                base
            }
        };
        let Some(effect_id) = base.segment(&segment_id).and_then(|(_, s)| {
            base.materials
                .effects_of(s)
                .get(index)
                .map(|e| e.id.clone())
        }) else {
            return;
        };
        let playhead = self.clock.position();
        if !commit {
            let source = base
                .segment(&segment_id)
                .map(|(_, s)| fx_edit::source_time(s, playhead));
            let shown = fx_edit::set_param_command(
                &base,
                &segment_id,
                &effect_id,
                param,
                EffectValue::Number(value),
                source,
            )
            .and_then(|(material, command)| {
                let mut copy = (*base).clone();
                copy.materials.effects.push(material);
                command.apply(&mut copy)?;
                Ok(copy)
            });
            if let Ok(project) = shown {
                self.project = Arc::new(project);
                self.generation += 1;
                self.audio.set_project(Arc::clone(&self.project));
                cx.notify();
            }
            return;
        }
        // Commit against the document as it is, never the preview copy.
        self.inspector.effects.drag = None;
        self.project = base;
        self.generation += 1;
        let result = fx_commands::fx_set_param(
            &self.state,
            segment_id,
            effect_id,
            param.to_string(),
            EffectValue::Number(value),
            Some(playhead),
        )
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }
}
