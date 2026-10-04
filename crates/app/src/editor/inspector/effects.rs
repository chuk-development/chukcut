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

use crate::ui::color::{linear_to_srgb, srgb_to_linear};
use crate::ui::{self, ColorEvent, ColorPicker, EmptyState, IconButton};

use super::controls::*;
use super::*;

/// The tab's label in the inspector's top row.
pub(super) const EFFECTS: &str = "Effects";

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
    /// The colour pickers of colour parameters, keyed the same way.
    colours: HashMap<(usize, &'static str), (Entity<ColorPicker>, Subscription)>,
    drag: Option<Drag>,
    /// Whether the tab was drawn this frame and the one before; see
    /// `text_style::TextTab`, which has the same problem with popovers.
    shown: bool,
    was_shown: bool,
}

impl EffectsPanel {
    /// Called once per inspector frame, before the tabs are drawn.
    pub(super) fn begin_frame(&mut self) {
        self.was_shown = std::mem::take(&mut self.shown);
    }
}

/// An effect colour is linear light; the picker speaks sRGB.
fn to_linear(c: [f32; 4]) -> [f32; 4] {
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
        c[3],
    ]
}

fn to_srgb(c: [f32; 4]) -> [f32; 4] {
    [
        linear_to_srgb(c[0]),
        linear_to_srgb(c[1]),
        linear_to_srgb(c[2]),
        c[3],
    ]
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
        if !self.inspector.effects.was_shown {
            for (picker, _) in self.inspector.effects.colours.values() {
                picker.update(cx, |picker, _| picker.open = false);
            }
        }
        self.inspector.effects.shown = true;
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
        // "Apply to": the effects on the whole clip, its matte's subject or
        // the rest (`matte_target`).
        if !effect_clip {
            sections.extend(self.apply_to_rows(
                segment,
                chukcut_engine::modules::matting::commands::MattePart::Effects,
                cx,
            ));
        }
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
        // The state is in the id: a tooltip that is open when the button
        // flips would otherwise keep saying "Turn off" after it was turned
        // off. A new id is a new element, whose tooltip starts closed.
        let eye = IconButton::new(
            SharedString::from(format!("fx-eye-{index}-{enabled}")),
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

    pub(super) fn effect_param_row(
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
                let source = fx_edit::source_time_in(&self.project.materials, segment, playhead);
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
                let current = to_srgb(effect.color(spec.id, default));
                let picker = self.effect_colour_picker(index, spec.id, window, cx);
                picker.update(cx, |picker, _| picker.sync(current));
                label_row(
                    spec.label,
                    ui::color_button(
                        format!("fx-colour-{index}-{}", spec.id),
                        &picker,
                        None,
                        true,
                        cx,
                    ),
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
                    SliderEvent::Change(v) => this.drag_effect_param(
                        index,
                        param,
                        EffectValue::Number(v.end()),
                        false,
                        cx,
                    ),
                    SliderEvent::Release(v) => {
                        this.drag_effect_param(index, param, EffectValue::Number(v.end()), true, cx)
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

    /// The colour picker of one colour parameter, made on first use.
    fn effect_colour_picker(
        &mut self,
        index: usize,
        param: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ColorPicker> {
        let key = (index, param);
        if let Some((picker, _)) = self.inspector.effects.colours.get(&key) {
            return picker.clone();
        }
        let picker = cx.new(|cx| ColorPicker::new(window, cx));
        let subscription = cx.subscribe(&picker, move |this: &mut Editor, _, event, cx| {
            let (color, commit) = match *event {
                ColorEvent::Preview(color) => (color, false),
                ColorEvent::Commit(color) => (color, true),
            };
            this.drag_effect_param(
                index,
                param,
                EffectValue::Color(to_linear(color)),
                commit,
                cx,
            );
        });
        self.inspector
            .effects
            .colours
            .insert(key, (picker.clone(), subscription));
        picker
    }

    /// A slider or colour moved (`commit` false) or was let go (`commit`
    /// true): shown on a copy while it moves, written once when it settles.
    fn drag_effect_param(
        &mut self,
        index: usize,
        param: &'static str,
        value: EffectValue,
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
                .map(|(_, s)| fx_edit::source_time_in(&base.materials, s, playhead));
            let shown =
                fx_edit::set_param_command(&base, &segment_id, &effect_id, param, value, source)
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
            value,
            Some(playhead),
        )
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }
}
