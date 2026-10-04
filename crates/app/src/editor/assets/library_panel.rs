//! The built-in library's state in the asset panel, and the pieces its tabs
//! share: a background loader for pictures, a chip, an audio row.
//!
//! Presentation over `chukcut_engine::modules::library`: the engine fetches,
//! caches and records licences; this file keeps what the panel has seen and
//! runs the engine's blocking calls off the UI thread.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chukcut_engine::modules::grading::presets::PresetEntry;
use chukcut_engine::modules::library::commands::{self as library};
use chukcut_engine::modules::library::looks::LookEntry;
use chukcut_engine::modules::library::sounds::{Sound, Track};
use chukcut_engine::modules::library::stickers::{IconHit, StickerIndex, StickerStyle};
use chukcut_engine::shell::spawn_blocking;
use gpui::{img, ObjectFit};

use super::*;
use crate::editor::font_picker::{FontPicked, FontPicker};

/// A picture the panel is fetching, has, or could not get.
pub(crate) enum Pic {
    Loading,
    Ready(PathBuf),
    Failed,
}

/// A sound-effect pack being opened, open, or failed.
pub(crate) enum PackState {
    Loading,
    Ready(Vec<Sound>),
    Failed(String),
}

/// The library's part of the asset panel.
pub(crate) struct LibraryPanel {
    /// The picker the title inspector opens.
    pub(crate) title_fonts: Entity<FontPicker>,
    /// The picker the caption style opens.
    pub(crate) caption_fonts: Entity<FontPicker>,

    pub(super) sticker_index: Option<Result<Arc<StickerIndex>, String>>,
    pub(super) sticker_loading: bool,
    pub(super) sticker_style: StickerStyle,
    /// How many stickers of the category are drawn; "Show more" raises it.
    pub(super) sticker_shown: usize,
    /// Noto Animated Emoji and whether the list is an old copy.
    pub(super) animated: Option<Result<AnimatedList, String>>,
    pub(super) animated_loading: bool,
    pub(super) icons: Option<Result<Vec<IconHit>, String>>,
    /// The query the icons answer.
    pub(super) icons_for: String,
    pub(super) icons_loading: bool,
    /// Bumped on every keystroke, so only the last one searches.
    pub(super) icon_typing: u64,

    pub(super) pics: HashMap<String, Pic>,
    /// Items being fetched to go on the timeline or to play.
    pub(super) busy: HashSet<String>,

    pub(super) music_all: Option<Result<(Vec<Track>, bool), String>>,
    pub(super) music_loading: bool,
    /// Index into the music chips: the moods, then "All tracks".
    pub(super) music_chip: usize,
    /// The key of the item the audition is playing.
    pub(super) playing: Option<String>,

    pub(super) packs: HashMap<&'static str, PackState>,
    pub(super) open_pack: &'static str,

    pub(super) looks: Option<Result<Vec<LookEntry>, String>>,
    pub(super) looks_loading: bool,
    /// The grade presets saved from the Adjust tab, read from disk when the
    /// Filters tab is drawn; `None` after a save, so it reads again.
    pub(crate) grade_presets: Option<Vec<PresetEntry>>,

    _subscriptions: Vec<Subscription>,
}

impl LibraryPanel {
    pub(crate) fn new(
        search: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Editor>,
    ) -> Self {
        let title_fonts = cx.new(|cx| FontPicker::new(window, cx));
        let caption_fonts = cx.new(|cx| FontPicker::new(window, cx));
        let subscriptions = vec![
            cx.subscribe(
                &title_fonts,
                |editor: &mut Editor, _, event: &FontPicked, cx| {
                    editor.use_font_for_title(event.0.clone(), cx);
                },
            ),
            cx.subscribe(
                &caption_fonts,
                |editor: &mut Editor, _, event: &FontPicked, cx| {
                    let family = event.0.clone();
                    editor.restyle_captions(move |s| s.font_family = family, cx);
                },
            ),
            // Icons search Iconify, so they wait for a pause in the typing.
            cx.subscribe(search, |editor: &mut Editor, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    editor.icons_typed(cx);
                }
            }),
        ];
        Self {
            title_fonts,
            caption_fonts,
            sticker_index: None,
            sticker_loading: false,
            sticker_style: StickerStyle::Fluent3d,
            sticker_shown: STICKER_PAGE,
            animated: None,
            animated_loading: false,
            icons: None,
            icons_for: String::new(),
            icons_loading: false,
            icon_typing: 0,
            pics: HashMap::new(),
            busy: HashSet::new(),
            music_all: None,
            music_loading: false,
            music_chip: 0,
            playing: None,
            packs: HashMap::new(),
            open_pack: library::library_sfx_packs()[0].id,
            looks: None,
            looks_loading: false,
            grade_presets: None,
            _subscriptions: subscriptions,
        }
    }
}

/// Stickers drawn per category before "Show more".
pub(super) const STICKER_PAGE: usize = 96;

/// The animated emoji list as the library hands it out.
pub(super) type AnimatedList = Arc<(
    Vec<chukcut_engine::modules::library::animated_emoji::AnimatedEmoji>,
    bool,
)>;

impl Editor {
    /// Run `work` on a blocking thread and hand its answer to `done`.
    pub(super) fn library_task<T: Send + 'static>(
        &mut self,
        cx: &mut Context<Self>,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Editor, T, &mut Context<Editor>) + 'static,
    ) {
        let future = spawn_blocking(work);
        cx.spawn(async move |this, cx| {
            if let Ok(value) = future.await {
                let _ = this.update(cx, |editor, cx| {
                    done(editor, value, cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// A picture from the library: the file once it is on disk, a quiet
    /// placeholder while `load` fetches it, `fallback` when it failed.
    pub(super) fn library_pic(
        &mut self,
        key: String,
        load: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
        fallback: impl FnOnce() -> AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.assets.library.pics.get(&key) {
            // The id is what lets GPUI play an animated GIF tile: an `img`
            // without one keeps no frame state and stays on frame one.
            Some(Pic::Ready(path)) => img(path.clone())
                .id(SharedString::from(format!("pic-{key}")))
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            Some(Pic::Failed) => fallback(),
            Some(Pic::Loading) => div().size_full().into_any_element(),
            None => {
                self.assets.library.pics.insert(key.clone(), Pic::Loading);
                self.library_task(cx, load, move |editor, result, _| {
                    let pic = match result {
                        Ok(path) => Pic::Ready(path),
                        Err(error) => {
                            tracing::debug!(%error, %key, "no library picture");
                            Pic::Failed
                        }
                    };
                    editor.assets.library.pics.insert(key, pic);
                });
                div().size_full().into_any_element()
            }
        }
    }

    /// Set the selected title's font (the title picker's answer).
    pub(crate) fn use_font_for_title(&mut self, family: String, cx: &mut Context<Self>) {
        let Some(material_id) = self
            .selected_segment()
            .map(|(_, segment)| segment.material_id.clone())
            .filter(|id| self.project.materials.text(id).is_some())
        else {
            self.report(Err("Select a title first".into()), cx);
            return;
        };
        let result =
            library::library_font_use_for_title(&self.state, &material_id, &family).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    /// Play `path`, or stop it when it is the one playing.
    pub(super) fn audition(&mut self, key: String, path: PathBuf, cx: &mut Context<Self>) {
        use chukcut_engine::modules::cloud::audition;
        if self.assets.library.playing.as_deref() == Some(key.as_str()) && audition::is_playing() {
            audition::stop();
            self.assets.library.playing = None;
        } else {
            audition::play(&path.to_string_lossy());
            self.assets.library.playing = Some(key);
        }
        cx.notify();
    }

    /// Whether the audition of `key` is what is sounding now.
    pub(super) fn is_auditioning(&self, key: &str) -> bool {
        self.assets.library.playing.as_deref() == Some(key)
            && chukcut_engine::modules::cloud::audition::is_playing()
    }
}

/// A pill that is on or off, as the cloud panels draw them.
pub(super) fn chip(
    id: impl Into<gpui::ElementId>,
    text: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(24.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(R_SM))
        .cursor_pointer()
        .text_size(px(TEXT_LABEL))
        .whitespace_nowrap()
        .border_1()
        .when(active, |pill| {
            pill.bg(accent_soft())
                .border_color(rgb(ACCENT))
                .text_color(rgb(ACCENT))
        })
        .when(!active, |pill| {
            pill.border_color(rgb(BORDER))
                .text_color(rgb(TEXT_DIM))
                .hover(|style| style.bg(rgb(PANEL_RAISED)).text_color(rgb(TEXT)))
        })
        .on_click(on_click)
        .child(text.into())
}

/// A row of chips that wraps.
pub(super) fn chips(children: impl IntoIterator<Item = impl IntoElement>) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(6.0))
        .children(children)
}

/// A line that says why a list is empty or old: a network failure, offline.
pub(super) fn notice(text: impl Into<SharedString>, tone: u32) -> gpui::Div {
    div()
        .text_size(px(TEXT_CAPTION + 1.0))
        .text_color(rgb(tone))
        .child(text.into())
}
