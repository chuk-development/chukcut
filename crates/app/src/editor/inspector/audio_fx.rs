//! The audio tools in the inspector: the effect sections of the Audio tab
//! (Equalizer, Parametric EQ, Compressor, Reverb, Echo, Pitch), the Voice
//! changer grid, and the Speed tab's "Change audio pitch" switch.
//!
//! Every change is an `audiofx_*` engine command on the clip that is heard,
//! so it is one undo step. A slider writes when it is let go: the sound of a
//! changed effect is a render of the whole clip (`audiofx::cache`), so a value
//! per pixel of drag would only queue renders nobody hears. The readout moves
//! with the thumb.

use std::collections::HashMap;

use chukcut_engine::modules::audiofx::commands as fx_commands;
use chukcut_engine::modules::audiofx::{
    catalog as fx_catalog, descriptor, fx_or_default, AudioFx, ParamSpec,
};
use chukcut_engine::state::AppState;
use gpui::component::slider::{Slider, SliderEvent, SliderState};
use gpui::component::switch::Switch;
use gpui::component::Disableable as _;
use gpui::{AnyElement, Subscription};

use super::controls::*;
use super::*;

/// The effect sections, in the order the Audio tab shows them.
const SECTIONS: [(&str, &str); 6] = [
    (fx_catalog::EQ3, "Equalizer"),
    (fx_catalog::EQ5, "Parametric EQ"),
    (fx_catalog::COMPRESSOR, "Compressor"),
    (fx_catalog::REVERB, "Reverb"),
    (fx_catalog::DELAY, "Echo"),
    (fx_catalog::PITCH, "Pitch"),
];

struct ParamSlider {
    state: Entity<SliderState>,
    _subscription: Subscription,
}

/// The audio tools' own state, one field on the inspector.
#[derive(Default)]
pub(crate) struct AudioFxPanel {
    /// By effect kind and parameter id: one section per kind.
    sliders: HashMap<(&'static str, &'static str), ParamSlider>,
    /// The slider being dragged, whose own position is shown, not the
    /// document's.
    dragging: Option<(&'static str, &'static str)>,
}

/// A parameter's value as a slider position, `0..=1` on a log scale for
/// frequencies and times, so 100 Hz and 10 kHz are equally easy to reach.
fn to_slider(spec: &ParamSpec, value: f32) -> f32 {
    if spec.log {
        (value.max(spec.min).ln() - spec.min.ln()) / (spec.max.ln() - spec.min.ln())
    } else {
        (value - spec.min) / (spec.max - spec.min)
    }
}

fn from_slider(spec: &ParamSpec, position: f32) -> f32 {
    let t = position.clamp(0.0, 1.0);
    if spec.log {
        (spec.min.ln() + t * (spec.max.ln() - spec.min.ln())).exp()
    } else {
        spec.min + t * (spec.max - spec.min)
    }
}

/// A value in its unit, as the readout shows it.
fn readout(spec: &ParamSpec, value: f32) -> String {
    match spec.unit {
        "dB" | "st" => format!("{value:+.1} {}", spec.unit),
        "Hz" if value >= 1_000.0 => format!("{:.1} kHz", value / 1_000.0),
        "Hz" => format!("{value:.0} Hz"),
        "ms" if value < 10.0 => format!("{value:.1} ms"),
        "ms" => format!("{value:.0} ms"),
        "%" => format!("{:.0} %", value * 100.0),
        "x" => format!("{value:.1}:1"),
        "Q" => format!("{value:.2}"),
        _ => format!("{value:.2}"),
    }
}

impl Editor {
    /// The heard clip's audio settings and its id.
    fn audio_fx_target(&self) -> Option<(String, AudioFx)> {
        let segment = self.target_segment(Prop::Volume)?;
        Some((segment.id.clone(), fx_or_default(&self.project, segment)))
    }

    fn run_audio_fx(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&Arc<AppState>, String) -> Result<(), String>,
    ) {
        let Some((segment_id, _)) = self.audio_fx_target() else {
            return;
        };
        let result = f(&self.state, segment_id);
        self.refresh(cx);
        self.report(result, cx);
    }

    /// The Audio tab's effect sections, for the heard clip.
    pub(super) fn audio_fx_sections(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some((_, fx)) = self.audio_fx_target() else {
            return Vec::new();
        };
        SECTIONS
            .iter()
            .map(|(kind, title)| self.audio_effect_section(&fx, kind, title, window, cx))
            .collect()
    }

    fn audio_effect_section(
        &mut self,
        fx: &AudioFx,
        kind: &'static str,
        title: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let effect = fx.effects.iter().find(|e| e.kind == kind).cloned();
        let on = effect.as_ref().is_some_and(|e| e.enabled);
        let mut rows = Vec::new();
        if let (Some(effect), Some(desc)) = (&effect, descriptor(kind)) {
            for spec in desc.params {
                let value = effect.value(spec.id).unwrap_or(spec.default);
                rows.push(self.audio_param_row(kind, &effect.id, spec, value, window, cx));
            }
        }
        let present = effect.as_ref().map(|e| e.id.clone());
        let removable = present.clone();
        Section {
            checkbox: Some(on),
            // Off keeps the settings (the effect is switched off in the
            // stack); the reset arrow takes it away entirely.
            on_check: Some(Box::new(move |this: &mut Editor, checked, cx| {
                let present = present.clone();
                this.run_audio_fx(cx, |state, segment| match present {
                    Some(effect) => {
                        fx_commands::audiofx_set_enabled(state, segment, effect, checked)
                            .map(|_| ())
                    }
                    None if checked => {
                        fx_commands::audiofx_add(state, segment, kind.into()).map(|_| ())
                    }
                    None => Ok(()),
                })
            })),
            on_reset: removable.map(|effect| {
                Box::new(move |this: &mut Editor, cx: &mut Context<Editor>| {
                    let effect = effect.clone();
                    this.run_audio_fx(cx, |state, segment| {
                        fx_commands::audiofx_remove(state, segment, effect).map(|_| ())
                    })
                }) as Box<dyn Fn(&mut Editor, &mut Context<Editor>)>
            }),
            ..Section::new(title)
        }
        .render(self.inspector.collapsed.contains(title), rows, cx)
    }

    fn audio_param_row(
        &mut self,
        kind: &'static str,
        effect_id: &str,
        spec: &'static ParamSpec,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A 0/1 parameter with no unit is a switch ("Keep voice character").
        if spec.unit.is_empty() && spec.min == 0.0 && spec.max == 1.0 {
            let effect = effect_id.to_string();
            let param = spec.id;
            let entity = cx.entity().downgrade();
            return label_row(
                spec.label,
                Switch::new(SharedString::from(format!("afx-{kind}-{param}")))
                    .checked(value >= 0.5)
                    .on_click(move |checked, _, cx| {
                        let (effect, checked) = (effect.clone(), *checked);
                        let _ = entity.update(cx, |this, cx| {
                            this.run_audio_fx(cx, |state, segment| {
                                fx_commands::audiofx_set_param(
                                    state,
                                    segment,
                                    effect,
                                    param.into(),
                                    if checked { 1.0 } else { 0.0 },
                                )
                                .map(|_| ())
                            })
                        });
                    }),
            );
        }
        let slider = self.audio_slider(kind, spec, value, window, cx);
        let dragging = self.inspector.audio_fx.dragging == Some((kind, spec.id));
        let shown = if dragging {
            from_slider(spec, slider.read(cx).value().end())
        } else {
            value
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .child(spec.label),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.0))
                    .child(div().flex_1().px(px(4.0)).child(Slider::new(&slider)))
                    .child(
                        div()
                            .w(px(72.0))
                            .text_right()
                            .font_family(FONT_MONO)
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT))
                            .child(readout(spec, shown)),
                    ),
            )
            .into_any_element()
    }

    /// The slider of one parameter, made on first use and kept in step with
    /// the document unless it is being dragged.
    fn audio_slider(
        &mut self,
        kind: &'static str,
        spec: &'static ParamSpec,
        value: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SliderState> {
        let key = (kind, spec.id);
        if let std::collections::hash_map::Entry::Vacant(slot) =
            self.inspector.audio_fx.sliders.entry(key)
        {
            let state = cx.new(|_| {
                SliderState::new()
                    .min(0.0)
                    .max(1.0)
                    .step(0.001)
                    .default_value(to_slider(spec, value))
            });
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this: &mut Editor, _, event: &SliderEvent, _, cx| match event {
                    SliderEvent::Change(_) => {
                        this.inspector.audio_fx.dragging = Some(key);
                        cx.notify();
                    }
                    SliderEvent::Release(v) => {
                        this.inspector.audio_fx.dragging = None;
                        let value = from_slider(spec, v.end());
                        let effect = this.audio_fx_target().and_then(|(_, fx)| {
                            fx.effects
                                .iter()
                                .find(|e| {
                                    e.kind == kind
                                        || (kind == "voice"
                                            && fx_catalog::VOICE_PRESETS.contains(&e.kind.as_str()))
                                })
                                .map(|e| e.id.clone())
                        });
                        if let Some(effect) = effect {
                            this.run_audio_fx(cx, |state, segment| {
                                fx_commands::audiofx_set_param(
                                    state,
                                    segment,
                                    effect,
                                    spec.id.into(),
                                    value,
                                )
                                .map(|_| ())
                            });
                        }
                    }
                },
            );
            slot.insert(ParamSlider {
                state,
                _subscription: subscription,
            });
        }
        let state = self.inspector.audio_fx.sliders[&key].state.clone();
        let position = to_slider(spec, value);
        if self.inspector.audio_fx.dragging != Some(key)
            && (state.read(cx).value().end() - position).abs() > 1e-4
        {
            state.update(cx, |s, cx| s.set_value(position, window, cx));
        }
        state
    }

    /// The Voice changer: a grid of presets, one at a time, and the chosen
    /// one's intensity.
    pub(super) fn voice_changer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((_, fx)) = self.audio_fx_target() else {
            return not_yet("Voice changer");
        };
        let current = fx
            .effects
            .iter()
            .find(|e| fx_catalog::VOICE_PRESETS.contains(&e.kind.as_str()))
            .cloned();
        let tile =
            |id: &'static str, label: &'static str, selected: bool, cx: &mut Context<Self>| {
                div()
                    .id(SharedString::from(format!("voice-{id}")))
                    .h(px(56.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(4.0))
                    .rounded(px(R_SM))
                    .border_1()
                    .border_color(rgb(if selected { ACCENT } else { BORDER }))
                    .bg(if selected {
                        accent_soft()
                    } else {
                        rgb(WELL).into()
                    })
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(if selected { ACCENT } else { TEXT_DIM }))
                    .cursor_pointer()
                    .hover(|style| style.text_color(rgb(TEXT)).border_color(rgb(BORDER_STRONG)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let kind = (id != "none").then(|| id.to_string());
                        this.run_audio_fx(cx, |state, segment| {
                            fx_commands::audiofx_set_voice(state, segment, kind).map(|_| ())
                        })
                    }))
                    .child(label)
            };
        let mut tiles = vec![tile("none", "None", current.is_none(), cx).into_any_element()];
        for kind in fx_catalog::VOICE_PRESETS {
            let label = descriptor(kind).map_or(kind, |d| d.label);
            let selected = current.as_ref().is_some_and(|c| c.kind == kind);
            tiles.push(tile(kind, label, selected, cx).into_any_element());
        }
        let grid = div()
            .grid()
            .grid_cols(3)
            .gap(px(8.0))
            .children(tiles)
            .into_any_element();
        let mut rows = vec![grid];
        if let Some(effect) = &current {
            if let Some(spec) = descriptor(&effect.kind).and_then(|d| d.param("intensity")) {
                let value = effect.value("intensity").unwrap_or(1.0);
                // One intensity slider whichever voice it is: keyed on a
                // shared name, so switching voices keeps the thumb in place.
                rows.push(self.audio_param_row("voice", &effect.id, spec, value, window, cx));
            }
        }
        Section::new("Voice changer").render(
            self.inspector.collapsed.contains("Voice changer"),
            rows,
            cx,
        )
    }

    /// The Speed tab's "Change audio pitch" switch.
    pub(super) fn pitch_switch(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some((_, fx)) = self.audio_fx_target() else {
            return label_row(
                "Change audio pitch",
                Switch::new("speed-pitch").checked(false).disabled(true),
            );
        };
        let entity = cx.entity().downgrade();
        label_row(
            "Change audio pitch",
            Switch::new("speed-pitch")
                .checked(fx.pitch_follows_speed)
                .on_click(move |checked, _, cx| {
                    let checked = *checked;
                    let _ = entity.update(cx, |this, cx| {
                        this.run_audio_fx(cx, |state, segment| {
                            fx_commands::audiofx_set_pitch_follows_speed(state, segment, checked)
                                .map(|_| ())
                        })
                    });
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_slider_round_trips_and_spans_its_range() {
        let spec = descriptor("eq3").unwrap().param("mid_freq").unwrap();
        for value in [200.0, 1_000.0, 5_000.0] {
            let back = from_slider(spec, to_slider(spec, value));
            assert!(
                (back - value).abs() / value < 1e-4,
                "{value} came back {back}"
            );
        }
        assert!((to_slider(spec, 200.0)).abs() < 1e-6);
        assert!((to_slider(spec, 5_000.0) - 1.0).abs() < 1e-6);
        assert_eq!(readout(spec, 1_500.0), "1.5 kHz");
    }
}
