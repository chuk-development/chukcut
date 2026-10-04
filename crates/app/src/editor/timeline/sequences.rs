//! Timelines and compound clips on the timeline panel: the tab row above the
//! lanes (one tab per timeline, "+" for a new one), the breadcrumbs that
//! replace it inside a compound clip, the clip menu's compound entries, and
//! how a compound clip is drawn.
//!
//! Every action is a command in `modules/sequence/commands.rs`; opening and
//! closing a compound clip are edits on the undo stack like any other
//! (decision 0024), so this file only picks what to act on and where the
//! playhead goes afterwards.

use chukcut_engine::modules::sequence::{self, build as seq_build, commands as seq};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::dialog::DialogFooter;
use gpui::component::input::{Input, InputState};
use gpui::component::menu::PopupMenu;
use gpui::component::WindowExt as _;
use gpui::{ClickEvent, Focusable as _};

use super::*;

actions!(
    chukcut,
    [
        CreateCompound,
        OpenCompound,
        FlattenCompound,
        CloseCompound,
        NewTimeline
    ]
);

/// Stacked frames: a compound clip.
pub(crate) const COMPOUND_GLYPH: ui::icons::Glyph = ui::icons::Glyph(
    br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="8.5" width="13" height="11" rx="2.5"/><path d="M7.5 5.5h9.5a3 3 0 0 1 3 3V16"/></svg>"#,
);

/// The height of the tab row.
const BAR_H: f32 = 26.0;

/// CapCut's keys: Alt+G makes a compound clip, Alt+Shift+G takes it apart.
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    const TYPING_OFF: Option<&str> = Some("!Input");
    vec![
        KeyBinding::new("alt-g", CreateCompound, TYPING_OFF),
        KeyBinding::new("alt-shift-g", FlattenCompound, TYPING_OFF),
    ]
}

/// What the clip menu's compound entries may do for the clicked selection.
#[derive(Clone, Copy, Default)]
pub(crate) struct MenuFlags {
    can_create: bool,
    is_compound: bool,
    inside: bool,
}

pub(crate) fn compound_menu(menu: PopupMenu, f: MenuFlags) -> PopupMenu {
    let menu = menu
        .separator()
        .menu_with_disabled(
            "Create compound clip",
            Box::new(CreateCompound),
            !f.can_create,
        )
        .menu_with_disabled("Open compound clip", Box::new(OpenCompound), !f.is_compound)
        .menu_with_disabled("Put clips back", Box::new(FlattenCompound), !f.is_compound);
    if f.inside {
        menu.menu("Close compound clip", Box::new(CloseCompound))
    } else {
        menu
    }
}

/// Whether `segment_id` on the open sequence is a compound clip.
pub(crate) fn is_compound_clip(project: &Project, segment_id: &str) -> bool {
    project
        .segment(segment_id)
        .is_some_and(|(_, s)| project.materials.sequence(&s.material_id).is_some())
}

impl Editor {
    pub(crate) fn compound_menu_flags(&self) -> MenuFlags {
        let ids = self.selection();
        let unlocked = ids.iter().all(|id| {
            self.project
                .segment(id)
                .is_some_and(|(track, _)| !track.locked)
        });
        MenuFlags {
            can_create: !ids.is_empty() && unlocked,
            is_compound: self
                .selected
                .as_deref()
                .is_some_and(|id| is_compound_clip(&self.project, id)),
            inside: !self.project.sequence.path.is_empty(),
        }
    }

    pub(crate) fn sequence_actions(&self, root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(cx.listener(|this, _: &CreateCompound, _, cx| this.create_compound(cx)))
            .on_action(cx.listener(|this, _: &OpenCompound, _, cx| this.open_compound(None, cx)))
            .on_action(cx.listener(|this, _: &FlattenCompound, _, cx| this.flatten_compound(cx)))
            .on_action(cx.listener(|this, _: &CloseCompound, _, cx| this.close_compound(None, cx)))
            .on_action(cx.listener(|this, _: &NewTimeline, _, cx| this.new_timeline(cx)))
    }

    /// After the open sequence changed: nothing selected, the view from the
    /// start unless the caller places the playhead.
    fn after_switch(&mut self, at: Micros, cx: &mut Context<Self>) {
        self.refresh(cx);
        self.clear_selection();
        self.timeline.selected_transition = None;
        self.timeline.selected_keyframe = None;
        self.timeline.inline_text = None;
        self.seek(at);
        let x = self.time_to_x(at);
        let (lanes_w, _) = self.lanes_size();
        if x < 0.0 || x > lanes_w {
            self.timeline.scroll_x =
                (at as f32 / 1e6 * self.timeline.zoom - lanes_w * 0.1).max(0.0);
            self.clamp_scroll();
        }
        cx.notify();
    }

    fn create_compound(&mut self, cx: &mut Context<Self>) {
        let ids = self.selection();
        if ids.is_empty() {
            self.status = Some("Select the clips to put in a compound clip".into());
            cx.notify();
            return;
        }
        self.pause();
        match seq::sequence_compound_create(&self.state, ids, None) {
            Ok(made) => {
                self.refresh(cx);
                self.select_only(&made.segment_id);
                self.report(Ok(()), cx);
            }
            Err(error) => {
                self.refresh(cx);
                self.report(Err(friendly(&error)), cx);
            }
        }
    }

    /// Open `segment_id`, or the selected clip, or the compound clip under
    /// the playhead — and keep the playhead on the same frame inside it.
    pub(crate) fn open_compound(&mut self, segment_id: Option<String>, cx: &mut Context<Self>) {
        let at = self.clock.position();
        let id = segment_id
            .or_else(|| {
                self.selected
                    .clone()
                    .filter(|id| is_compound_clip(&self.project, id))
            })
            .or_else(|| {
                self.project
                    .tracks
                    .iter()
                    .filter(|t| !t.hidden)
                    .filter_map(|t| t.segment_at(at))
                    .find(|s| self.project.materials.sequence(&s.material_id).is_some())
                    .map(|s| s.id.clone())
            });
        let Some(id) = id else {
            self.status = Some("Select a compound clip to open".into());
            cx.notify();
            return;
        };
        let inner = self
            .project
            .segment(&id)
            .map(|(_, s)| {
                let map = self.project.materials.time_map(s);
                map.source_time_at(at)
                    .unwrap_or_else(|| map.clamped_source_time(at))
            })
            .unwrap_or(0);
        self.pause();
        let result = seq::sequence_compound_open(&self.state, id).map(|_| ());
        let ok = result.is_ok();
        self.report(result.map_err(|e| friendly(&e)), cx);
        if ok {
            self.after_switch(inner, cx);
        }
    }

    /// Close the open compound clip, or go out to breadcrumb `level`, and put
    /// the playhead where the frame on screen sits outside.
    pub(crate) fn close_compound(&mut self, level: Option<usize>, cx: &mut Context<Self>) {
        let depth = self.project.sequence.path.len();
        if depth == 0 {
            return;
        }
        let level = level.unwrap_or(depth - 1).min(depth - 1);
        // From the open sequence out to the level asked for, innermost last.
        let mut chain: Vec<String> = self.project.sequence.path[level..].to_vec();
        chain.push(self.project.sequence.id.clone());
        let mut at = self.clock.position();
        self.pause();
        let result = seq::sequence_compound_close_to(&self.state, level).map(|_| ());
        if result.is_ok() {
            if let Some(project) = self.state.project.read().as_ref() {
                // Out one level at a time: each compound clip maps the time
                // one step further out.
                for pair in chain.windows(2).rev() {
                    at = outer_time_in(project, &pair[0], &pair[1], at);
                }
            }
        }
        let ok = result.is_ok();
        self.report(result.map_err(|e| friendly(&e)), cx);
        if ok {
            self.after_switch(at, cx);
        }
    }

    fn flatten_compound(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self
            .selected
            .clone()
            .filter(|id| is_compound_clip(&self.project, id))
        else {
            self.status = Some("Select a compound clip to put its clips back".into());
            cx.notify();
            return;
        };
        self.pause();
        let result = seq::sequence_compound_flatten(&self.state, id).map(|_| ());
        self.refresh(cx);
        self.clear_selection();
        self.report(result.map_err(|e| friendly(&e)), cx);
    }

    fn switch_timeline(&mut self, id: String, cx: &mut Context<Self>) {
        self.pause();
        let result = seq::sequence_timeline_switch(&self.state, id).map(|_| ());
        let ok = result.is_ok();
        self.report(result.map_err(|e| friendly(&e)), cx);
        if ok {
            self.after_switch(0, cx);
        }
    }

    fn new_timeline(&mut self, cx: &mut Context<Self>) {
        self.pause();
        let result = seq::sequence_timeline_new(&self.state, None).map(|_| ());
        let ok = result.is_ok();
        self.report(result.map_err(|e| friendly(&e)), cx);
        if ok {
            self.after_switch(0, cx);
        }
    }

    fn duplicate_timeline(&mut self, id: String, cx: &mut Context<Self>) {
        let result = seq::sequence_timeline_duplicate(&self.state, id).map(|_| ());
        self.refresh(cx);
        self.report(result.map_err(|e| friendly(&e)), cx);
    }

    fn delete_timeline(&mut self, id: String, cx: &mut Context<Self>) {
        let was_open = sequence::root_id(&self.project) == id;
        self.pause();
        let result = seq::sequence_timeline_delete(&self.state, id).map(|_| ());
        let ok = result.is_ok();
        self.report(result.map_err(|e| friendly(&e)), cx);
        if ok && was_open {
            self.after_switch(0, cx);
        } else {
            self.refresh(cx);
        }
    }

    fn rename_sequence(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let current = sequence::name_of(&self.project, &id).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let focus = input.read(cx).focus_handle(cx);
        let editor = cx.entity();
        window.open_dialog(cx, move |dialog, _, _| {
            let rename = {
                let (editor, input, id) = (editor.clone(), input.clone(), id.clone());
                move |window: &mut Window, cx: &mut App| {
                    let name = input.read(cx).value().to_string();
                    window.close_dialog(cx);
                    let id = id.clone();
                    editor.update(cx, |editor, cx| {
                        let result = seq::sequence_rename(&editor.state, id, name).map(|_| ());
                        editor.refresh(cx);
                        editor.report(result.map_err(|e| friendly(&e)), cx);
                    });
                }
            };
            let on_enter = rename.clone();
            dialog
                .w(px(360.0))
                .title("Rename timeline")
                .child(Input::new(&input))
                // Enter renames, as the button does.
                .on_ok(move |_, window, cx| {
                    on_enter(window, cx);
                    false
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("rename-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rename-ok")
                                .primary()
                                .label("Rename")
                                .on_click(move |_, window, cx| rename(window, cx)),
                        ),
                )
        });
        window.focus(&focus, cx);
    }

    /// The row between the toolbar and the lanes: the timelines as tabs, or,
    /// inside a compound clip, the way back out.
    pub(crate) fn render_sequence_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let inside = !self.project.sequence.path.is_empty();
        let bar = div()
            .id("sequence-bar")
            .h(px(BAR_H))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .px(px(6.0))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .bg(rgb(PANEL))
            .text_size(px(TEXT_CAPTION));
        if inside {
            return bar.children(self.render_breadcrumbs(cx)).into_any_element();
        }
        let tabs = sequence::timelines(&self.project);
        let many = tabs.len() > 1;
        bar.children(tabs.into_iter().enumerate().map(|(index, tab)| {
            let id = tab.id.clone();
            let (switch_id, rename_id, dup_id, del_id) =
                (id.clone(), id.clone(), id.clone(), id.clone());
            let editor = cx.entity();
            div()
                .id(SharedString::from(format!("timeline-tab-{index}")))
                .h(px(BAR_H - 6.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(R_SM))
                .cursor_pointer()
                .when(tab.active, |this| {
                    this.bg(rgb(PANEL_RAISED))
                        .text_color(rgb(TEXT))
                        .border_b_2()
                        .border_color(rgb(ACCENT))
                })
                .when(!tab.active, |this| {
                    this.text_color(rgb(TEXT_DIM))
                        .hover(|style| style.bg(rgb(PANEL_RAISED)))
                })
                .child(tab.name.clone())
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if event.click_count() == 2 {
                        this.rename_sequence(switch_id.clone(), window, cx);
                    } else if sequence::root_id(&this.project) != switch_id {
                        this.switch_timeline(switch_id.clone(), cx);
                    }
                }))
                .context_menu(move |menu, _, _| {
                    let (e1, e2, e3) = (editor.clone(), editor.clone(), editor.clone());
                    let (rename_id, dup_id, del_id) =
                        (rename_id.clone(), dup_id.clone(), del_id.clone());
                    menu.item(
                        gpui::component::menu::PopupMenuItem::new("Rename…").on_click(
                            move |_, window, cx| {
                                let id = rename_id.clone();
                                e1.update(cx, |this, cx| this.rename_sequence(id, window, cx));
                            },
                        ),
                    )
                    .item(
                        gpui::component::menu::PopupMenuItem::new("Duplicate").on_click(
                            move |_, _, cx| {
                                let id = dup_id.clone();
                                e2.update(cx, |this, cx| this.duplicate_timeline(id, cx));
                            },
                        ),
                    )
                    .item(
                        gpui::component::menu::PopupMenuItem::new("Delete")
                            .disabled(!many)
                            .on_click(move |_, _, cx| {
                                let id = del_id.clone();
                                e3.update(cx, |this, cx| this.delete_timeline(id, cx));
                            }),
                    )
                })
        }))
        .child(
            tool_button(
                "timeline-new",
                IconSrc::Glyph(ui::icons::PLUS),
                "New timeline",
                "",
            )
            .on_click(cx.listener(|this, _, _, cx| this.new_timeline(cx))),
        )
        .into_any_element()
    }

    fn render_breadcrumbs(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let crumbs = sequence::breadcrumbs(&self.project);
        let last = crumbs.len() - 1;
        let mut out: Vec<gpui::AnyElement> = vec![tool_button(
            "compound-close",
            IconSrc::Glyph(ui::icons::CHEVRON_LEFT),
            "Close compound clip",
            "",
        )
        .on_click(cx.listener(|this, _, _, cx| this.close_compound(None, cx)))
        .into_any_element()];
        for (level, (_, name)) in crumbs.into_iter().enumerate() {
            if level > 0 {
                out.push(
                    ui::icons::glyph(ui::icons::CHEVRON_RIGHT, 12.0, rgb(TEXT_MUTED))
                        .into_any_element(),
                );
            }
            let current = level == last;
            let crumb = div()
                .id(SharedString::from(format!("crumb-{level}")))
                .h(px(BAR_H - 6.0))
                .px(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .rounded(px(R_SM))
                .when(level > 0, |this| {
                    this.child(ui::icons::glyph(
                        COMPOUND_GLYPH,
                        12.0,
                        rgb(if current {
                            CLIP_COMPOUND_TITLE
                        } else {
                            TEXT_MUTED
                        }),
                    ))
                })
                .child(name);
            out.push(if current {
                crumb.text_color(rgb(TEXT)).into_any_element()
            } else {
                crumb
                    .text_color(rgb(TEXT_DIM))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL_RAISED)))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.close_compound(Some(level), cx)),
                    )
                    .into_any_element()
            });
        }
        out
    }

    /// Start rendering compound clip strip `key` (`sequence::thumbs::
    /// strip_key`) at `count` tiles, unless it is there, on its way, or every
    /// worker is busy. The render runs on the background executor against a
    /// snapshot of the document.
    fn request_compound_strip(&mut self, key: &str, count: usize, cx: &mut Context<Self>) {
        let Some(job) = self.timeline.media.wanted_strip(key, count) else {
            return;
        };
        let project = Arc::clone(&self.project);
        let sequence_id = sequence::thumbs::strip_sequence(key).to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(
                    async move { media_cache::load_compound_strip(&project, &sequence_id, count) },
                )
                .await;
            let _ = this.update(cx, |editor, cx| {
                for image in editor.timeline.media.finish_strip(job, result) {
                    cx.drop_image(image, None);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A compound clip's body: a filmstrip of what it shows — its sequence
    /// rendered, overlays and titles included, as the preview draws it — and
    /// a band at the foot with its lanes drawn as bars, so it reads as a
    /// stack of clips rather than one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_compound_clip(
        &mut self,
        mut body: gpui::Div,
        segment: &Segment,
        source: TimeRange,
        width: f32,
        height: f32,
        x0: f32,
        lanes_w: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        const BAND_H: f32 = 10.0;
        let speed = if segment.speed.is_finite() && segment.speed > 0.0 {
            segment.speed
        } else {
            1.0
        };
        let zoom = self.timeline.zoom;
        let thumbs_h = (height - TITLE_H - BAND_H).max(4.0);
        let aspect = self.project.canvas.width as f32 / self.project.canvas.height.max(1) as f32;
        let tile_w = (thumbs_h * aspect).max(8.0);

        // The sequence rendered, as a strip over its whole length, keyed by
        // a digest of its contents so an edit inside renders a new one.
        let key = sequence::thumbs::strip_key(&self.project, &segment.material_id);
        let duration = sequence::duration_of(&self.project, &segment.material_id).unwrap_or(0);
        let mut slots: Vec<(f32, Arc<RenderImage>)> = Vec::new();
        if let Some(key) = key.filter(|_| duration > 0) {
            let count = media_cache::strip_count(duration, zoom / speed, tile_w);
            self.request_compound_strip(&key, count, cx);
            // On a speed curve the tiles follow the curve, as the picture does.
            let map = self.project.materials.time_map(segment);
            let shift = source.start - segment.source_range.start;
            let first = ((-x0) / tile_w).floor().max(0.0) as i64;
            let last = ((lanes_w - x0) / tile_w).ceil() as i64;
            if let Some(strip) = self.timeline.media.strip(&key, count) {
                let n = strip.tiles.len();
                for k in first..=last {
                    let slot = k as f32 * tile_w;
                    if slot >= width || n == 0 {
                        break;
                    }
                    let offset = (slot as f64 / zoom as f64 * 1e6) as Micros;
                    let at = if map.is_curved() {
                        map.source_at(offset) + shift
                    } else {
                        source.start + (offset as f64 * speed as f64) as Micros
                    };
                    let index =
                        ((at.max(0) as f64 / duration as f64 * n as f64) as usize).min(n - 1);
                    slots.push((slot, Arc::clone(&strip.tiles[index])));
                }
            }
        }
        body = body.child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    for (slot, image) in &slots {
                        let tile = Bounds::new(
                            point(bounds.origin.x + px(*slot), bounds.origin.y),
                            size(px(tile_w), bounds.size.height),
                        );
                        let _ = window.paint_image(
                            bounds,
                            tile,
                            Corners::default(),
                            Arc::clone(image),
                            0,
                            false,
                        );
                    }
                },
            )
            .absolute()
            .left(px(0.0))
            .top(px(TITLE_H))
            .w_full()
            .h(px(thumbs_h)),
        );

        // The lanes inside, as bars: where each inner clip sits in the part
        // of the sequence this clip shows.
        let mut bars: Vec<(f32, f32, f32, u32)> = Vec::new();
        if let Some(tracks) = sequence::tracks_of(&self.project, &segment.material_id) {
            let rows = tracks.len().max(1) as f32;
            let row_h = ((BAND_H - 2.0) / rows).max(1.0);
            for (i, track) in tracks.iter().rev().enumerate() {
                let color = match track.kind {
                    TrackKind::Audio => CLIP_AUDIO_WAVE,
                    TrackKind::Text => CLIP_TEXT_TITLE,
                    TrackKind::Effect | TrackKind::Sticker => CLIP_EFFECT_TITLE,
                    TrackKind::Video => CLIP_VIDEO_TITLE,
                };
                for s in &track.segments {
                    let a = s.target_range.start.max(source.start);
                    let b = s.target_range.end().min(source.end());
                    if b <= a {
                        continue;
                    }
                    let left = (a - source.start) as f32 / 1e6 / speed * zoom;
                    let right = (b - source.start) as f32 / 1e6 / speed * zoom;
                    bars.push((left, (right - left).max(1.0), 1.0 + i as f32 * row_h, color));
                }
            }
            let row_h = ((BAND_H - 2.0) / rows).max(1.0) - 0.5;
            body = body.child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .bottom(px(0.0))
                    .w_full()
                    .h(px(BAND_H))
                    .bg(rgb(CLIP_COMPOUND_TITLE))
                    .children(bars.into_iter().map(|(left, w, top, color)| {
                        div()
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .w(px(w))
                            .h(px(row_h.max(1.0)))
                            .rounded(px(1.0))
                            .bg(rgb(color))
                    })),
            );
        }
        // A second outline just inside the first: a stack, at a glance.
        body.child(
            div()
                .absolute()
                .left(px(2.0))
                .top(px(2.0))
                .right(px(2.0))
                .bottom(px(2.0))
                .rounded(px(R_SM))
                .border_1()
                .border_color(with_alpha(CLIP_COMPOUND_EDGE, 0.7)),
        )
    }
}

/// The time in sequence `parent` of time `at` inside sequence `inner`,
/// through the first compound clip of `inner` there that shows that instant
/// (or the first one at all, clamped to what it shows).
fn outer_time_in(project: &Project, parent: &str, inner: &str, at: Micros) -> Micros {
    let Some(tracks) = sequence::tracks_of(project, parent) else {
        return at;
    };
    let clips: Vec<&Segment> = tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .filter(|s| s.material_id == inner)
        .collect();
    let showing = clips
        .iter()
        .find(|s| s.source_range.start <= at && at < s.source_range.end())
        .or_else(|| clips.first());
    match showing {
        Some(s) => seq_build::outer_time(s, at.clamp(s.source_range.start, s.source_range.end()))
            .clamp(s.target_range.start, s.target_range.end()),
        None => at,
    }
}
