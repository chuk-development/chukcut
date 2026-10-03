//! The asset panel on the left, laid out like CapCut's: a row of icon tabs,
//! a category column, a search field and a tile grid.
//!
//! Only tabs that lead somewhere real are shown — Media, Audio, Text,
//! Transitions and Filters. Stickers, effect libraries, templates and the AI
//! tabs have nothing behind them yet.

mod library;
mod media;

use std::collections::HashMap;

use gpui::assets::IconName as Lucide;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::{Icon, Sizable as _};
use gpui::{AnyElement, Entity, ExternalPaths, Subscription};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetTab {
    Media,
    Audio,
    Text,
    Transitions,
    Filters,
}

impl AssetTab {
    const ALL: [AssetTab; 5] = [
        AssetTab::Media,
        AssetTab::Audio,
        AssetTab::Text,
        AssetTab::Transitions,
        AssetTab::Filters,
    ];

    fn label(self) -> &'static str {
        match self {
            AssetTab::Media => "Media",
            AssetTab::Audio => "Audio",
            AssetTab::Text => "Text",
            AssetTab::Transitions => "Transitions",
            AssetTab::Filters => "Filters",
        }
    }

    fn icon(self) -> Lucide {
        match self {
            AssetTab::Media => Lucide::Clapperboard,
            AssetTab::Audio => Lucide::Music,
            AssetTab::Text => Lucide::Type,
            AssetTab::Transitions => Lucide::Blend,
            AssetTab::Filters => Lucide::SlidersHorizontal,
        }
    }

    /// The entries of the category column. The first one is the default.
    fn categories(self) -> &'static [&'static str] {
        match self {
            AssetTab::Media => &["Import", "Project media"],
            AssetTab::Audio => &["Import", "Project audio"],
            AssetTab::Text => &["Add text"],
            AssetTab::Transitions => &["Transitions"],
            AssetTab::Filters => &["Filters"],
        }
    }

    fn searchable(self) -> bool {
        !matches!(self, AssetTab::Text)
    }
}

/// A media tile's picture.
pub(crate) enum Thumb {
    Loading,
    Ready(PathBuf),
    Failed,
}

/// The panel's own state, held as one field on the editor.
pub(crate) struct AssetPanel {
    tab: AssetTab,
    /// The selected category of each tab, by index into `categories()`.
    category: HashMap<&'static str, usize>,
    search: Entity<InputState>,
    /// Poster frames of video materials, by material id. Filled in the
    /// background the first time a tile is drawn.
    thumbs: HashMap<String, Thumb>,
    /// The tile last clicked, drawn with an outline.
    picked: Option<String>,
    _search_changed: Subscription,
}

impl AssetPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Editor>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            tab: AssetTab::Media,
            category: HashMap::new(),
            search,
            thumbs: HashMap::new(),
            picked: None,
            _search_changed: subscription,
        }
    }

    fn category(&self) -> usize {
        self.category.get(self.tab.label()).copied().unwrap_or(0)
    }

    /// The search text, lower-cased, or `None` when the field is empty.
    fn query(&self, cx: &App) -> Option<String> {
        let text = self.search.read(cx).value().trim().to_lowercase();
        (!text.is_empty()).then_some(text)
    }
}

/// Whether `name` passes the search field.
fn matches(name: &str, query: &Option<String>) -> bool {
    query
        .as_ref()
        .is_none_or(|q| name.to_lowercase().contains(q.as_str()))
}

/// `mm:ss`, as the duration badge on a tile.
fn badge_time(duration: Micros) -> String {
    let seconds = (duration.max(0) as f64 / 1_000_000.0).round() as i64;
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

pub(super) const TILE_W: f32 = 116.0;
pub(super) const TILE_H: f32 = 72.0;

impl Editor {
    pub(super) fn render_media(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.assets.tab;
        let category = self.assets.category();

        let tabs = AssetTab::ALL
            .into_iter()
            .map(|each| {
                let active = each == tab;
                let color = if active { ACCENT } else { TEXT_DIM };
                div()
                    .id(SharedString::from(format!("asset-tab-{}", each.label())))
                    .min_w(px(52.0))
                    .px_1()
                    .py_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(3.0))
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(rgb(color))
                    .hover(|style| style.text_color(rgb(if active { ACCENT } else { TEXT })))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.assets.tab = each;
                        cx.notify();
                    }))
                    .child(Icon::new(each.icon()).size(px(18.0)))
                    .child(div().text_xs().child(each.label()))
            })
            .collect::<Vec<_>>();

        let categories = tab
            .categories()
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let active = index == category;
                div()
                    .id(SharedString::from(format!("asset-category-{name}")))
                    .h(px(28.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_xs()
                    .when(active, |pill| {
                        pill.bg(rgb(PANEL_RAISED)).text_color(rgb(ACCENT))
                    })
                    .when(!active, |pill| {
                        pill.text_color(rgb(TEXT))
                            .hover(|style| style.bg(rgb(PANEL_RAISED)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.assets.category.insert(this.assets.tab.label(), index);
                        cx.notify();
                    }))
                    .child(*name)
            })
            .collect::<Vec<_>>();

        let content = match tab {
            AssetTab::Media => self.render_media_tab(category, cx).into_any_element(),
            AssetTab::Audio => self.render_audio_tab(category, cx).into_any_element(),
            AssetTab::Text => self.render_text_tab(cx).into_any_element(),
            AssetTab::Transitions => self.render_transitions_tab(cx).into_any_element(),
            AssetTab::Filters => self.render_filters_tab(cx).into_any_element(),
        };

        div()
            .id("asset-panel")
            .w(px(MEDIA_W))
            .flex_none()
            .flex()
            .flex_col()
            .rounded_md()
            .overflow_hidden()
            .bg(rgb(PANEL))
            // Files dragged in from the desktop land in the library, wherever
            // on the panel they are dropped.
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                this.import_to_library(paths.paths().to_vec(), cx);
            }))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(rgb(BG))
                    .children(tabs),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .w(px(116.0))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .border_r_1()
                            .border_color(rgb(BG))
                            .children(categories),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap_2()
                            .p_2()
                            .when(tab.searchable(), |column| {
                                column.child(
                                    Input::new(&self.assets.search)
                                        .small()
                                        .cleanable(true)
                                        .prefix(Icon::new(Lucide::Search).size(px(14.0))),
                                )
                            })
                            .child(content),
                    ),
            )
    }

    /// The scrolling area the tiles of a tab live in.
    fn tile_area(id: &'static str) -> gpui::Stateful<gpui::Div> {
        div().id(id).flex_1().min_h(px(0.0)).overflow_y_scroll()
    }

    /// A tile with a picture area, a caption (none when empty), and a "+" that appears on
    /// hover. `picture` fills the picture area; `on_add` runs for the "+".
    fn tile(
        id: SharedString,
        picture: AnyElement,
        caption: String,
        picked: bool,
        on_add: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let group = SharedString::from(format!("{id}-group"));
        div()
            .id(id.clone())
            .group(group.clone())
            .w(px(TILE_W))
            .flex()
            .flex_col()
            .gap_1()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .w(px(TILE_W))
                    .h(px(TILE_H))
                    .rounded_md()
                    .overflow_hidden()
                    .bg(rgb(PANEL_RAISED))
                    .border_1()
                    .border_color(if picked {
                        rgb(ACCENT)
                    } else {
                        rgb(PANEL_RAISED)
                    })
                    .group_hover(group.clone(), |style| {
                        style.border_color(rgb(if picked { ACCENT } else { TEXT_DIM }))
                    })
                    .child(picture)
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-add")))
                            .absolute()
                            .right(px(4.0))
                            .bottom(px(4.0))
                            .size(px(22.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(ACCENT))
                            .text_color(rgb(0x0b1214))
                            .invisible()
                            .group_hover(group, |style| style.visible())
                            .hover(|style| style.bg(rgb(ACCENT_HOVER)))
                            .on_click(move |event, window, cx| {
                                cx.stop_propagation();
                                on_add(event, window, cx);
                            })
                            .child(Icon::new(Lucide::Plus).size(px(14.0))),
                    ),
            )
            .when(!caption.is_empty(), |tile| {
                tile.child(
                    div()
                        .w(px(TILE_W))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child(caption),
                )
            })
    }

    /// CapCut's empty state: a large box that opens the file picker and
    /// accepts files dragged in from the desktop.
    fn drop_zone(
        &self,
        id: &'static str,
        title: &'static str,
        hint: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .flex_1()
            .min_h(px(160.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_lg()
            .bg(rgb(PANEL_RAISED))
            .border_1()
            .border_color(rgb(PANEL_RAISED))
            .cursor_pointer()
            .hover(|style| style.border_color(rgb(BORDER)))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.border_color(rgb(ACCENT)))
            .on_click(cx.listener(|this, _, window, cx| this.on_import(&Import, window, cx)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(22.0))
                            .rounded_full()
                            .bg(rgb(ACCENT))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(0x0b1214))
                            .child(Icon::new(Lucide::Plus).size(px(14.0))),
                    )
                    .child(div().text_color(rgb(TEXT)).child(title)),
            )
            .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(hint))
    }
}
