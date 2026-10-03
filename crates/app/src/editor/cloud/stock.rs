//! The Stock tab: Pexels and Pixabay videos and photos, Freesound sounds.
//!
//! Search on Enter (Pexels allows 200 searches an hour; a search per
//! keystroke would spend it in minutes). A hit's "+" downloads it into the
//! library cache with its `asset.json` and puts it at the playhead; clicking
//! the tile only adds it to the library. Every tile shows its licence before
//! anything is downloaded.

use chukcut_engine::modules::cloud::audition;
use chukcut_engine::modules::cloud::registry::ProviderKind;
use chukcut_engine::modules::cloud::stock::{self as engine_stock, StockHit, StockQuery};
use chukcut_engine::modules::workspace::paths;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::Input;
use gpui::component::switch::Switch;
use gpui::component::{Disableable as _, Sizable as _};
use gpui::{img, ObjectFit};

use super::*;
use crate::ui::{icons, Badge, Tone};

/// The Stock tab's categories.
pub(crate) const STOCK_CATEGORIES: &[&str] = &["Videos", "Photos", "Sounds"];

fn kind_of(category: usize) -> StockKind {
    match category {
        0 => StockKind::Video,
        1 => StockKind::Photo,
        _ => StockKind::Sound,
    }
}

fn slot(kind: StockKind) -> &'static str {
    match kind {
        StockKind::Video => "stock-video",
        StockKind::Photo => "stock-photo",
        StockKind::Sound => "stock-sound",
    }
}

/// Whether accounts of `provider` have `kind`; known without a request.
fn offers(provider: ProviderKind, kind: StockKind) -> bool {
    match provider {
        ProviderKind::Pexels | ProviderKind::Pixabay => kind != StockKind::Sound,
        ProviderKind::Freesound => kind == StockKind::Sound,
        _ => false,
    }
}

const STOCK_TILE_W: f32 = 148.0;
const STOCK_TILE_H: f32 = 96.0;

impl Editor {
    pub(crate) fn render_stock_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.assets.cloud.refresh_accounts();
        let kind = kind_of(category);
        let able: Vec<AccountView> = self
            .assets
            .cloud
            .able(Capability::StockSearch)
            .into_iter()
            .filter(|a| offers(a.account.kind, kind))
            .collect();
        let Some(account) = self.assets.cloud.chosen_of(slot(kind), &able) else {
            return no_account(
                "stock-empty",
                match kind {
                    StockKind::Sound => "Stock sounds",
                    _ => "Stock videos and photos",
                },
                match kind {
                    StockKind::Sound => "Freesound",
                    _ => "Pexels or Pixabay",
                },
                cx,
            );
        };
        // A new category or account: the last answer is not for it.
        if self.assets.cloud.stock_kind_seen != Some(kind) {
            self.assets.cloud.stock_kind_seen = Some(kind);
            self.assets.cloud.stock_page = None;
        }
        let cloud = &self.assets.cloud;
        let loading = cloud.stock_loading;
        let header = column()
            .gap(px(8.0))
            .child(
                row()
                    .child(label("Source"))
                    .child(account_picker(slot(kind), able, &account, cx))
                    .when(kind == StockKind::Sound, |r| {
                        r.child(
                            Switch::new("stock-nc")
                                .checked(cloud.stock_nc)
                                .label("Show non-commercial")
                                .small()
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.assets.cloud.stock_nc = *checked;
                                    this.assets.cloud.stock_page_no = 1;
                                    this.stock_search(cx);
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&cloud.stock_query).small().cleanable(true)),
                    )
                    .child(
                        Button::new("stock-search")
                            .label(if loading {
                                "Searching\u{2026}"
                            } else {
                                "Search"
                            })
                            .small()
                            .primary()
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.assets.cloud.stock_page_no = 1;
                                this.stock_search(cx);
                            })),
                    ),
            );

        let body = match &cloud.stock_page {
            None => hint(if loading {
                "Searching\u{2026}"
            } else {
                "Search to see results. Everything you add keeps its licence and credit in the project."
            })
            .into_any_element(),
            Some(Err(error)) => div()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(DANGER))
                .child(error.clone())
                .into_any_element(),
            Some(Ok(page)) if page.hits.is_empty() => hint("Nothing found.").into_any_element(),
            Some(Ok(page)) => {
                let page = page.clone();
                let tiles: Vec<AnyElement> = page
                    .hits
                    .iter()
                    .map(|hit| {
                        if hit.kind == StockKind::Sound {
                            self.stock_sound_row(hit, cx)
                        } else {
                            self.stock_tile(hit, cx)
                        }
                    })
                    .collect();
                let list = if kind == StockKind::Sound {
                    column().gap(px(6.0)).children(tiles)
                } else {
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(10.0))
                        .children(tiles)
                };
                let attribution_url = page.attribution_url.clone();
                column()
                    .gap(px(10.0))
                    .child(list)
                    .child(
                        row()
                            .child(
                                div()
                                    .id("stock-attribution")
                                    .cursor_pointer()
                                    .text_size(px(TEXT_CAPTION))
                                    .text_color(rgb(ACCENT))
                                    .hover(|s| s.text_color(rgb(ACCENT_HOVER)))
                                    .on_click(move |_, _, cx| cx.open_url(&attribution_url))
                                    .child(page.attribution.clone()),
                            )
                            .child(div().flex_1())
                            .when(page.has_more, |r| {
                                r.child(
                                    Button::new("stock-more")
                                        .label("Next page")
                                        .small()
                                        .disabled(loading)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.assets.cloud.stock_page_no += 1;
                                            this.stock_search(cx);
                                        })),
                                )
                            })
                            .when(self.assets.cloud.stock_page_no > 1, |r| {
                                r.child(
                                    Button::new("stock-first")
                                        .label("First page")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.assets.cloud.stock_page_no = 1;
                                            this.stock_search(cx);
                                        })),
                                )
                            }),
                    )
                    .into_any_element()
            }
        };
        div()
            .id("stock-tab")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(
                column()
                    .gap(px(12.0))
                    .pb(px(12.0))
                    .child(header)
                    .child(body),
            )
            .into_any_element()
    }

    pub(crate) fn stock_search(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.assets.cloud.stock_kind_seen else {
            return;
        };
        let able: Vec<AccountView> = self
            .assets
            .cloud
            .able(Capability::StockSearch)
            .into_iter()
            .filter(|a| offers(a.account.kind, kind))
            .collect();
        let Some(account) = self.assets.cloud.chosen_of(slot(kind), &able) else {
            return;
        };
        let text = self
            .assets
            .cloud
            .stock_query
            .read(cx)
            .value()
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        let mut query = StockQuery::new(text, kind);
        query.page = self.assets.cloud.stock_page_no;
        query.include_non_commercial = self.assets.cloud.stock_nc;
        self.assets.cloud.stock_loading = true;
        let id = account.account.id.clone();
        self.cloud_task(
            cx,
            move || cloud_commands::cloud_stock_search(&CloudStore::user(), &id, &query),
            move |editor, result, _| {
                editor.assets.cloud.stock_loading = false;
                // A different category may be open by now.
                if editor.assets.cloud.stock_kind_seen == Some(kind) {
                    editor.assets.cloud.stock_page = Some(result);
                }
            },
        );
        cx.notify();
    }

    /// The picture behind `url`, fetched once into the cache.
    fn stock_picture(&mut self, url: Option<&str>, cx: &mut Context<Self>) -> AnyElement {
        let placeholder = || {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(icons::glyph(STOCK_GLYPH, 20.0, rgb(TEXT_MUTED)))
                .into_any_element()
        };
        let Some(url) = url else {
            return placeholder();
        };
        match self.assets.cloud.pictures.get(url) {
            Some(Picture::Ready(path)) => img(path.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            Some(_) => placeholder(),
            None => {
                self.assets
                    .cloud
                    .pictures
                    .insert(url.to_string(), Picture::Loading);
                let fetch = url.to_string();
                let key = url.to_string();
                self.cloud_task(
                    cx,
                    move || {
                        engine_stock::cached_preview(
                            &paths::cache_root().join("library").join("thumbs"),
                            &fetch,
                        )
                    },
                    move |editor, result, _| {
                        let picture = match result {
                            Ok(path) => Picture::Ready(path),
                            Err(error) => {
                                tracing::debug!(%error, "no stock thumbnail");
                                Picture::Failed
                            }
                        };
                        editor.assets.cloud.pictures.insert(key, picture);
                    },
                );
                placeholder()
            }
        }
    }

    fn stock_tile(&mut self, hit: &StockHit, cx: &mut Context<Self>) -> AnyElement {
        let picture = self.stock_picture(hit.thumb_url.as_deref(), cx);
        let key = format!("{}-{}", hit.provider, hit.id);
        let busy = self.assets.cloud.downloading.contains_key(&key);
        let group = SharedString::from(format!("stock-{key}-group"));
        let (add_hit, library_hit) = (hit.clone(), hit.clone());
        let duration = (hit.kind == StockKind::Video && hit.duration_seconds > 0.0)
            .then(|| mmss((hit.duration_seconds * 1_000_000.0) as Micros));
        div()
            .id(SharedString::from(format!("stock-{key}")))
            .group(group.clone())
            .w(px(STOCK_TILE_W))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.stock_fetch(library_hit.clone(), Placement::Library, cx)
            }))
            .child(
                div()
                    .relative()
                    .w(px(STOCK_TILE_W))
                    .h(px(STOCK_TILE_H))
                    .rounded(px(R_SM))
                    .overflow_hidden()
                    .bg(rgb(PANEL_RAISED))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .group_hover(group.clone(), |s| s.border_color(rgb(BORDER_STRONG)))
                    .child(picture)
                    .children(duration.map(|d| {
                        div()
                            .absolute()
                            .left(px(5.0))
                            .bottom(px(5.0))
                            .child(Badge::new(d).tone(Tone::OnMedia).mono())
                    }))
                    .child(
                        div()
                            .id(SharedString::from(format!("stock-add-{key}")))
                            .absolute()
                            .right(px(5.0))
                            .bottom(px(5.0))
                            .size(px(22.0))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(ACCENT))
                            .when(!busy, |b| {
                                b.invisible().group_hover(group.clone(), |s| s.visible())
                            })
                            .hover(|s| s.bg(rgb(ACCENT_HOVER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let at = this.clock.position();
                                this.stock_fetch(add_hit.clone(), Placement::At(at), cx)
                            }))
                            .child(if busy {
                                div()
                                    .text_size(px(TEXT_BADGE))
                                    .text_color(rgb(ON_ACCENT))
                                    .child("\u{2026}")
                                    .into_any_element()
                            } else {
                                icons::glyph(icons::PLUS, 14.0, rgb(ON_ACCENT)).into_any_element()
                            }),
                    ),
            )
            .child(
                div()
                    .w(px(STOCK_TILE_W))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_DIM))
                    .child(if hit.creator.is_empty() {
                        hit.title.clone()
                    } else {
                        hit.creator.clone()
                    }),
            )
            .into_any_element()
    }

    fn stock_sound_row(&mut self, hit: &StockHit, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("{}-{}", hit.provider, hit.id);
        let busy = self.assets.cloud.downloading.contains_key(&key);
        let preview = hit
            .preview_url
            .clone()
            .unwrap_or_else(|| hit.download_url.clone());
        let add_hit = hit.clone();
        let badge = super::super::accounts::licence_badge(
            hit.licence.commercial,
            hit.licence.attribution_required,
        );
        div()
            .id(SharedString::from(format!("stock-{key}")))
            .px(px(8.0))
            .py(px(6.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .rounded(px(R_SM))
            .bg(rgb(PANEL_RAISED))
            .border_1()
            .border_color(rgb(HAIRLINE))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(TEXT_BODY))
                            .child(hit.title.clone()),
                    )
                    .child(
                        row()
                            .child(badge)
                            .child(Badge::new(hit.licence.name.clone()))
                            .child(hint(format!(
                                "{} \u{b7} {}",
                                hit.creator,
                                mmss((hit.duration_seconds * 1_000_000.0) as Micros)
                            ))),
                    ),
            )
            .child(
                Button::new(SharedString::from(format!("stock-play-{key}")))
                    .label("Play")
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, _| audition::play(&preview)),
            )
            .child(
                Button::new(SharedString::from(format!("stock-add-{key}")))
                    .label(if busy {
                        "Adding\u{2026}"
                    } else {
                        "Add at playhead"
                    })
                    .xsmall()
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let at = this.clock.position();
                        this.stock_fetch(add_hit.clone(), Placement::At(at), cx)
                    })),
            )
            .into_any_element()
    }

    /// Download `hit` with its licence record, then import and place it.
    fn stock_fetch(&mut self, hit: StockHit, placement: Placement, cx: &mut Context<Self>) {
        let key = format!("{}-{}", hit.provider, hit.id);
        if self.assets.cloud.downloading.contains_key(&key) {
            return;
        }
        self.assets
            .cloud
            .downloading
            .insert(key.clone(), placement.clone());
        self.status = Some(format!("Downloading {}\u{2026}", hit.title).into());
        self.cloud_task(
            cx,
            move || cloud_commands::cloud_stock_download(&hit, &|_| {}, &AtomicBool::new(false)),
            move |editor, result, cx| {
                let placement = editor
                    .assets
                    .cloud
                    .downloading
                    .remove(&key)
                    .unwrap_or(Placement::Library);
                match result {
                    Ok(path) => editor.cloud_import(path, placement, Vec::new(), cx),
                    Err(error) => editor.report(Err(error), cx),
                }
            },
        );
        cx.notify();
    }
}
