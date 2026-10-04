//! The Effects tab: the built-in effects by category, and the layouts.
//!
//! Every tile is a real render: the compositor draws the effect over a
//! sample picture once, in the background, and the engine keeps the PNG
//! (`fx/tiles.rs`). A click applies the effect to the selected clip, or puts
//! an effect clip at the playhead when nothing is selected; a drag onto the
//! timeline puts an effect clip where it is dropped.

use chukcut_engine::modules::fx::commands as fx_commands;
use chukcut_engine::modules::fx::edit::{Corner, SplitLayout};
use chukcut_engine::modules::fx::{self, Category};
use gpui::{img, ObjectFit};

use super::*;

/// Tiles are rendered at twice their size, so they stay sharp on a HiDPI
/// screen.
pub(super) const TILE_PX: (u32, u32) = ((TILE_W * 2.0) as u32, (TILE_H * 2.0) as u32);

/// The Effects tab's category column: the catalog's categories, in order.
pub(super) const CATEGORIES: [&str; 8] = [
    "Blur", "Light", "Motion", "Retro", "Distort", "Film", "Layout", "Face",
];

/// What an effect tile carries while it is dragged towards the timeline.
#[derive(Clone)]
pub(crate) struct EffectDrag {
    pub kind: &'static str,
    pub name: &'static str,
}

impl Render for EffectDrag {
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
            .child(self.name)
    }
}

/// A layout tile: an arrangement rather than an effect.
#[derive(Clone, Copy)]
enum Layout {
    Pip,
    Split(SplitLayout),
}

impl Editor {
    /// The picture of a rendered tile: the cached PNG once it exists, a
    /// placeholder while the compositor draws it. `key` names it among the
    /// panel's thumbnails; `render` runs on a background thread.
    pub(super) fn rendered_tile(
        &mut self,
        key: String,
        render: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let placeholder = || div().size_full().bg(rgb(PANEL_RAISED)).into_any_element();
        match self.assets.thumbs.get(&key) {
            Some(Thumb::Ready(path)) => img(path.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            Some(Thumb::Loading) | Some(Thumb::Failed) => placeholder(),
            None => {
                self.assets.thumbs.insert(key.clone(), Thumb::Loading);
                cx.spawn(async move |this, cx| {
                    let rendered = cx
                        .background_executor()
                        .spawn(async move { render() })
                        .await;
                    let thumb = match rendered {
                        Ok(path) => Thumb::Ready(path),
                        Err(error) => {
                            tracing::warn!(%error, %key, "no preview tile");
                            Thumb::Failed
                        }
                    };
                    let _ = this.update(cx, |editor, cx| {
                        editor.assets.thumbs.insert(key, thumb);
                        cx.notify();
                    });
                })
                .detach();
                placeholder()
            }
        }
    }

    pub(super) fn render_effects_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.assets.query(cx);
        let category = Category::ALL[category.min(Category::ALL.len() - 1)];
        let hint = match (category, self.selected.is_some()) {
            (Category::Layout, _) => {
                "Select a clip for picture in picture, or several for a split screen."
            }
            (_, true) => {
                "Click to add to the selected clip. Drag onto the timeline for an effect clip."
            }
            (_, false) => {
                "Click to add an effect clip at the playhead, or drag one onto the timeline."
            }
        };

        let mut tiles: Vec<AnyElement> = Vec::new();
        if category == Category::Layout {
            let layouts = [(Layout::Pip, "Picture in picture")].into_iter().chain(
                SplitLayout::ALL
                    .into_iter()
                    .map(|l| (Layout::Split(l), l.label())),
            );
            for (layout, label) in layouts {
                if !matches(label, &query) {
                    continue;
                }
                let (key, picture) = match layout {
                    Layout::Pip => {
                        let picture = self.rendered_tile(
                            "layout:pip".into(),
                            || fx_commands::fx_pip_tile(TILE_PX),
                            cx,
                        );
                        ("pip".to_string(), picture)
                    }
                    Layout::Split(split) => {
                        let picture = self.rendered_tile(
                            format!("layout:{split:?}"),
                            move || fx_commands::fx_split_tile(split, TILE_PX),
                            cx,
                        );
                        (format!("{split:?}"), picture)
                    }
                };
                let editor = cx.entity().downgrade();
                tiles.push(
                    Self::tile(
                        SharedString::from(format!("layout-{key}")),
                        picture,
                        label.to_string(),
                        false,
                        move |_, _, cx| {
                            let _ = editor.update(cx, |this, cx| this.apply_layout(layout, cx));
                        },
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.apply_layout(layout, cx)))
                    .into_any_element(),
                );
            }
        }
        for effect in fx::catalog()
            .iter()
            .filter(|e| e.category == category && matches(e.label, &query))
        {
            let kind = effect.id;
            let picture = self.rendered_tile(
                format!("fx:{kind}"),
                move || fx_commands::fx_tile(kind.to_string(), TILE_PX),
                cx,
            );
            let editor = cx.entity().downgrade();
            let description = effect.description;
            tiles.push(
                Self::tile(
                    SharedString::from(format!("effect-{kind}")),
                    picture,
                    effect.label.to_string(),
                    false,
                    move |_, _, cx| {
                        let _ = editor.update(cx, |this, cx| this.apply_effect(kind, cx));
                    },
                )
                .tooltip(move |window, cx| {
                    gpui::component::tooltip::Tooltip::new(description).build(window, cx)
                })
                .on_click(cx.listener(move |this, _, _, cx| this.apply_effect(kind, cx)))
                .on_drag(
                    EffectDrag {
                        kind,
                        name: effect.label,
                    },
                    |drag, _, _, cx| cx.new(|_| drag.clone()),
                )
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
            .child(Self::tile_area("effect-grid").child(Self::tile_grid(tiles)))
    }

    /// Add `kind` to the selected clip, or put an effect clip at the playhead
    /// when no picture is selected.
    fn apply_effect(&mut self, kind: &'static str, cx: &mut Context<Self>) {
        let target = self.selected.clone().filter(|id| {
            self.project
                .segment(id)
                .is_some_and(|(track, _)| track.kind != TrackKind::Audio)
        });
        let result = match target {
            Some(segment_id) => {
                fx_commands::fx_add(&self.state, segment_id, kind.to_string()).map(|_| ())
            }
            None => {
                let at = self.clock.position();
                fx_commands::fx_add_clip(&self.state, kind.to_string(), at, None, None).map(
                    |added| {
                        self.selected = Some(added.segment_id);
                    },
                )
            }
        };
        if result.is_ok() {
            self.inspector_show_effects();
        }
        self.refresh(cx);
        self.report(result, cx);
    }

    fn apply_layout(&mut self, layout: Layout, cx: &mut Context<Self>) {
        let result = match layout {
            Layout::Pip => match self.selected.clone() {
                Some(id) => {
                    fx_commands::fx_layout_pip(&self.state, id, Corner::TopRight).map(|_| ())
                }
                None => Err("Select the clip to show in a corner".into()),
            },
            Layout::Split(split) => {
                // Topmost lane first: the clip that sits above goes in the
                // first cell, which is the top or the left one.
                let mut ids = self.selection();
                let order = |id: &String| {
                    self.project
                        .tracks
                        .iter()
                        .position(|t| t.segments.iter().any(|s| &s.id == id))
                        .unwrap_or(0)
                };
                ids.sort_by_key(|id| std::cmp::Reverse(order(id)));
                fx_commands::fx_layout_split(&self.state, ids, split).map(|_| ())
            }
        };
        self.refresh(cx);
        self.report(result, cx);
    }

    /// An effect dropped on the timeline: an effect clip where it landed.
    pub(crate) fn on_effect_drop(
        &mut self,
        drag: &EffectDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((at, lane)) = self.drop_target(window.mouse_position()) else {
            return;
        };
        let lane = lane.filter(|id| {
            self.project
                .track(id)
                .is_some_and(|t| t.kind == TrackKind::Effect)
        });
        let result = fx_commands::fx_add_clip(&self.state, drag.kind.to_string(), at, None, lane)
            .map(|added| {
                self.selected = Some(added.segment_id);
            });
        if result.is_ok() {
            self.inspector_show_effects();
        }
        self.refresh(cx);
        self.report(result, cx);
    }
}
