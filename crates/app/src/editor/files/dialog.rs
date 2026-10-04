//! The built-in file browser: what [`super::choose`] shows when the desktop
//! has no file chooser portal.
//!
//! A modal in the app's own design: places and recent folders on the left,
//! the folder's contents on the right (folders first, then the files the
//! request takes), a path field to type or paste a location into, and for a
//! Save the file name. Double-click opens a folder or picks a file; Enter in
//! the path field goes there, or picks the file it names.

use std::ops::Range;
use std::path::{Path, PathBuf};

use futures_channel::oneshot;
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputEvent, InputState};
use gpui::component::switch::Switch;
use gpui::component::{Disableable as _, Icon, Sizable as _, WindowExt as _};
use gpui::prelude::*;
use gpui::{
    div, px, rgb, uniform_list, App, ClickEvent, Context, Entity, Focusable as _, SharedString,
    Subscription, Window,
};

use super::browse::{self, Entry, Filter, Mode, Place};
use super::{home, FileRequest};
use crate::theme::*;
use crate::ui::IconButton;

const ROW: f32 = 28.0;
const WIDTH: f32 = 820.0;
const LIST_H: f32 = 380.0;

/// Open the browser over whatever is on screen (a dialog included).
pub(super) fn open(
    request: FileRequest,
    start: PathBuf,
    answer: oneshot::Sender<Option<Vec<PathBuf>>>,
    window: &mut Window,
    cx: &mut App,
) {
    let title = request.title.clone();
    let browser = cx.new(|cx| FileBrowser::new(request, start, answer, window, cx));
    // A Save starts in the name, everything else in the path field.
    let field = {
        let browser = browser.read(cx);
        browser.name.clone().unwrap_or_else(|| browser.path.clone())
    };
    let focus = field.read(cx).focus_handle(cx);
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(px(WIDTH))
            .p_0()
            .bg(rgb(PANEL))
            .border_color(rgb(BORDER))
            .title(
                div()
                    .px(px(PAD))
                    .pt(px(10.0))
                    .text_size(px(TEXT_DISPLAY))
                    .child(title.clone()),
            )
            // Enter belongs to the path and name fields (go there, save);
            // the dialog's own Enter-to-confirm would close it unanswered.
            .on_ok(|_, _, _| false)
            .child(browser.clone())
    });
    window.focus(&focus, cx);
}

/// The file whose replacement a Save is waiting to have confirmed.
type Replace = Option<PathBuf>;

pub(super) struct FileBrowser {
    request: FileRequest,
    dir: PathBuf,
    entries: Result<Vec<Entry>, String>,
    /// Selected files, in the order they were picked.
    selected: Vec<PathBuf>,
    /// Where a Shift+click range starts.
    anchor: Option<usize>,
    path: Entity<InputState>,
    name: Option<Entity<InputState>>,
    hidden: bool,
    /// Show every file, not only the ones the request takes.
    all_files: bool,
    places: Vec<Place>,
    recent: Vec<PathBuf>,
    replace: Replace,
    notice: Option<SharedString>,
    answer: Option<oneshot::Sender<Option<Vec<PathBuf>>>>,
    _subscriptions: Vec<Subscription>,
}

pub(super) fn recent_file() -> PathBuf {
    chukcut_engine::modules::workspace::paths::config_root().join("recent-folders.json")
}

impl FileBrowser {
    fn new(
        request: FileRequest,
        start: PathBuf,
        answer: oneshot::Sender<Option<Vec<PathBuf>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let path = cx.new(|cx| {
            InputState::new(window, cx).default_value(start.to_string_lossy().to_string())
        });
        let mut subscriptions = vec![cx.subscribe_in(
            &path,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let typed = input.read(cx).value().to_string();
                    this.go_typed(&typed, window, cx);
                }
            },
        )];
        let name = match &request.mode {
            Mode::Save { name } => {
                let input = cx.new(|cx| InputState::new(window, cx).default_value(name.clone()));
                subscriptions.push(cx.subscribe_in(
                    &input,
                    window,
                    |this, _, event: &InputEvent, window, cx| match event {
                        InputEvent::PressEnter { .. } => this.accept(window, cx),
                        InputEvent::Change => {
                            this.replace = None;
                            cx.notify();
                        }
                        _ => {}
                    },
                ));
                Some(input)
            }
            _ => None,
        };
        let home = home();
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let mut browser = Self {
            request,
            dir: start.clone(),
            entries: Ok(Vec::new()),
            selected: Vec::new(),
            anchor: None,
            path,
            name,
            hidden: false,
            all_files: false,
            places: browse::places(&home, &config),
            recent: browse::load_recent(&recent_file()),
            replace: None,
            notice: None,
            answer: Some(answer),
            _subscriptions: subscriptions,
        };
        browser.reload();
        browser
    }

    fn filter(&self) -> Filter {
        if self.all_files {
            Filter::Any
        } else {
            self.request.filter
        }
    }

    fn reload(&mut self) {
        self.entries = browse::list(&self.dir, &self.request.mode, self.filter(), self.hidden)
            .map_err(|error| format!("Cannot read {}: {error}", self.dir.display()));
    }

    /// Show `dir`.
    fn go(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.dir = dir;
        self.selected.clear();
        self.anchor = None;
        self.replace = None;
        self.notice = None;
        self.reload();
        let text = self.dir.to_string_lossy().to_string();
        self.path
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    /// Enter in the path field: a folder is opened, a file is picked (or, for
    /// a Save, named), anything else is said.
    fn go_typed(&mut self, typed: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target = browse::resolve_typed(typed, &self.dir, &home());
        if target.is_dir() {
            self.go(target, window, cx);
            return;
        }
        match &self.request.mode {
            Mode::Save { .. } => {
                // A full path to a new file: its folder, and its name below.
                let (Some(parent), Some(file)) = (target.parent(), target.file_name()) else {
                    return;
                };
                if parent.is_dir() {
                    let file = file.to_string_lossy().to_string();
                    self.go(parent.to_path_buf(), window, cx);
                    if let Some(name) = &self.name {
                        name.update(cx, |input, cx| input.set_value(file, window, cx));
                    }
                    self.accept(window, cx);
                    return;
                }
            }
            Mode::Open { .. } if target.is_file() => {
                self.finish(Some(vec![target]), window, cx);
                return;
            }
            _ => {}
        }
        self.notice = Some(format!("{} does not exist", target.display()).into());
        cx.notify();
    }

    fn up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(parent) = self.dir.parent() {
            let parent = parent.to_path_buf();
            self.go(parent, window, cx);
        }
    }

    fn click_row(
        &mut self,
        index: usize,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(entries) = &self.entries else { return };
        let Some(entry) = entries.get(index).cloned() else {
            return;
        };
        if entry.is_dir {
            // A click selects a folder (what Choose folder takes), a double
            // click opens it.
            if event.click_count() >= 2 {
                self.go(entry.path, window, cx);
            } else {
                self.selected = vec![entry.path];
                self.anchor = Some(index);
                cx.notify();
            }
            return;
        }
        if event.click_count() >= 2 {
            match &self.request.mode {
                Mode::Open { .. } => self.finish(Some(vec![entry.path]), window, cx),
                Mode::Save { .. } => {
                    self.set_name(&entry.name, window, cx);
                    self.accept(window, cx);
                }
                Mode::Folder => {}
            }
            return;
        }
        let modifiers = event.modifiers();
        let multiple = matches!(self.request.mode, Mode::Open { multiple: true });
        if multiple && modifiers.shift {
            let from = self.anchor.unwrap_or(index);
            let (a, b) = (from.min(index), from.max(index));
            self.selected = entries[a..=b]
                .iter()
                .filter(|e| !e.is_dir)
                .map(|e| e.path.clone())
                .collect();
        } else if multiple && (modifiers.control || modifiers.platform) {
            if let Some(at) = self.selected.iter().position(|p| *p == entry.path) {
                self.selected.remove(at);
            } else {
                self.selected.retain(|p| !p.is_dir());
                self.selected.push(entry.path);
            }
            self.anchor = Some(index);
        } else {
            self.selected = vec![entry.path];
            self.anchor = Some(index);
        }
        if let Mode::Save { .. } = self.request.mode {
            self.set_name(&entry.name, window, cx);
        }
        cx.notify();
    }

    fn set_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.name {
            let name = name.to_string();
            input.update(cx, |input, cx| input.set_value(name, window, cx));
        }
        self.replace = None;
    }

    /// The main button.
    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.request.mode {
            Mode::Open { .. } => {
                let files: Vec<PathBuf> = self
                    .selected
                    .iter()
                    .filter(|p| p.is_file())
                    .cloned()
                    .collect();
                if let [folder] = self.selected.as_slice() {
                    if folder.is_dir() {
                        let folder = folder.clone();
                        self.go(folder, window, cx);
                        return;
                    }
                }
                if !files.is_empty() {
                    self.finish(Some(files), window, cx);
                }
            }
            Mode::Folder => {
                // The selected folder, or the one on screen.
                let folder = self
                    .selected
                    .first()
                    .filter(|p| p.is_dir())
                    .cloned()
                    .unwrap_or_else(|| self.dir.clone());
                self.finish(Some(vec![folder]), window, cx);
            }
            Mode::Save { .. } => {
                let typed = self
                    .name
                    .as_ref()
                    .map(|n| n.read(cx).value().to_string())
                    .unwrap_or_default();
                let Some(target) = browse::save_target(&self.dir, &typed, self.request.filter)
                else {
                    self.notice = Some(if typed.contains('/') {
                        "A file name cannot contain /".into()
                    } else {
                        "Type a file name".into()
                    });
                    cx.notify();
                    return;
                };
                if target.is_dir() {
                    self.go(target, window, cx);
                    return;
                }
                if target.exists() && self.replace.as_ref() != Some(&target) {
                    // The first press asks; the button now says Replace.
                    self.notice = Some(
                        format!(
                            "{} already exists. Press Replace to overwrite it.",
                            target.file_name().unwrap_or_default().to_string_lossy()
                        )
                        .into(),
                    );
                    self.replace = Some(target);
                    cx.notify();
                    return;
                }
                self.finish(Some(vec![target]), window, cx);
            }
        }
    }

    fn finish(&mut self, paths: Option<Vec<PathBuf>>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(paths) = &paths {
            // Remember the folder the choice was made in.
            let folder = match (&self.request.mode, paths.first()) {
                (Mode::Folder, Some(p)) => p.clone(),
                (_, Some(p)) => p.parent().map(Path::to_path_buf).unwrap_or_default(),
                _ => self.dir.clone(),
            };
            if folder.is_dir() {
                browse::remember(&mut self.recent, &folder);
                browse::save_recent(&recent_file(), &self.recent);
            }
        }
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(paths);
        }
        window.close_dialog(cx);
    }

    fn primary_label(&self) -> SharedString {
        match &self.request.mode {
            Mode::Save { .. } if self.replace.is_some() => "Replace".into(),
            Mode::Folder => "Choose folder".into(),
            _ => self.request.title.clone(),
        }
    }

    fn can_accept(&self, cx: &App) -> bool {
        match &self.request.mode {
            Mode::Open { .. } => !self.selected.is_empty(),
            Mode::Folder => true,
            Mode::Save { .. } => self
                .name
                .as_ref()
                .is_some_and(|n| !n.read(cx).value().trim().is_empty()),
        }
    }

    // --- drawing ------------------------------------------------------------------

    fn render_place(
        &self,
        id: (&'static str, usize),
        icon: Lucide,
        label: String,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = path == self.dir;
        div()
            .id(id)
            .h(px(ROW))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(R_XS))
            .cursor_pointer()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(if current { TEXT } else { TEXT_DIM }))
            .when(current, |row| row.bg(rgb(PANEL_RAISED)))
            .hover(|style| style.bg(rgb(PANEL_RAISED)).text_color(rgb(TEXT)))
            .child(Icon::new(icon).size(px(14.0)))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(label),
            )
            .on_click(cx.listener(move |this, _, window, cx| this.go(path.clone(), window, cx)))
    }

    fn render_row(&self, index: usize, entry: &Entry, cx: &mut Context<Self>) -> gpui::AnyElement {
        let selected = self.selected.contains(&entry.path);
        let icon = icon_for(entry);
        div()
            .id(("file-row", index))
            .w_full()
            .h(px(ROW))
            .px(px(10.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .rounded(px(R_XS))
            .cursor_pointer()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT))
            .when(selected, |row| row.bg(rgb(OVERLAY)))
            .when(!selected, |row| {
                row.hover(|style| style.bg(rgb(PANEL_RAISED)))
            })
            .child(
                div()
                    .text_color(rgb(if entry.is_dir { ACCENT } else { TEXT_DIM }))
                    .child(Icon::new(icon).size(px(15.0))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(entry.name.clone()),
            )
            .child(
                div()
                    .w(px(80.0))
                    .flex()
                    .justify_end()
                    .font_family(FONT_MONO)
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .when(!entry.is_dir, |cell| {
                        cell.child(browse::size_label(entry.size))
                    }),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.click_row(index, event, window, cx)
            }))
            .into_any_element()
    }
}

impl Render for FileBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let at_root = self.dir.parent().is_none();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                IconButton::new("files-up", Lucide::ArrowUp)
                    .tooltip("Up one folder")
                    .disabled(at_root)
                    .on_click(cx.listener(|this, _, window, cx| this.up(window, cx))),
            )
            .child(div().flex_1().child(Input::new(&self.path).small()))
            .child(
                // The state is in the id so an open tooltip is rebuilt with
                // the new words after a click.
                IconButton::new(
                    SharedString::from(format!("files-hidden-{}", self.hidden)),
                    if self.hidden {
                        Lucide::Eye
                    } else {
                        Lucide::EyeOff
                    },
                )
                .tooltip(if self.hidden {
                    "Hide hidden files"
                } else {
                    "Show hidden files"
                })
                .toggled(self.hidden)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.hidden = !this.hidden;
                    this.reload();
                    cx.notify();
                })),
            );

        let mut side = div()
            .w(px(180.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .pr(px(8.0))
            .border_r_1()
            .border_color(rgb(HAIRLINE));
        let places = self.places.clone();
        for (i, place) in places.into_iter().enumerate() {
            let icon = match place.label.as_str() {
                "Home" => Lucide::House,
                "Computer" => Lucide::HardDrive,
                _ => Lucide::Folder,
            };
            side = side.child(self.render_place(
                ("files-place", i),
                icon,
                place.label,
                place.path,
                cx,
            ));
        }
        if !self.recent.is_empty() {
            side = side.child(
                div()
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .px(px(8.0))
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child("Recent"),
            );
            let recent = self.recent.clone();
            for (i, path) in recent.into_iter().enumerate() {
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                side = side.child(self.render_place(
                    ("files-recent", i),
                    Lucide::Clock,
                    label,
                    path,
                    cx,
                ));
            }
        }

        let list: gpui::AnyElement = match &self.entries {
            Err(error) => div()
                .flex_1()
                .p(px(PAD))
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(DANGER))
                .child(error.clone())
                .into_any_element(),
            Ok(entries) if entries.is_empty() => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(TEXT_LABEL))
                .text_color(rgb(TEXT_MUTED))
                .child(match self.request.mode {
                    Mode::Folder => "No folders here",
                    _ => "Nothing here to choose",
                })
                .into_any_element(),
            Ok(entries) => {
                let rows = entries.clone();
                uniform_list(
                    "file-list",
                    rows.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        range
                            .map(|i| this.render_row(i, &rows[i], cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h(px(0.0))
                .into_any_element()
            }
        };

        let filter_switch =
            (self.request.filter != Filter::Any && self.request.mode != Mode::Folder).then(|| {
                Switch::new("files-all")
                    .checked(self.all_files)
                    .label("Show all files")
                    .xsmall()
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.all_files = *checked;
                        this.reload();
                        cx.notify();
                    }))
            });

        let name_field = self.name.as_ref().map(|name| {
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(NAME_LABEL_W))
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(TEXT_DIM))
                        .child("Name"),
                )
                .child(div().flex_1().child(Input::new(name).small()))
        });

        let accept_enabled = self.can_accept(cx);
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(if self.all_files || self.request.mode == Mode::Folder {
                        SharedString::from("")
                    } else {
                        SharedString::from(self.request.filter.label())
                    }),
            )
            .children(filter_switch)
            .child(
                Button::new("files-cancel")
                    .label("Cancel")
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.finish(None, window, cx))),
            )
            .child(
                Button::new("files-accept")
                    .primary()
                    .small()
                    .label(self.primary_label())
                    .disabled(!accept_enabled)
                    .on_click(cx.listener(|this, _, window, cx| this.accept(window, cx))),
            );

        div()
            .px(px(PAD))
            .pb(px(PAD))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(toolbar)
            .child(
                div().h(px(LIST_H)).flex().gap(px(8.0)).child(side).child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .rounded(px(R_SM))
                        .bg(rgb(WELL))
                        .p(px(4.0))
                        .child(list),
                ),
            )
            .children(self.notice.clone().map(|notice| {
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(WARNING))
                    .child(notice)
            }))
            .children(name_field)
            .child(footer)
    }
}

/// The width of the Name label in a Save.
const NAME_LABEL_W: f32 = 40.0;

/// The row's icon: by what the file is, not by the request.
fn icon_for(entry: &Entry) -> Lucide {
    const STILLS: &[&str] = &[
        "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "avif", "heic",
    ];
    const SOUNDS: &[&str] = &[
        "mp3", "wav", "flac", "aac", "m4a", "ogg", "oga", "opus", "wma", "aif", "aiff",
    ];
    if entry.is_dir {
        return Lucide::Folder;
    }
    if Filter::Projects.accepts(&entry.path) {
        return Lucide::Clapperboard;
    }
    if Filter::Subtitles.accepts(&entry.path) {
        return Lucide::Captions;
    }
    if !Filter::Media.accepts(&entry.path) {
        return Lucide::File;
    }
    let ext = entry
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if STILLS.contains(&ext.as_str()) {
        Lucide::Image
    } else if SOUNDS.contains(&ext.as_str()) {
        Lucide::Music
    } else {
        Lucide::Film
    }
}
