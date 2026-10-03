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
use gpui::component::Sizable as _;
use gpui::{AnyElement, Entity, ExternalPaths, Subscription};

use super::*;
use crate::ui::{icons, EmptyState, Glyph, IconSrc, Panel, RailTab};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetTab {
    Media,
    Audio,
    Text,
    Captions,
    Transitions,
    Filters,
    /// Pexels, Pixabay and Freesound with the user's keys (`editor/cloud`).
    Stock,
}

impl AssetTab {
    const ALL: [AssetTab; 7] = [
        AssetTab::Media,
        AssetTab::Audio,
        AssetTab::Text,
        AssetTab::Captions,
        AssetTab::Transitions,
        AssetTab::Filters,
        AssetTab::Stock,
    ];

    fn label(self) -> &'static str {
        match self {
            AssetTab::Media => "Media",
            AssetTab::Audio => "Audio",
            AssetTab::Text => "Text",
            AssetTab::Captions => "Captions",
            AssetTab::Transitions => "Transitions",
            AssetTab::Filters => "Filters",
            AssetTab::Stock => "Stock",
        }
    }

    fn icon(self) -> Glyph {
        match self {
            AssetTab::Media => icons::MEDIA,
            AssetTab::Audio => icons::AUDIO,
            AssetTab::Text => icons::TEXT,
            AssetTab::Captions => icons::CAPTIONS,
            AssetTab::Transitions => icons::TRANSITIONS,
            AssetTab::Filters => icons::FILTERS,
            AssetTab::Stock => super::cloud::STOCK_GLYPH,
        }
    }

    /// The entries of the category column. The first one is the default.
    fn categories(self) -> &'static [&'static str] {
        match self {
            // The entries after the first two are `editor/cloud`'s.
            AssetTab::Media => &["Import", "Project media", "AI tools"],
            AssetTab::Audio => &[
                "Import",
                "Project audio",
                super::cloud::AUDIO_CATEGORIES[0],
                super::cloud::AUDIO_CATEGORIES[1],
                super::cloud::AUDIO_CATEGORIES[2],
            ],
            AssetTab::Text => &["Add text"],
            AssetTab::Captions => super::captions::CATEGORIES,
            AssetTab::Transitions => &["Transitions"],
            AssetTab::Filters => &["Filters"],
            AssetTab::Stock => super::cloud::STOCK_CATEGORIES,
        }
    }

    fn searchable(self, category: usize) -> bool {
        match self {
            AssetTab::Text | AssetTab::Captions | AssetTab::Stock => false,
            // The cloud categories have fields of their own.
            AssetTab::Media | AssetTab::Audio => category < 2,
            _ => true,
        }
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
    /// Text to speech, sound effects, music, stock, AI tools, translation.
    pub(crate) cloud: super::cloud::CloudPanel,
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
            cloud: super::cloud::CloudPanel::new(window, cx),
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

/// Four tiles to a row in the asset panel's content column.
pub(super) const TILE_W: f32 = 112.0;
pub(super) const TILE_H: f32 = 70.0;
pub(super) const TILE_GAP: f32 = 12.0;

impl Editor {
    pub(super) fn render_media(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.assets.tab;
        let category = self.assets.category();

        let tabs = AssetTab::ALL
            .into_iter()
            .map(|each| {
                RailTab::new(
                    format!("asset-tab-{}", each.label()),
                    each.icon(),
                    each.label(),
                )
                .selected(each == tab)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.assets.tab = each;
                    cx.notify();
                }))
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
                    .h(px(30.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(R_SM))
                    .cursor_pointer()
                    .text_size(px(TEXT_LABEL))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .when(active, |pill| {
                        pill.bg(rgb(PANEL_RAISED))
                            .text_color(rgb(TEXT))
                            .font_weight(gpui::FontWeight::MEDIUM)
                    })
                    .when(!active, |pill| {
                        pill.text_color(rgb(TEXT_DIM))
                            .hover(|style| style.bg(rgb(PANEL_RAISED)).text_color(rgb(TEXT)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.assets.category.insert(this.assets.tab.label(), index);
                        cx.notify();
                    }))
                    .child(*name)
            })
            .collect::<Vec<_>>();

        let content = match tab {
            AssetTab::Media if category >= 2 => self.render_ai_tools(cx),
            AssetTab::Media => self.render_media_tab(category, cx).into_any_element(),
            AssetTab::Audio if category >= 2 => self.render_cloud_audio(category - 2, cx),
            AssetTab::Audio => self.render_audio_tab(category, cx).into_any_element(),
            AssetTab::Text => self.render_text_tab(cx).into_any_element(),
            AssetTab::Captions => self.render_captions_tab(category, cx),
            AssetTab::Transitions => self.render_transitions_tab(cx).into_any_element(),
            AssetTab::Filters => self.render_filters_tab(cx).into_any_element(),
            AssetTab::Stock => self.render_stock_tab(category, cx),
        };

        Panel::new("asset-panel")
            .w(px(MEDIA_W))
            .flex_none()
            // The rail stands in for a header: CapCut's icon tabs, our glyphs.
            .header(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .px(px(GUTTER))
                    .py(px(GUTTER))
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .children(tabs),
            )
            .child(
                div()
                    .id("asset-body")
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    // Files dragged in from the desktop land in the library,
                    // wherever on the panel they are dropped.
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.import_to_library(paths.paths().to_vec(), cx);
                    }))
                    .child(
                        div()
                            .w(px(124.0))
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .p(px(8.0))
                            .border_r_1()
                            .border_color(rgb(HAIRLINE))
                            .children(categories),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .p(px(PAD))
                            .when(tab.searchable(category), |column| {
                                column.child(
                                    Input::new(&self.assets.search)
                                        .small()
                                        .cleanable(true)
                                        .text_size(px(TEXT_BODY))
                                        .bg(rgb(WELL))
                                        .border_color(rgb(BORDER))
                                        .prefix(
                                            IconSrc::from(Lucide::Search)
                                                .svg(14.0, rgb(TEXT_MUTED)),
                                        ),
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
            .gap(px(6.0))
            .cursor_pointer()
            .child(
                // The ring sits outside the picture, so a picked tile does
                // not crop its own picture.
                div()
                    .p(px(2.0))
                    .m(px(-2.0))
                    .rounded(px(R_SM + 2.0))
                    .border_1()
                    .border_color(if picked {
                        rgb(ACCENT)
                    } else {
                        gpui::transparent_black().into()
                    })
                    .child(
                        div()
                            .relative()
                            .w(px(TILE_W))
                            .h(px(TILE_H))
                            .rounded(px(R_SM))
                            .overflow_hidden()
                            .bg(rgb(PANEL_RAISED))
                            .border_1()
                            .border_color(rgb(HAIRLINE))
                            .group_hover(group.clone(), |style| {
                                style.border_color(rgb(BORDER_STRONG))
                            })
                            .child(picture)
                            .child(
                                div()
                                    .id(SharedString::from(format!("{id}-add")))
                                    .absolute()
                                    .right(px(5.0))
                                    .bottom(px(5.0))
                                    .size(px(22.0))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(rgb(ACCENT))
                                    .invisible()
                                    .group_hover(group.clone(), |style| style.visible())
                                    .hover(|style| style.bg(rgb(ACCENT_HOVER)))
                                    .on_click(move |event, window, cx| {
                                        cx.stop_propagation();
                                        on_add(event, window, cx);
                                    })
                                    .child(icons::glyph(icons::PLUS, 14.0, rgb(ON_ACCENT))),
                            ),
                    ),
            )
            .when(!caption.is_empty(), |tile| {
                tile.child(
                    div()
                        .w(px(TILE_W))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(TEXT_CAPTION))
                        .text_color(rgb(if picked { TEXT } else { TEXT_DIM }))
                        .group_hover(group, |style| style.text_color(rgb(TEXT)))
                        .child(caption),
                )
            })
    }

    /// The empty library: a well that opens the file picker and accepts
    /// files dragged in from the desktop.
    fn drop_zone(
        &self,
        id: &'static str,
        title: &'static str,
        hint: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        EmptyState::new(id, icons::IMPORT, title)
            .hint(hint)
            .drop_zone(cx.listener(|this, _, window, cx| this.on_import(&Import, window, cx)))
    }
}
