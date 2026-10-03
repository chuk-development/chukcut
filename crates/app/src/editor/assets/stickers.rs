//! The Stickers tab: emoji by Unicode group in three looks, and icons from
//! the Iconify sets the licence policy allows.
//!
//! A click (or the "+") fetches the sticker with its licence record and puts
//! it on an overlay lane at the playhead, centred
//! (`library::commands::library_sticker_add`), selected so it can be moved
//! and tracked at once.

use chukcut_engine::modules::library::commands as library;
use chukcut_engine::modules::library::licence::Policy;
use chukcut_engine::modules::library::stickers::{IconHit, Sticker, StickerStyle};

use super::library_panel::{chip, chips, notice, STICKER_PAGE};
use super::*;

/// The category column: Unicode's groups (shortened), then icons.
pub(super) const CATEGORIES: [&str; 10] = [
    "Smileys",
    "People",
    "Animals",
    "Food",
    "Travel",
    "Activities",
    "Objects",
    "Symbols",
    "Flags",
    "Icons",
];

/// The Unicode group behind each emoji category.
const GROUPS: [&str; 9] = [
    "Smileys & Emotion",
    "People & Body",
    "Animals & Nature",
    "Food & Drink",
    "Travel & Places",
    "Activities",
    "Objects",
    "Symbols",
    "Flags",
];

const ICONS: usize = 9;
const STICKER_TILE: f32 = 72.0;

/// The Stickers rail glyph: a square with a peeled corner, on the kit's grid.
pub(crate) const STICKER_GLYPH: crate::ui::Glyph = crate::ui::Glyph(
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><path d="M20 13V7a3 3 0 0 0-3-3H7a3 3 0 0 0-3 3v10a3 3 0 0 0 3 3h6z"/><path d="M13 20v-4a3 3 0 0 1 3-3h4"/><path d="M9 10h.01M15 10h.01"/></svg>"#,
);

impl Editor {
    pub(super) fn render_stickers_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if category == ICONS {
            return self.render_icons(cx);
        }
        self.load_sticker_index(cx);
        let style = self.assets.library.sticker_style;
        let styles = chips(StickerStyle::ALL.into_iter().map(|each| {
            chip(
                SharedString::from(format!("sticker-style-{}", each.label())),
                each.label(),
                each == style,
                cx.listener(move |this, _, _, cx| {
                    this.assets.library.sticker_style = each;
                    this.assets.library.sticker_shown = STICKER_PAGE;
                    cx.notify();
                }),
            )
        }));
        let licence = match style {
            StickerStyle::Noto => {
                "Noto Emoji by Google, Apache 2.0: free to use, no credit needed."
            }
            _ => "Fluent Emoji by Microsoft, MIT: free to use, no credit needed.",
        };
        let head = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(styles)
            .child(Self::hint(licence));

        let index = match &self.assets.library.sticker_index {
            None => return column_with(head, notice("Loading the emoji list\u{2026}", TEXT_MUTED)),
            Some(Err(error)) => {
                let error = error.clone();
                return column_with(
                    head,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(notice(error, DANGER))
                        .child(chip(
                            "sticker-retry",
                            "Try again",
                            false,
                            cx.listener(|this, _, _, cx| {
                                this.assets.library.sticker_index = None;
                                cx.notify();
                            }),
                        )),
                );
            }
            Some(Ok(index)) => Arc::clone(index),
        };
        let query = self.assets.query(cx).unwrap_or_default();
        let group = GROUPS.get(category).copied();
        let found = index.search(&query, group, style);
        let total = found.len();
        let shown = self.assets.library.sticker_shown.min(total);
        let mut tiles = Vec::with_capacity(shown);
        for sticker in found.into_iter().take(shown) {
            tiles.push(self.sticker_tile(sticker, style, cx));
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(sticker_grid(tiles));
        if index.stale {
            body = body.child(notice(
                "Offline: the emoji list is from the last time. Stickers already used still work.",
                TEXT_MUTED,
            ));
        }
        if total == 0 {
            body = body.child(notice(
                if style == StickerStyle::Noto {
                    "No emoji matches."
                } else {
                    "No emoji matches in this look. Try Noto, which has every emoji."
                },
                TEXT_MUTED,
            ));
        }
        if shown < total {
            body = body.child(chip(
                "sticker-more",
                format!("Show more ({} left)", total - shown),
                false,
                cx.listener(|this, _, _, cx| {
                    this.assets.library.sticker_shown += STICKER_PAGE;
                    cx.notify();
                }),
            ));
        }
        column_with(head, Self::tile_area("sticker-grid").child(body))
    }

    fn load_sticker_index(&mut self, cx: &mut Context<Self>) {
        let library = &mut self.assets.library;
        if library.sticker_index.is_some() || library.sticker_loading {
            return;
        }
        library.sticker_loading = true;
        self.library_task(cx, library::library_sticker_index, |editor, result, _| {
            editor.assets.library.sticker_loading = false;
            editor.assets.library.sticker_index = Some(result);
        });
    }

    fn sticker_tile(
        &mut self,
        sticker: Sticker,
        style: StickerStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = format!("st:{}:{}", style.label(), sticker.id);
        let for_thumb = sticker.clone();
        let glyph = sticker.glyph.clone();
        let picture = self.library_pic(
            key.clone(),
            move || library::library_sticker_thumb(&for_thumb, style),
            move || {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(28.0))
                    .child(glyph)
                    .into_any_element()
            },
            cx,
        );
        let busy = self.assets.library.busy.contains(&key);
        let add = sticker.clone();
        let name = sticker.name.clone();
        sticker_tile(
            SharedString::from(key.clone()),
            picture,
            busy,
            cx.listener(move |this, _, _, cx| {
                let sticker = add.clone();
                this.add_sticker(
                    key.clone(),
                    move || library::library_sticker_fetch(&sticker, style),
                    cx,
                )
            }),
        )
        .tooltip(move |window, cx| {
            gpui::component::tooltip::Tooltip::new(name.clone()).build(window, cx)
        })
        .into_any_element()
    }

    /// Fetch a sticker file, then put it on the timeline at the playhead.
    fn add_sticker(
        &mut self,
        key: String,
        fetch: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if !self.assets.library.busy.insert(key.clone()) {
            return;
        }
        let at = self.clock.position();
        self.library_task(cx, fetch, move |editor, result, cx| {
            editor.assets.library.busy.remove(&key);
            let path = match result {
                Ok(path) => path,
                Err(error) => {
                    editor.report(Err(error), cx);
                    return;
                }
            };
            let state = Arc::clone(&editor.state);
            cx.spawn(async move |this, cx| {
                let added = library::library_sticker_add(&state, &path, at).await;
                let _ = this.update(cx, |editor, cx| {
                    editor.refresh(cx);
                    match added {
                        Ok(_) => {
                            // Select the new sticker: the last segment that
                            // starts at the playhead on an image lane.
                            editor.selected = editor
                                .project
                                .tracks
                                .iter()
                                .rev()
                                .flat_map(|t| t.segments.iter())
                                .find(|s| {
                                    s.target_range.start == at.max(0)
                                        && editor.project.materials.image(&s.material_id).is_some()
                                })
                                .map(|s| s.id.clone());
                            editor.report(Ok(()), cx);
                        }
                        Err(error) => editor.report(Err(error), cx),
                    }
                });
            })
            .detach();
        });
        cx.notify();
    }

    // --- icons ----------------------------------------------------------------

    /// The search field changed: search Iconify after a short pause, when the
    /// Icons category is showing.
    pub(super) fn icons_typed(&mut self, cx: &mut Context<Self>) {
        if self.assets.tab != AssetTab::Stickers || self.assets.category() != ICONS {
            return;
        }
        self.assets.library.icon_typing += 1;
        let generation = self.assets.library.icon_typing;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |editor, cx| {
                if editor.assets.library.icon_typing == generation {
                    editor.search_icons(cx);
                }
            });
        })
        .detach();
    }

    fn search_icons(&mut self, cx: &mut Context<Self>) {
        let query = self.assets.query(cx).unwrap_or_default();
        if query == self.assets.library.icons_for && self.assets.library.icons.is_some() {
            return;
        }
        self.assets.library.icons_for = query.clone();
        if query.is_empty() {
            self.assets.library.icons = None;
            cx.notify();
            return;
        }
        self.assets.library.icons_loading = true;
        self.library_task(
            cx,
            move || library::library_icon_search(&query),
            |editor, result, _| {
                editor.assets.library.icons_loading = false;
                editor.assets.library.icons = Some(result);
            },
        );
        cx.notify();
    }

    fn render_icons(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let head = Self::hint("Icons from open sets (MIT, Apache, CC0, CC BY), drawn white.");
        let body: AnyElement = match &self.assets.library.icons {
            _ if self.assets.library.icons_loading => {
                notice("Searching\u{2026}", TEXT_MUTED).into_any_element()
            }
            None => notice(
                "Search for a word: heart, arrow, star, fire\u{2026}",
                TEXT_MUTED,
            )
            .into_any_element(),
            Some(Err(error)) => notice(error.clone(), DANGER).into_any_element(),
            Some(Ok(hits)) if hits.is_empty() => {
                notice("No icon matches.", TEXT_MUTED).into_any_element()
            }
            Some(Ok(hits)) => {
                let hits = hits.clone();
                let tiles: Vec<AnyElement> = hits
                    .into_iter()
                    .map(|hit| self.icon_tile(hit, cx))
                    .collect();
                sticker_grid(tiles).into_any_element()
            }
        };
        column_with(
            head,
            Self::tile_area("icon-grid").child(div().flex().flex_col().child(body)),
        )
    }

    fn icon_tile(&mut self, icon: IconHit, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("icon:{}", icon.id());
        let for_thumb = icon.clone();
        let picture = self.library_pic(
            key.clone(),
            move || library::library_icon_thumb(&for_thumb),
            || div().size_full().into_any_element(),
            cx,
        );
        let busy = self.assets.library.busy.contains(&key);
        let tooltip = format!(
            "{} \u{b7} {} ({}){}",
            icon.name,
            icon.set.name,
            icon.set.licence.name,
            if icon.set.policy == Policy::Warn {
                ": share-alike, the video takes this licence"
            } else if icon.set.licence.attribution_required {
                ": credit goes into the credits file"
            } else {
                ""
            }
        );
        let add = icon.clone();
        sticker_tile(
            SharedString::from(key.clone()),
            picture,
            busy,
            cx.listener(move |this, _, _, cx| {
                let icon = add.clone();
                this.add_sticker(key.clone(), move || library::library_icon_fetch(&icon), cx)
            }),
        )
        .tooltip(move |window, cx| {
            gpui::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .into_any_element()
    }
}

fn column_with(head: impl IntoElement, body: impl IntoElement) -> AnyElement {
    div()
        .flex_1()
        .min_h(px(0.0))
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(head)
        .child(body)
        .into_any_element()
}

fn sticker_grid(tiles: Vec<AnyElement>) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(8.0))
        .pt(px(2.0))
        .pl(px(2.0))
        .children(tiles)
}

/// A square tile: the sticker on a raised well, a "+" on hover, the whole
/// tile clickable.
fn sticker_tile(
    id: SharedString,
    picture: AnyElement,
    busy: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let group = SharedString::from(format!("{id}-group"));
    div()
        .id(id)
        .group(group.clone())
        .relative()
        .size(px(STICKER_TILE))
        .p(px(8.0))
        .rounded(px(R_SM))
        .bg(rgb(PANEL_RAISED))
        .border_1()
        .border_color(rgb(HAIRLINE))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(BORDER_STRONG)))
        .on_click(on_click)
        .child(picture)
        .child(
            div()
                .absolute()
                .right(px(4.0))
                .bottom(px(4.0))
                .size(px(18.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(ACCENT))
                .when(!busy, |b| b.invisible().group_hover(group, |s| s.visible()))
                .child(if busy {
                    div()
                        .text_size(px(TEXT_BADGE))
                        .text_color(rgb(ON_ACCENT))
                        .child("\u{2026}")
                        .into_any_element()
                } else {
                    icons::glyph(icons::PLUS, 12.0, rgb(ON_ACCENT)).into_any_element()
                }),
        )
}
