//! Project templates in the app: the gallery (start screen and the asset
//! panel's Templates tab), the fill dialog that makes a project from one,
//! the slots of the open project, and "Save as template".
//!
//! Every action is an engine command (`modules::template::commands`); this
//! file only draws and asks for files.

use std::collections::HashMap;
use std::rc::Rc;

use chukcut_engine::modules::template::commands::{
    self as template_commands, SaveRequest, TemplateInfo,
};
use chukcut_engine::modules::template::slot::{Slot, SlotMedia};
use gpui::assets::IconName as Lucide;
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::input::{Input, InputState};
use gpui::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui::{AnyElement, Entity, EventEmitter, ObjectFit, WeakEntity};

use super::*;
use crate::ui::Badge;

/// The short edge of a rendered preview tile, in pixels.
const TILE_SHORT: u32 = 240;

/// The gallery's filter: everything, one category, or the user's own.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Show {
    All,
    Category(&'static str),
    Mine,
}

/// The categories the built-ins are filed under, in the order a category
/// column lists them.
pub(crate) const CATEGORIES: [&str; 4] = ["Social", "Travel & vlog", "Business", "Cinematic"];

enum Thumb {
    Loading,
    Ready(PathBuf),
    Failed,
}

/// What the gallery tells its owner.
pub(crate) enum GalleryEvent {
    /// A template was clicked: offer to fill it.
    Chosen(TemplateInfo),
}

/// The template list and its preview tiles, loaded off the UI thread. One
/// per surface that shows templates.
pub(crate) struct Gallery {
    list: Option<Vec<TemplateInfo>>,
    thumbs: HashMap<String, Thumb>,
}

impl EventEmitter<GalleryEvent> for Gallery {}

impl Gallery {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let mut gallery = Self {
            list: None,
            thumbs: HashMap::new(),
        };
        gallery.reload(cx);
        gallery
    }

    /// Read the list again: after a save or a delete.
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async { template_commands::template_list() })
                .await;
            let _ = this.update(cx, |gallery, cx| {
                gallery.list = Some(list);
                // A saved template may reuse an id's tile; draw anew.
                gallery.thumbs.clear();
                cx.notify();
            });
        })
        .detach();
    }

    fn thumb(&mut self, id: &str, cx: &mut Context<Self>) -> Option<PathBuf> {
        match self.thumbs.get(id) {
            Some(Thumb::Ready(path)) => return Some(path.clone()),
            Some(_) => return None,
            None => {}
        }
        self.thumbs.insert(id.to_string(), Thumb::Loading);
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let key = id.clone();
            let rendered = cx
                .background_executor()
                .spawn(async move { template_commands::template_thumbnail(&key, TILE_SHORT) })
                .await;
            let thumb = match rendered {
                Ok(path) => Thumb::Ready(path),
                Err(error) => {
                    tracing::warn!(%error, %id, "no template tile");
                    Thumb::Failed
                }
            };
            let _ = this.update(cx, |gallery, cx| {
                gallery.thumbs.insert(id, thumb);
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// The tiles for `show` whose name or description has `query`, each
    /// `width` wide.
    pub(crate) fn tiles(
        &mut self,
        show: &Show,
        query: Option<&str>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(list) = self.list.clone() else {
            return vec![dim_line("Loading templates\u{2026}")];
        };
        let picked: Vec<TemplateInfo> = list
            .into_iter()
            .filter(|t| match show {
                Show::All => true,
                Show::Category(c) => t.builtin && t.category == *c,
                Show::Mine => !t.builtin,
            })
            .filter(|t| {
                query.is_none_or(|q| {
                    t.name.to_lowercase().contains(q) || t.description.to_lowercase().contains(q)
                })
            })
            .collect();
        if picked.is_empty() {
            return vec![dim_line(match show {
                Show::Mine => {
                    "No templates of your own yet. Open a project and use \u{201c}Save as template\u{201d}."
                }
                _ => "No template matches.",
            })];
        }
        picked
            .into_iter()
            .map(|info| self.tile(info, width, cx))
            .collect()
    }

    fn tile(&mut self, info: TemplateInfo, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let height = (width * 1.25).round();
        let thumb = self.thumb(&info.id, cx);
        let id = SharedString::from(format!("template-{}", info.id));
        let group = SharedString::from(format!("template-group-{}", info.id));
        let picture = match thumb {
            Some(path) => img(path)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div().size_full().into_any_element(),
        };
        let slots = info.slots.len();
        let caption = format!(
            "{slots} clip{} \u{b7} {}",
            if slots == 1 { "" } else { "s" },
            clock(info.duration)
        );
        let chosen = info.clone();
        div()
            .id(id)
            .group(group.clone())
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(GalleryEvent::Chosen(chosen.clone()));
            }))
            .child(
                div()
                    .relative()
                    .w(px(width))
                    .h(px(height))
                    .rounded(px(R_SM))
                    .overflow_hidden()
                    .bg(rgb(WELL))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .group_hover(group.clone(), |s| s.border_color(rgb(BORDER_STRONG)))
                    .child(picture)
                    .child(
                        div()
                            .absolute()
                            .left(px(5.0))
                            .bottom(px(5.0))
                            .child(Badge::new(format!(
                                "{}:{}",
                                aspect_of(&info).0,
                                aspect_of(&info).1
                            ))),
                    )
                    .child(
                        div()
                            .absolute()
                            .right(px(5.0))
                            .bottom(px(5.0))
                            .child(Badge::new(clock(info.duration))),
                    ),
            )
            .child(
                div()
                    .w(px(width))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .group_hover(group, |s| s.text_color(rgb(TEXT)))
                    .child(info.name.clone()),
            )
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_MUTED))
                    .child(caption),
            )
            .into_any_element()
    }
}

fn dim_line(text: &str) -> AnyElement {
    div()
        .text_size(px(TEXT_LABEL))
        .text_color(rgb(TEXT_MUTED))
        .child(text.to_string())
        .into_any_element()
}

/// The canvas shape of a template, reduced.
fn aspect_of(info: &TemplateInfo) -> (u32, u32) {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 {
            a
        } else {
            gcd(b, a % b)
        }
    }
    let g = gcd(info.width.max(1), info.height.max(1));
    (info.width.max(1) / g, info.height.max(1) / g)
}

/// `0:12` from microseconds.
pub(crate) fn clock(duration: Micros) -> String {
    let seconds = (duration.max(0) as f64 / 1e6).round() as i64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn seconds_label(duration: Micros) -> String {
    format!("{:.1} s", duration as f64 / 1e6)
}

// ---------------------------------------------------------------------------
// The fill dialog
// ---------------------------------------------------------------------------

/// What the fill dialog asks the owner to do.
#[derive(Debug, Clone)]
pub(crate) struct FillRequest {
    pub template_id: String,
    pub media: Vec<PathBuf>,
    pub name: Option<String>,
}

type OnCreate = Rc<dyn Fn(FillRequest, &mut Window, &mut App)>;

/// Open the fill dialog for `info`. `on_create` gets the files in slot order.
pub(crate) fn open_fill(
    info: TemplateInfo,
    on_create: impl Fn(FillRequest, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let on_create: OnCreate = Rc::new(on_create);
    let dialog = cx.new(|cx| FillDialog::new(info, on_create, window, cx));
    window.open_dialog(cx, move |surface, _, _| {
        surface
            .w(px(620.0))
            .p_0()
            .on_ok(|_, _, _| false)
            .title(div().px_4().pt_3().child("Use template"))
            .child(dialog.clone())
    });
}

struct FillDialog {
    info: TemplateInfo,
    media: Vec<Option<PathBuf>>,
    name: Entity<InputState>,
    on_create: OnCreate,
}

impl FillDialog {
    fn new(
        info: TemplateInfo,
        on_create: OnCreate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).placeholder(info.name.clone()));
        Self {
            media: vec![None; info.slots.len()],
            info,
            name,
            on_create,
        }
    }

    /// Pick several files; they fill the empty slots from `from`, in order.
    fn choose_many(&mut self, from: usize, cx: &mut Context<Self>) {
        let picked = files::choose(
            FileRequest::open_many("Choose clips for the template", Filter::Media),
            cx,
        );
        cx.spawn(async move |this, cx| {
            let Some(paths) = picked.await else {
                return;
            };
            let _ = this.update(cx, |dialog, cx| {
                let mut paths = paths.into_iter();
                for slot in dialog.media.iter_mut().skip(from) {
                    if slot.is_none() {
                        match paths.next() {
                            Some(path) => *slot = Some(path),
                            None => break,
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_one(&mut self, index: usize, cx: &mut Context<Self>) {
        let picked = files::choose_one(FileRequest::open("Choose a clip", Filter::Media), cx);
        cx.spawn(async move |this, cx| {
            if let Some(path) = picked.await {
                let _ = this.update(cx, |dialog, cx| {
                    dialog.media[index] = Some(path);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// The files in slot order. A gap stops the list: the engine fills
    /// slots in order, and a later file must not slide into an earlier slot.
    fn request(&self, cx: &App) -> FillRequest {
        let media = self.media.iter().map_while(|m| m.clone()).collect();
        let typed = self.name.read(cx).value().trim().to_string();
        FillRequest {
            template_id: self.info.id.clone(),
            media,
            name: (!typed.is_empty()).then_some(typed),
        }
    }
}

impl Render for FillDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let filled = self.media.iter().filter(|m| m.is_some()).count();
        let total = self.media.len();
        let gap = self
            .media
            .iter()
            .position(|m| m.is_none())
            .is_some_and(|first_empty| self.media[first_empty..].iter().any(|m| m.is_some()));
        let rows: Vec<AnyElement> = self
            .info
            .slots
            .iter()
            .enumerate()
            .map(|(n, slot)| {
                let chosen = self.media[n].clone();
                let name = chosen
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|f| f.to_string_lossy().into_owned());
                let detail = format!(
                    "{} \u{b7} {}:{} \u{b7} {}",
                    seconds_label(slot.duration),
                    slot.aspect[0],
                    slot.aspect[1],
                    match slot.accepts {
                        SlotMedia::Any => "video or photo",
                        SlotMedia::Video => "video",
                        SlotMedia::Image => "photo",
                    }
                );
                div()
                    .id(SharedString::from(format!("fill-slot-{n}")))
                    .min_h(px(40.0))
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .child(
                        div()
                            .size(px(24.0))
                            .flex_none()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(rgb(if chosen.is_some() {
                                ACCENT
                            } else {
                                PANEL_RAISED
                            }))
                            .text_size(px(TEXT_LABEL))
                            .font_family(FONT_MONO)
                            .text_color(rgb(if chosen.is_some() {
                                ON_ACCENT
                            } else {
                                TEXT_DIM
                            }))
                            .child(slot.index.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_color(rgb(if name.is_some() { TEXT } else { TEXT_DIM }))
                                    .child(name.unwrap_or_else(|| {
                                        slot.label.clone().unwrap_or_else(|| "Empty".into())
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_CAPTION))
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(detail),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("fill-choose-{n}")))
                            .label(if chosen.is_some() {
                                "Change"
                            } else {
                                "Choose\u{2026}"
                            })
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| this.choose_one(n, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("fill-clear-{n}")))
                            .icon(Lucide::X)
                            .xsmall()
                            .ghost()
                            .disabled(chosen.is_none())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.media[n] = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        let create = {
            let on_create = Rc::clone(&self.on_create);
            let request = self.request(cx);
            Button::new("fill-create")
                .label(if filled == total {
                    "Create project".to_string()
                } else {
                    format!("Create with {filled} of {total}")
                })
                .small()
                .primary()
                .disabled(gap)
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    on_create(request.clone(), window, cx);
                })
        };

        div()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_3()
            .text_color(rgb(TEXT))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(TEXT_DISPLAY))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(self.info.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(TEXT_LABEL))
                            .text_color(rgb(TEXT_DIM))
                            .child(format!(
                                "{} \u{b7} {}\u{d7}{} \u{b7} {}",
                                self.info.description,
                                self.info.width,
                                self.info.height,
                                clock(self.info.duration)
                            )),
                    ),
            )
            .child(
                Input::new(&self.name)
                    .small()
                    .bg(rgb(WELL))
                    .border_color(rgb(BORDER)),
            )
            .child(
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .child(format!(
                        "Your clips fill the slots in order: longer ones are trimmed, shorter ones slowed, other shapes cropped. {filled} of {total} chosen."
                    )),
            )
            .child(
                div()
                    .id("fill-slots")
                    .max_h(px(360.0))
                    .overflow_y_scroll()
                    .rounded(px(R_SM))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .children(rows),
            )
            .when(gap, |d| {
                d.child(
                    div()
                        .text_size(px(TEXT_LABEL))
                        .text_color(rgb(WARNING))
                        .child("Fill the slots in order: an empty slot before a chosen one would move the clips up."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("fill-many")
                            .label("Choose several\u{2026}")
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| this.choose_many(0, cx))),
                    )
                    .child(create),
            )
    }
}

// ---------------------------------------------------------------------------
// "Replace media…" from the timeline
// ---------------------------------------------------------------------------

impl Editor {
    /// The clip "Replace media…" fills: the selected clip, when it is a
    /// video or photo clip on an unlocked picture lane. A template slot is
    /// the case it is for; any other picture clip works the same way.
    pub(crate) fn replace_media_target(&self) -> Option<String> {
        let (track, segment) = self
            .selected
            .as_deref()
            .and_then(|id| self.project.segment(id))?;
        let pool = &self.project.materials;
        let picture = pool.video(&segment.material_id).is_some()
            || pool.image(&segment.material_id).is_some();
        (track.kind == TrackKind::Video && !track.locked && picture).then(|| segment.id.clone())
    }

    /// Ask for a file and put it into the selected clip, keeping the clip's
    /// place, length and look: the slots dialog's "Replace…", one undo step.
    pub(crate) fn replace_media(&mut self, cx: &mut Context<Self>) {
        let Some(segment_id) = self.replace_media_target() else {
            self.status = Some("Select a video or photo clip to replace its media".into());
            cx.notify();
            return;
        };
        let state = Arc::clone(&self.state);
        let picked = files::choose_one(FileRequest::open("Replace with", Filter::Media), cx);
        cx.spawn(async move |this, cx| {
            let Some(path) = picked.await else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            let _ = this.update(cx, |editor, cx| {
                editor.status = Some("Replacing media\u{2026}".into());
                cx.notify();
            });
            // Probing the file is IO; the edit itself is quick.
            let result = cx
                .background_executor()
                .spawn(async move {
                    template_commands::template_replace_media(&state, &segment_id, &path, None)
                        .map(|r| r.slowed_to)
                })
                .await;
            let _ = this.update(cx, |editor, cx| {
                editor.refresh(cx);
                editor.status = match result {
                    Ok(Some(speed)) => Some(
                        format!("The new clip is shorter; it plays at {speed:.2}\u{d7}.").into(),
                    ),
                    Ok(None) => None,
                    Err(error) => Some(error.into()),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

// ---------------------------------------------------------------------------
// The open project's slots
// ---------------------------------------------------------------------------

/// Open the list of the open project's slots, each with "Replace…".
pub(crate) fn open_slots(editor: WeakEntity<Editor>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|_| SlotsDialog {
        editor,
        notice: None,
    });
    window.open_dialog(cx, move |surface, _, _| {
        surface
            .w(px(620.0))
            .p_0()
            .title(div().px_4().pt_3().child("Template slots"))
            .child(dialog.clone())
    });
}

struct SlotsDialog {
    editor: WeakEntity<Editor>,
    notice: Option<SharedString>,
}

impl SlotsDialog {
    fn slots(&self, cx: &App) -> Vec<Slot> {
        self.editor
            .upgrade()
            .and_then(|e| template_commands::template_slots(&e.read(cx).state).ok())
            .unwrap_or_default()
    }

    fn replace(&mut self, segment_id: String, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let state = Arc::clone(&editor.read(cx).state);
        let picked = files::choose_one(FileRequest::open("Replace with", Filter::Media), cx);
        cx.spawn(async move |this, cx| {
            let Some(path) = picked.await else {
                return;
            };
            let path = path.to_string_lossy().into_owned();
            // Probing the file is IO; the edit itself is quick.
            let result = cx
                .background_executor()
                .spawn(async move {
                    template_commands::template_replace_media(&state, &segment_id, &path, None)
                        .map(|r| r.slowed_to)
                })
                .await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.notice = match &result {
                    Ok(Some(speed)) => Some(
                        format!("The clip is shorter than the slot; it plays at {speed:.2}\u{d7}.")
                            .into(),
                    ),
                    Ok(None) => None,
                    Err(error) => Some(error.clone().into()),
                };
                if let Some(editor) = dialog.editor.upgrade() {
                    editor.update(cx, |editor, cx| editor.refresh(cx));
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for SlotsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let slots = self.slots(cx);
        let rows: Vec<AnyElement> = slots
            .into_iter()
            .map(|slot| {
                let file = slot
                    .media_path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .map(|f| f.to_string_lossy().into_owned());
                let segment_id = slot.segment_id.clone();
                div()
                    .min_h(px(40.0))
                    .px_3()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .child(
                        div()
                            .w(px(28.0))
                            .text_size(px(TEXT_LABEL))
                            .font_family(FONT_MONO)
                            .text_color(rgb(if slot.filled { ACCENT } else { TEXT_DIM }))
                            .child(slot.index.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(TEXT_BODY))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(file.unwrap_or_else(|| {
                                        slot.label.clone().unwrap_or_else(|| "Empty".into())
                                    })),
                            )
                            .child(
                                div()
                                    .text_size(px(TEXT_CAPTION))
                                    .text_color(rgb(TEXT_MUTED))
                                    .child(format!(
                                        "{} at {} \u{b7} {}:{}",
                                        seconds_label(slot.duration),
                                        seconds_label(slot.start),
                                        slot.aspect[0],
                                        slot.aspect[1]
                                    )),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("slot-replace-{}", slot.index)))
                            .label(if slot.filled {
                                "Replace\u{2026}"
                            } else {
                                "Fill\u{2026}"
                            })
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.replace(segment_id.clone(), cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_3()
            .text_color(rgb(TEXT))
            .children(self.notice.clone().map(|n| {
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(WARNING))
                    .child(n)
            }))
            .child(if rows.is_empty() {
                dim_line("This project has no slots. Projects made from a template have them.")
            } else {
                div()
                    .id("slot-list")
                    .max_h(px(420.0))
                    .overflow_y_scroll()
                    .rounded(px(R_SM))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .children(rows)
                    .into_any_element()
            })
    }
}

// ---------------------------------------------------------------------------
// Save as template
// ---------------------------------------------------------------------------

/// Open "Save as template" for the open project. `slots` are the clips that
/// become slots (the selection, in timeline order); empty keeps the slots
/// the project has. `saved` runs after a successful save.
pub(crate) fn open_save(
    editor: WeakEntity<Editor>,
    slots: Vec<String>,
    saved: impl Fn(&mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let saved: Rc<dyn Fn(&mut App)> = Rc::new(saved);
    let dialog = cx.new(|cx| SaveDialog::new(editor, slots, saved, window, cx));
    window.open_dialog(cx, move |surface, _, _| {
        surface
            .w(px(520.0))
            .p_0()
            .on_ok(|_, _, _| false)
            .title(div().px_4().pt_3().child("Save as template"))
            .child(dialog.clone())
    });
}

struct SaveDialog {
    editor: WeakEntity<Editor>,
    slots: Vec<String>,
    name: Entity<InputState>,
    description: Entity<InputState>,
    notice: Option<SharedString>,
    saved: Rc<dyn Fn(&mut App)>,
}

impl SaveDialog {
    fn new(
        editor: WeakEntity<Editor>,
        slots: Vec<String>,
        saved: Rc<dyn Fn(&mut App)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project_name = editor
            .upgrade()
            .map(|e| e.read(cx).project.name.clone())
            .unwrap_or_default();
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("Template name");
            state.set_value(project_name, window, cx);
            state
        });
        let description =
            cx.new(|cx| InputState::new(window, cx).placeholder("What it is for (optional)"));
        Self {
            editor,
            slots,
            name,
            description,
            notice: None,
            saved,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let state = Arc::clone(&editor.read(cx).state);
        let request = SaveRequest {
            name: self.name.read(cx).value().to_string(),
            description: self.description.read(cx).value().to_string(),
            category: String::new(),
            slots: self.slots.clone(),
            labels: Vec::new(),
        };
        let saved = Rc::clone(&self.saved);
        // Copying the media in can take a moment for long clips; it is
        // blocking file IO, so off the UI thread.
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { template_commands::template_save(&state, &request) })
                .await;
            let _ = this.update_in(cx, |dialog, window, cx| match result {
                Ok(info) => {
                    window.close_dialog(cx);
                    saved(cx);
                    if let Some(editor) = dialog.editor.upgrade() {
                        editor.update(cx, |editor, cx| {
                            editor.status = Some(
                                format!(
                                    "Saved the template \u{201c}{}\u{201d} with {} slot{}",
                                    info.name,
                                    info.slots.len(),
                                    if info.slots.len() == 1 { "" } else { "s" }
                                )
                                .into(),
                            );
                            cx.notify();
                        });
                    }
                }
                Err(error) => {
                    dialog.notice = Some(error.into());
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl Render for SaveDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let what = if self.slots.is_empty() {
            "The project's slots stay slots.".to_string()
        } else {
            format!(
                "The {} selected clip{} become{} slot{}, in timeline order. Media still in use is copied into the template.",
                self.slots.len(),
                if self.slots.len() == 1 { "" } else { "s" },
                if self.slots.len() == 1 { "s" } else { "" },
                if self.slots.len() == 1 { "" } else { "s" },
            )
        };
        div()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_3()
            .text_color(rgb(TEXT))
            .child(
                Input::new(&self.name)
                    .small()
                    .bg(rgb(WELL))
                    .border_color(rgb(BORDER)),
            )
            .child(
                Input::new(&self.description)
                    .small()
                    .bg(rgb(WELL))
                    .border_color(rgb(BORDER)),
            )
            .child(
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(TEXT_DIM))
                    .child(what),
            )
            .children(self.notice.clone().map(|n| {
                div()
                    .text_size(px(TEXT_LABEL))
                    .text_color(rgb(DANGER))
                    .child(n)
            }))
            .child(
                div().flex().flex_row().justify_end().child(
                    Button::new("save-template")
                        .label("Save template")
                        .small()
                        .primary()
                        .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                ),
            )
    }
}

/// The Templates tab's glyph: a frame split into a large and two small
/// slots.
pub(crate) const TEMPLATE_GLYPH: crate::ui::Glyph = crate::ui::Glyph(
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="18" height="18" rx="3"/><path d="M12 3v18"/><path d="M12 12h9"/></svg>"#,
);
