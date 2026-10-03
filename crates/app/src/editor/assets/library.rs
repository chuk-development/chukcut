//! The Text, Transitions and Filters tabs. Their tiles are our own drawings,
//! not previews of real footage: they say what the effect does at a glance.

use chukcut_engine::modules::inspector::commands as inspector_commands;
use chukcut_engine::modules::inspector::edit::ColorEdit;
use chukcut_engine::modules::project::TransitionKind;
use chukcut_engine::modules::text::commands as text_commands;
use chukcut_engine::modules::transitions::commands as transition_commands;
use gpui::{linear_color_stop, linear_gradient, Hsla};

use super::*;

/// A filter: a named colour grade, applied to the selected clip through the
/// same command as the inspector's sliders.
struct Filter {
    name: &'static str,
    /// `None` clears the clip's grade.
    grade: Option<(f32, f32, f32, f32)>,
    /// The tile's two colours.
    swatch: (u32, u32),
}

/// (brightness, contrast, saturation, temperature), each at rest at 0, 1, 1, 0.
const FILTERS: [Filter; 8] = [
    Filter {
        name: "None",
        grade: None,
        swatch: (0x4a4a4a, 0x2a2a2a),
    },
    Filter {
        name: "Warm",
        grade: Some((0.02, 1.05, 1.1, 0.3)),
        swatch: (0xf4a259, 0xbc4b51),
    },
    Filter {
        name: "Cool",
        grade: Some((0.0, 1.05, 1.0, -0.3)),
        swatch: (0x5bc0eb, 0x2b4c7e),
    },
    Filter {
        name: "Vivid",
        grade: Some((0.0, 1.12, 1.45, 0.0)),
        swatch: (0xff3d7f, 0xffd23f),
    },
    Filter {
        name: "Mono",
        grade: Some((0.0, 1.1, 0.0, 0.0)),
        swatch: (0xd9d9d9, 0x3a3a3a),
    },
    Filter {
        name: "Fade",
        grade: Some((0.06, 0.78, 0.75, 0.0)),
        swatch: (0xd8cfc4, 0x9a9590),
    },
    Filter {
        name: "Punch",
        grade: Some((0.0, 1.3, 1.2, 0.05)),
        swatch: (0xe63946, 0x1d3557),
    },
    Filter {
        name: "Moody",
        grade: Some((-0.06, 1.15, 0.7, -0.12)),
        swatch: (0x3d5a6c, 0x101820),
    },
];

fn gradient(from: u32, to: u32) -> gpui::Background {
    let (from, to): (Hsla, Hsla) = (rgb(from).into(), rgb(to).into());
    linear_gradient(
        135.0,
        linear_color_stop(from, 0.0),
        linear_color_stop(to, 1.0),
    )
}

/// The picture of a transition tile: clip A (teal) giving way to clip B
/// (violet) the way the transition does it.
fn transition_art(kind: TransitionKind) -> AnyElement {
    const A: u32 = 0x1a9aa5;
    const B: u32 = 0x6b5bd2;
    let half = |color: u32| div().flex_1().h_full().bg(rgb(color));
    let base = div().size_full().flex().flex_row().overflow_hidden();
    match kind {
        TransitionKind::Dissolve => base
            .bg({
                let (a, b): (Hsla, Hsla) = (rgb(A).into(), rgb(B).into());
                linear_gradient(90.0, linear_color_stop(a, 0.2), linear_color_stop(b, 0.8))
            })
            .into_any_element(),
        TransitionKind::DipToColor => base
            .child(half(A))
            .child(div().w(px(36.0)).h_full().bg(rgb(0x000000)))
            .child(half(B))
            .into_any_element(),
        TransitionKind::Wipe => base
            .child(half(B))
            .child(div().w(px(3.0)).h_full().bg(rgb(0xffffff)))
            .child(half(A))
            .into_any_element(),
        TransitionKind::Slide => base
            .bg(rgb(B))
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(-40.0))
                    .top_0()
                    .w(px(TILE_W))
                    .h_full()
                    .bg(rgb(A))
                    .border_r_2()
                    .border_color(rgb(0xffffff)),
            )
            .into_any_element(),
        // Library transitions are drawn by the compositor, not here.
        TransitionKind::Library => base
            .bg({
                let (a, b): (Hsla, Hsla) = (rgb(A).into(), rgb(B).into());
                linear_gradient(135.0, linear_color_stop(a, 0.3), linear_color_stop(b, 0.7))
            })
            .into_any_element(),
        TransitionKind::Zoom => base
            .bg(rgb(B))
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(64.0))
                    .h(px(38.0))
                    .rounded_sm()
                    .bg(rgb(A))
                    .border_1()
                    .border_color(rgb(0xffffff)),
            )
            .into_any_element(),
    }
}

impl Editor {
    /// One dim line above a tab's tiles.
    fn hint(text: impl Into<SharedString>) -> impl IntoElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .text_size(px(TEXT_CAPTION + 1.0))
            .text_color(rgb(TEXT_MUTED))
            .child(IconSrc::from(Lucide::Info).svg(13.0, rgb(TEXT_MUTED)))
            .child(text.into())
    }

    /// The wrapping grid of a tab's tiles.
    fn tile_grid(tiles: impl IntoIterator<Item = impl IntoElement>) -> gpui::Div {
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_x(px(TILE_GAP))
            .gap_y(px(12.0))
            .pt(px(2.0))
            .pl(px(2.0))
            .children(tiles)
    }

    pub(super) fn render_text_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let picture = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gradient(0x2a2c32, 0x1a1b1f))
            .text_size(px(TEXT_DISPLAY + 3.0))
            .font_weight(gpui::FontWeight::BOLD)
            .text_color(rgb(TEXT))
            .child("Aa")
            .into_any_element();
        let editor = cx.entity().downgrade();
        let tile = Self::tile(
            "text-default".into(),
            picture,
            "Default text".into(),
            false,
            move |_, _, cx| {
                let _ = editor.update(cx, |this, cx| this.add_text(cx));
            },
        )
        .on_click(cx.listener(|this, _, _, cx| this.add_text(cx)));

        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(Self::hint("Adds a title at the playhead."))
            .child(Self::tile_grid([tile]))
    }

    fn add_text(&mut self, cx: &mut Context<Self>) {
        let at = self.clock.position();
        match text_commands::text_add(&self.state, at, None, None) {
            Ok(added) => {
                self.refresh(cx);
                self.selected = Some(added.segment_id);
                self.report(Ok(()), cx);
            }
            Err(error) => self.report(Err(error), cx),
        }
    }

    pub(super) fn render_transitions_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.assets.query(cx);
        let hint = if self.selected.is_some() {
            "Click a transition to put it between the selected clip and the next."
        } else {
            "Select a clip on the timeline, then pick a transition."
        };
        let tiles = transition_commands::transitions_catalog()
            .into_iter()
            .filter(|descriptor| matches(descriptor.label, &query))
            .map(|descriptor| {
                let kind = descriptor.kind;
                let editor = cx.entity().downgrade();
                Self::tile(
                    SharedString::from(format!("transition-{:?}", kind)),
                    transition_art(kind),
                    descriptor.label.to_string(),
                    false,
                    move |_, _, cx| {
                        let _ = editor.update(cx, |this, cx| this.add_transition(kind, cx));
                    },
                )
                .tooltip({
                    let description = descriptor.description;
                    move |window, cx| {
                        gpui::component::tooltip::Tooltip::new(description).build(window, cx)
                    }
                })
                .on_click(cx.listener(move |this, _, _, cx| this.add_transition(kind, cx)))
            });
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(Self::hint(hint))
            .child(Self::tile_area("transition-grid").child(Self::tile_grid(tiles)))
    }

    /// Put a transition at the end of the selected clip, which is where
    /// CapCut puts it: the head of the clip after it. A last clip takes it
    /// at its own head instead. A clip that already has one gets its kind
    /// changed rather than a second one.
    fn add_transition(&mut self, kind: TransitionKind, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.clone() else {
            self.report(
                Err("Select a clip first: the transition goes between it and the next one".into()),
                cx,
            );
            return;
        };
        let next = self
            .project
            .segment(&selected)
            .and_then(|(track, segment)| {
                let end = segment.target_range.start + segment.target_range.duration;
                track
                    .segments
                    .iter()
                    .filter(|s| s.target_range.start >= end)
                    .min_by_key(|s| s.target_range.start)
                    .map(|s| s.id.clone())
            });
        let target = next.into_iter().chain([selected]).find(|id| {
            transition_commands::transitions_max_duration(&self.state, id.clone())
                .is_ok_and(|room| room > 0)
        });
        let Some(target) = target else {
            self.report(
                Err("A transition needs two clips side by side on one lane".into()),
                cx,
            );
            return;
        };
        let existing = self
            .project
            .segment(&target)
            .and_then(|(_, segment)| self.project.materials.transition_of(segment).cloned());
        let result = match existing {
            Some(mut transition) => {
                transition.kind = kind;
                transition_commands::transitions_set(&self.state, target, transition)
            }
            None => transition_commands::transitions_add(&self.state, target, kind, None),
        }
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    pub(super) fn render_filters_tab(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.assets.query(cx);
        let hint = if self.selected.is_some() {
            "Click a filter to grade the selected clip."
        } else {
            "Select a clip on the timeline, then pick a filter."
        };
        let tiles = FILTERS
            .iter()
            .enumerate()
            .filter(|(_, filter)| matches(filter.name, &query))
            .map(|(index, filter)| {
                let picture = div()
                    .size_full()
                    .bg(gradient(filter.swatch.0, filter.swatch.1))
                    .into_any_element();
                let editor = cx.entity().downgrade();
                Self::tile(
                    SharedString::from(format!("filter-{}", filter.name)),
                    picture,
                    filter.name.to_string(),
                    false,
                    move |_, _, cx| {
                        let _ = editor.update(cx, |this, cx| this.apply_filter(index, cx));
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| this.apply_filter(index, cx)))
            });
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(Self::hint(hint))
            .child(Self::tile_area("filter-grid").child(Self::tile_grid(tiles)))
    }

    fn apply_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(selected) = self.selected.clone() else {
            self.report(Err("Select a clip first".into()), cx);
            return;
        };
        let color =
            FILTERS[index].grade.map(
                |(brightness, contrast, saturation, temperature)| ColorEdit {
                    brightness,
                    contrast,
                    saturation,
                    temperature,
                    lut: None,
                },
            );
        let result =
            inspector_commands::inspector_set_color(&self.state, selected, color).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }
}
