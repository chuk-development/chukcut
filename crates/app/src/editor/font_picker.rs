//! The font picker the title and caption style controls open.
//!
//! One list over two sources: the families the text renderer can draw now
//! (the system's, and those the library installed) under "Installed", and
//! the Fontsource catalogue by category. Every row shows the family's name
//! in its own face — a PNG the engine draws, so the picker shows exactly the
//! face the export will use. A catalogue family is downloaded and registered
//! when it is picked; the pick is then the family name, the same as for a
//! system font, so nothing downstream knows where a font came from.
//!
//! The list is a `uniform_list`, so only the rows on screen ask for their
//! preview: scrolling through two thousand families costs a request per row
//! seen, not two thousand.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use chukcut_engine::modules::library::commands::{self as library, FontList};
use chukcut_engine::modules::library::fonts::{FontCategory, FontEntry};
use chukcut_engine::shell::spawn_blocking;
use gpui::assets::IconName as Lucide;
use gpui::component::button::Button;
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::popover::Popover;
use gpui::component::Sizable as _;
use gpui::prelude::*;
use gpui::{
    div, img, px, rgb, uniform_list, App, Context, Entity, EventEmitter, ObjectFit, SharedString,
    Subscription, Window,
};

use crate::theme::*;
use crate::ui::{Badge, IconSrc, Tone};

/// A family was chosen. The family name, as the text renderer knows it.
pub(crate) struct FontPicked(pub String);

/// A row's picture.
enum Preview {
    Loading,
    /// The PNG and its size in pixels.
    Ready(PathBuf, (u32, u32)),
    Failed,
}

/// One row of the list.
#[derive(Clone)]
enum Row {
    /// A family the renderer has.
    Installed(String),
    /// A catalogue family.
    Catalogue(FontEntry),
}

impl Row {
    fn family(&self) -> &str {
        match self {
            Row::Installed(name) => name,
            Row::Catalogue(entry) => &entry.family,
        }
    }

    fn key(&self) -> String {
        match self {
            Row::Installed(name) => format!("sys:{name}"),
            Row::Catalogue(entry) => format!("lib:{}", entry.id),
        }
    }
}

/// The category column: "Installed", then the catalogue's.
const TABS: [&str; 8] = [
    "Installed",
    "Popular",
    "All",
    "Sans",
    "Serif",
    "Display",
    "Hand",
    "Mono",
];

fn catalogue_category(tab: usize) -> Option<FontCategory> {
    tab.checked_sub(1).map(|i| FontCategory::ALL[i])
}

/// The row height, and the height a preview is shown at.
const ROW: f32 = 34.0;
const PREVIEW_H: f32 = 20.0;
const WIDTH: f32 = 380.0;

pub(crate) struct FontPicker {
    /// Whether the popover is open; the owner reads it when it draws the
    /// button.
    pub(crate) open: bool,
    /// The family in use, checked in the list.
    pub(crate) current: String,
    search: Entity<InputState>,
    tab: usize,
    installed: Option<Vec<String>>,
    /// Library families by name, to badge them among the installed ones.
    library_families: HashSet<String>,
    catalogue: Option<Result<Arc<FontList>, String>>,
    loading_catalogue: bool,
    previews: HashMap<String, Preview>,
    installing: HashSet<String>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FontPicked> for FontPicker {}

impl FontPicker {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search fonts"));
        let subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        Self {
            open: false,
            current: String::new(),
            search,
            tab: 1,
            installed: None,
            library_families: HashSet::new(),
            catalogue: None,
            loading_catalogue: false,
            previews: HashMap::new(),
            installing: HashSet::new(),
            error: None,
            _subscriptions: vec![subscription],
        }
    }

    /// The installed families, read off the UI thread once per opening.
    fn load_installed(&mut self, cx: &mut Context<Self>) {
        if self.installed.is_some() {
            return;
        }
        self.installed = Some(Vec::new());
        let task = spawn_blocking(|| {
            let families = library::library_system_fonts();
            let library: HashSet<String> = library::library_fonts_installed()
                .into_iter()
                .map(|f| f.family)
                .collect();
            (families, library)
        });
        cx.spawn(async move |this, cx| {
            if let Ok((families, library)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.installed = Some(families);
                    this.library_families = library;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn load_catalogue(&mut self, cx: &mut Context<Self>) {
        if self.catalogue.as_ref().is_some_and(|c| c.is_ok()) || self.loading_catalogue {
            return;
        }
        self.loading_catalogue = true;
        let task = spawn_blocking(library::library_font_catalogue);
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                this.loading_catalogue = false;
                this.catalogue = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let query = self.search.read(cx).value().trim().to_lowercase();
        match catalogue_category(self.tab) {
            None => self
                .installed
                .iter()
                .flatten()
                .filter(|name| query.is_empty() || name.to_lowercase().contains(&query))
                .map(|name| Row::Installed(name.clone()))
                .collect(),
            Some(category) => match &self.catalogue {
                Some(Ok(list)) => {
                    chukcut_engine::modules::library::fonts::search(&list.fonts, &query, category)
                        .into_iter()
                        .map(Row::Catalogue)
                        .collect()
                }
                _ => Vec::new(),
            },
        }
    }

    /// The row's preview, asked for the first time it is drawn.
    fn preview(&mut self, row: &Row, cx: &mut Context<Self>) -> Option<(PathBuf, (u32, u32))> {
        let key = row.key();
        match self.previews.get(&key) {
            Some(Preview::Ready(path, size)) => return Some((path.clone(), *size)),
            Some(_) => return None,
            None => {}
        }
        self.previews.insert(key.clone(), Preview::Loading);
        let row = row.clone();
        let task = spawn_blocking(move || {
            let path = match &row {
                Row::Installed(name) => library::library_font_system_preview(name),
                Row::Catalogue(entry) => library::library_font_preview(entry),
            }?;
            let size = image::image_dimensions(&path).map_err(|e| e.to_string())?;
            Ok::<_, String>((path, size))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
            let _ = this.update(cx, |this, cx| {
                let preview = match result {
                    Ok((path, size)) => Preview::Ready(path, size),
                    Err(error) => {
                        tracing::debug!(%error, "no font preview");
                        Preview::Failed
                    }
                };
                this.previews.insert(key, preview);
                cx.notify();
            });
        })
        .detach();
        None
    }

    fn pick(&mut self, row: Row, cx: &mut Context<Self>) {
        match row {
            Row::Installed(name) => {
                self.choose(name, cx);
            }
            Row::Catalogue(entry) => {
                let installed = self
                    .installed
                    .as_ref()
                    .is_some_and(|names| names.contains(&entry.family));
                if installed {
                    self.choose(entry.family.clone(), cx);
                    return;
                }
                if !self.installing.insert(entry.id.clone()) {
                    return;
                }
                self.error = None;
                let id = entry.id.clone();
                let task = spawn_blocking(move || library::library_font_install(&entry));
                cx.spawn(async move |this, cx| {
                    let result = task.await.unwrap_or_else(|e| Err(e.to_string()));
                    let _ = this.update(cx, |this, cx| {
                        this.installing.remove(&id);
                        match result {
                            Ok(done) => {
                                let family = if done.families.contains(&done.font.family) {
                                    done.font.family.clone()
                                } else {
                                    done.families[0].clone()
                                };
                                // The installed list is read again next time.
                                this.installed = None;
                                this.choose(family, cx);
                            }
                            Err(error) => {
                                this.error = Some(error);
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
                cx.notify();
            }
        }
    }

    fn choose(&mut self, family: String, cx: &mut Context<Self>) {
        self.current = family.clone();
        self.open = false;
        cx.emit(FontPicked(family));
        cx.notify();
    }

    fn render_row(&mut self, index: usize, row: Row, cx: &mut Context<Self>) -> gpui::AnyElement {
        let family = row.family().to_string();
        let preview = self.preview(&row, cx);
        let current = family == self.current;
        let busy = matches!(&row, Row::Catalogue(e) if self.installing.contains(&e.id));
        let installed_here = match &row {
            Row::Installed(name) => self.library_families.contains(name),
            Row::Catalogue(entry) => self
                .installed
                .as_ref()
                .is_some_and(|names| names.contains(&entry.family)),
        };
        let name: gpui::AnyElement = match preview {
            Some((path, (w, h))) if h > 0 => {
                let width = (PREVIEW_H * w as f32 / h as f32).min(WIDTH - 120.0);
                img(path)
                    .h(px(PREVIEW_H))
                    .w(px(width))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            }
            _ => div()
                .text_size(px(TEXT_BODY))
                .text_color(rgb(TEXT_DIM))
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
                .child(family.clone())
                .into_any_element(),
        };
        let badge = match (&row, busy, installed_here) {
            (_, true, _) => Some(Badge::new("Downloading\u{2026}").tone(Tone::Accent)),
            (Row::Installed(_), _, true) => Some(Badge::new("Library")),
            (Row::Catalogue(_), _, true) => Some(Badge::new("Installed").tone(Tone::Success)),
            (Row::Catalogue(entry), _, false) => Some(Badge::new(
                if entry.license.eq_ignore_ascii_case("OFL-1.1") {
                    "OFL".to_string()
                } else {
                    entry.license.clone()
                },
            )),
            _ => None,
        };
        let pick_row = row.clone();
        div()
            .id(("font-row", index))
            .h(px(ROW))
            .px(px(10.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .rounded(px(R_SM))
            .cursor_pointer()
            .when(current, |r| r.bg(rgb(PANEL_RAISED)))
            .hover(|s| s.bg(rgb(PANEL_RAISED)))
            .on_click(cx.listener(move |this, _, _, cx| this.pick(pick_row.clone(), cx)))
            .child(div().flex_1().min_w(px(0.0)).overflow_hidden().child(name))
            .children(badge)
            .when(current, |r| {
                r.child(IconSrc::from(Lucide::Check).svg(14.0, rgb(ACCENT)))
            })
            .into_any_element()
    }
}

impl Render for FontPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_installed(cx);
        if catalogue_category(self.tab).is_some() {
            self.load_catalogue(cx);
        }
        let rows = self.rows(cx);
        let count = rows.len();
        let rows = Arc::new(rows);

        let tabs = TABS.iter().enumerate().map(|(index, label)| {
            let active = index == self.tab;
            div()
                .id(("font-tab", index))
                .h(px(24.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .rounded(px(R_SM))
                .cursor_pointer()
                .text_size(px(TEXT_LABEL))
                .when(active, |t| {
                    t.bg(rgb(PANEL_RAISED))
                        .text_color(rgb(TEXT))
                        .font_weight(gpui::FontWeight::MEDIUM)
                })
                .when(!active, |t| {
                    t.text_color(rgb(TEXT_DIM))
                        .hover(|s| s.text_color(rgb(TEXT)))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = index;
                    cx.notify();
                }))
                .child(*label)
        });

        let status: Option<String> = match (catalogue_category(self.tab), &self.catalogue) {
            (Some(_), Some(Err(error))) => Some(error.clone()),
            (Some(_), None) | (Some(_), Some(Ok(_))) if self.loading_catalogue => {
                Some("Loading the font list\u{2026}".into())
            }
            (Some(_), Some(Ok(list))) if list.stale => {
                Some("Offline: showing the font list from the last time.".into())
            }
            (_, _) if count == 0 && self.installed.is_some() => Some("No font matches.".into()),
            _ => None,
        };

        let list = uniform_list(
            "font-list",
            count,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                range
                    .map(|i| this.render_row(i, rows[i].clone(), cx))
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h(px(0.0));

        div()
            .w(px(WIDTH))
            .h(px(440.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                Input::new(&self.search)
                    .small()
                    .cleanable(true)
                    .text_size(px(TEXT_BODY))
                    .bg(rgb(WELL))
                    .border_color(rgb(BORDER))
                    .prefix(IconSrc::from(Lucide::Search).svg(14.0, rgb(TEXT_MUTED))),
            )
            .child(div().flex().flex_row().flex_wrap().gap(px(2.0)).children(tabs))
            .children(self.error.clone().map(|error| {
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(DANGER))
                    .child(error)
            }))
            .children(status.map(|text| {
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(text)
            }))
            .child(list)
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(if catalogue_category(self.tab).is_some() {
                        "Fonts from Fontsource, free for any video (OFL). Picked fonts download once."
                    } else {
                        "Fonts on this computer and those downloaded from the library."
                    }),
            )
    }
}

/// The button that shows the current family and opens `picker` under it.
/// `label` is what the button says (the family, or "Default").
pub(crate) fn font_button(
    id: impl Into<SharedString>,
    label: String,
    picker: &Entity<FontPicker>,
    cx: &App,
) -> impl IntoElement {
    let id: SharedString = id.into();
    let open = picker.read(cx).open;
    let content = picker.clone();
    let toggle = picker.clone();
    Popover::new(id.clone())
        .anchor(gpui::Anchor::TopLeft)
        .open(open)
        .on_open_change(move |open, _, cx| {
            toggle.update(cx, |picker, cx| {
                picker.open = *open;
                if *open {
                    // Fresh each time: a font may have been installed since.
                    picker.installed = None;
                }
                cx.notify();
            });
            // The button belongs to whichever view owns the picker; it has to
            // draw again for the popover to follow `open`.
            cx.refresh_windows();
        })
        .trigger(
            Button::new(SharedString::from(format!("{id}-button")))
                .label(label)
                .small()
                .outline()
                .dropdown_caret(true),
        )
        .content(move |_, _, _| content.clone())
}
