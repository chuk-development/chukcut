//! The editor window: media, preview, timeline.
//!
//! One entity owns the session. The document lives in the engine's
//! [`AppState`]; this view keeps an `Arc` snapshot of it for drawing and for
//! the render thread, and refreshes the snapshot after every edit. Every edit
//! goes through the engine's command layer (`modules/*/commands.rs`), the same
//! functions a CLI or an MCP server calls.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use chukcut_engine::modules::audio::AudioEngine;
use chukcut_engine::modules::preview::clock::{frame_at, PlaybackClock};
use chukcut_engine::modules::project::commands as project_commands;
use chukcut_engine::modules::project::{Micros, Project, Track, TrackKind};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use chukcut_engine::state::AppState;
use gpui::prelude::*;
use gpui::{
    actions, canvas, div, img, px, rgb, App, Bounds, Context, FocusHandle, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, Point, RenderImage,
    ScrollWheelEvent, SharedString, Task, Window,
};

use crate::edits;
use crate::player::Player;

actions!(
    chukcut,
    [
        PlayPause,
        Split,
        DeleteSelected,
        Undo,
        Redo,
        Import,
        Open,
        Save,
        StepBack,
        StepForward,
        GoToStart,
        GoToEnd,
        ZoomIn,
        ZoomOut,
        Quit
    ]
);

// --- look --------------------------------------------------------------------

const BG: u32 = 0x0e0e10;
const PANEL: u32 = 0x18181b;
const PANEL_RAISED: u32 = 0x222226;
const BORDER: u32 = 0x2c2c31;
const TEXT: u32 = 0xe8e8ea;
const TEXT_DIM: u32 = 0x8b8b93;
const ACCENT: u32 = 0x22d3ee;
const PLAYHEAD: u32 = 0xffffff;
const CLIP_VIDEO: u32 = 0x1f6f8b;
const CLIP_IMAGE: u32 = 0x5b4b9a;
const CLIP_AUDIO: u32 = 0x2f7d4f;
const CLIP_TEXT: u32 = 0x9a6b2f;
const CLIP_OTHER: u32 = 0x55555c;

const HEADER_W: f32 = 96.0;
const RULER_H: f32 = 26.0;
const TIMELINE_H: f32 = 280.0;
const MEDIA_W: f32 = 270.0;

fn row_height(kind: TrackKind) -> f32 {
    match kind {
        TrackKind::Video => 58.0,
        TrackKind::Audio => 42.0,
        _ => 34.0,
    }
}

/// How often the view checks the clock and the render thread.
const TICK: Duration = Duration::from_millis(8);

// --- state -------------------------------------------------------------------

enum Drag {
    Scrub,
    Clip {
        segment_id: String,
        kind: TrackKind,
        grab: Micros,
        origin_track: String,
        origin_start: Micros,
        track: String,
        start: Micros,
    },
}

pub struct Editor {
    state: Arc<AppState>,
    audio: Arc<AudioEngine>,
    clock: PlaybackClock,
    player: Player,
    focus: FocusHandle,

    project: Arc<Project>,
    /// Bumped on every edit, so the render request after an edit is never
    /// mistaken for the identical one before it.
    generation: u64,

    frame: Option<Arc<RenderImage>>,
    last_request: Option<(i64, (u32, u32), u64)>,
    scale: f32,

    selected: Option<String>,
    /// Timeline zoom, in pixels per second.
    zoom: f32,
    scroll_x: f32,
    drag: Option<Drag>,
    status: Option<SharedString>,

    viewer: Rc<Cell<Bounds<Pixels>>>,
    timeline: Rc<Cell<Bounds<Pixels>>>,
    _ticker: Task<()>,
}

impl Editor {
    pub fn new(
        state: Arc<AppState>,
        startup: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = Arc::new(
            state
                .project
                .read()
                .clone()
                .expect("main opens a project before the window"),
        );
        let audio = AudioEngine::new();
        audio.set_project(Arc::clone(&project));
        let clock =
            PlaybackClock::with_source(audio.time_source(), project.fps, project.duration());

        let focus = cx.focus_handle();
        window.focus(&focus, cx);

        let ticker = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |editor, cx| editor.tick(cx)).is_err() {
                break;
            }
        });

        let mut editor = Self {
            state,
            audio,
            clock,
            player: Player::new(),
            focus,
            project,
            generation: 0,
            frame: None,
            last_request: None,
            scale: window.scale_factor(),
            selected: None,
            zoom: 60.0,
            scroll_x: 0.0,
            drag: None,
            status: None,
            viewer: Rc::new(Cell::new(Bounds::default())),
            timeline: Rc::new(Cell::new(Bounds::default())),
            _ticker: ticker,
        };
        if !startup.is_empty() {
            editor.import_paths(startup, cx);
        }
        editor
    }

    // --- the clock and the picture --------------------------------------------

    fn tick(&mut self, cx: &mut Context<Self>) {
        let playing = self.clock.is_playing();
        if playing && self.clock.is_at_end() {
            self.pause();
        }
        self.request_frame();

        let mut changed = playing;
        if let Some(frame) = self.player.take() {
            if let Some(old) = self.frame.replace(frame.image) {
                // A frame is uploaded into the window's atlas when drawn; drop
                // the old one or every frame of playback stays resident.
                cx.drop_image(old, None);
            }
            let _ = frame.time;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The size to render at: the canvas fitted into the viewer, in device
    /// pixels, never larger than the canvas itself.
    fn render_size(&self) -> Option<(u32, u32)> {
        let bounds = self.viewer.get();
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if bw < 4.0 || bh < 4.0 {
            return None;
        }
        let (cw, ch) = (
            self.project.canvas.width as f32,
            self.project.canvas.height as f32,
        );
        let fit = (bw / cw).min(bh / ch) * self.scale;
        let fit = fit.min(1.0);
        Some(((cw * fit).round() as u32, (ch * fit).round() as u32))
    }

    fn request_frame(&mut self) {
        let Some(size) = self.render_size() else {
            return;
        };
        let time = self.clock.position();
        let key = (frame_at(time, self.project.fps), size, self.generation);
        if self.last_request == Some(key) {
            return;
        }
        self.last_request = Some(key);
        // Just inside the frame, like the export: see `SAMPLE_SLACK`.
        self.player.request(
            Arc::clone(&self.project),
            time + chukcut_engine::modules::project::SAMPLE_SLACK,
            size,
        );
    }

    fn play(&mut self) {
        if self.project.duration() <= 0 {
            return;
        }
        if self.clock.is_at_end() {
            self.clock.seek(0);
        }
        self.audio.set_project(Arc::clone(&self.project));
        self.audio.play(self.clock.position());
        self.clock.play();
    }

    fn pause(&mut self) {
        self.clock.pause();
        self.audio.pause();
    }

    fn seek(&mut self, to: Micros) {
        let to = to.clamp(0, self.project.duration().max(0));
        self.clock.seek(to);
        if self.clock.is_playing() {
            self.audio.seek(to);
        }
    }

    // --- edits -------------------------------------------------------------------

    /// Take a fresh snapshot of the document after anything changed it.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = self.state.project.read().clone() {
            self.project = Arc::new(project);
        }
        self.generation += 1;
        self.audio.set_project(Arc::clone(&self.project));
        self.clock.retime(self.project.fps, self.project.duration());
        if let Some(id) = &self.selected {
            if self.project.segment(id).is_none() {
                self.selected = None;
            }
        }
        cx.notify();
    }

    fn report(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.status = result.err().map(SharedString::from);
        cx.notify();
    }

    fn apply(&mut self, command: Result<EditCommand, String>, cx: &mut Context<Self>) {
        let result = command
            .and_then(|command| timeline_commands::timeline_apply(&self.state, command))
            .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let state = Arc::clone(&self.state);
        self.status = Some("Importing…".into());
        cx.spawn(async move |this, cx| {
            let mut errors = Vec::new();
            for path in paths {
                let path = path.to_string_lossy().to_string();
                match project_commands::project_import_media(&state, path.clone()).await {
                    Ok(imported) => {
                        let command = state
                            .with_project(|project| edits::append(project, &imported.id))
                            .and_then(|command| command);
                        if let Err(error) =
                            command.and_then(|c| timeline_commands::timeline_apply(&state, c))
                        {
                            errors.push(error);
                        }
                    }
                    Err(error) => errors.push(format!("{path}: {error}")),
                }
            }
            let _ = this.update(cx, |editor, cx| {
                editor.refresh(cx);
                let result = if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors.join(" · "))
                };
                editor.report(result, cx);
            });
        })
        .detach();
    }

    // --- actions -----------------------------------------------------------------

    fn on_play_pause(&mut self, _: &PlayPause, _: &mut Window, cx: &mut Context<Self>) {
        if self.clock.is_playing() {
            self.pause();
        } else {
            self.play();
        }
        cx.notify();
    }

    fn on_split(&mut self, _: &Split, _: &mut Window, cx: &mut Context<Self>) {
        let at = self.clock.position();
        let result = match self.selected.clone() {
            Some(id) => timeline_commands::timeline_split(&self.state, id, at),
            None => timeline_commands::timeline_split_all(&self.state, at),
        }
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    fn on_delete(&mut self, _: &DeleteSelected, _: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.take() else {
            return;
        };
        let command = edits::remove(&self.project, &id);
        self.apply(command, cx);
    }

    fn on_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let result = timeline_commands::timeline_undo(&self.state).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    fn on_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let result = timeline_commands::timeline_redo(&self.state).map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    fn on_import(&mut self, _: &Import, _: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |editor, cx| editor.import_paths(paths, cx));
            }
        })
        .detach();
    }

    fn on_open(&mut self, _: &Open, _: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open project".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |editor, cx| {
                editor.pause();
                let result = project_commands::project_open(
                    &editor.state,
                    path.to_string_lossy().to_string(),
                )
                .map(|_| ());
                editor.selected = None;
                editor.clock.seek(0);
                editor.refresh(cx);
                editor.report(result, cx);
            });
        })
        .detach();
    }

    fn on_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.project_path.read().is_some() {
            let result = project_commands::project_save(&self.state, None).map(|_| ());
            self.status = Some(match &result {
                Ok(()) => "Saved".into(),
                Err(error) => error.clone().into(),
            });
            cx.notify();
            return;
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let name = format!("{}.chukcut", self.project.name);
        let picked = cx.prompt_for_new_path(&home, Some(&name));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |editor, cx| {
                let result = project_commands::project_save(
                    &editor.state,
                    Some(path.to_string_lossy().to_string()),
                );
                editor.status = Some(match result {
                    Ok(path) => format!("Saved {path}").into(),
                    Err(error) => error.into(),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn step(&mut self, frames: i64) {
        let interval = (1_000_000.0 / self.project.fps.max(1.0)) as Micros;
        self.pause();
        self.seek(self.clock.position() + frames * interval);
    }

    fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.zoom = (self.zoom * factor).clamp(2.0, 2000.0);
        cx.notify();
    }

    // --- timeline geometry ----------------------------------------------------------

    fn time_to_x(&self, time: Micros) -> f32 {
        HEADER_W + time as f32 / 1_000_000.0 * self.zoom - self.scroll_x
    }

    fn x_to_time(&self, x: f32) -> Micros {
        (((x - HEADER_W + self.scroll_x) / self.zoom) * 1_000_000.0).max(0.0) as Micros
    }

    /// Timeline-local coordinates of a window position.
    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.timeline.get();
        (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        )
    }

    /// The track under a timeline-local y, if any.
    fn track_at(&self, y: f32) -> Option<&Track> {
        let mut top = RULER_H;
        for track in &self.project.tracks {
            let height = row_height(track.kind);
            if y >= top && y < top + height {
                return Some(track);
            }
            top += height;
        }
        None
    }

    fn on_timeline_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let (x, y) = self.local(event.position);
        let time = self.x_to_time(x);
        if y >= RULER_H && x >= HEADER_W {
            let hit = self.track_at(y).and_then(|track| {
                track
                    .segments
                    .iter()
                    .find(|s| {
                        time >= s.target_range.start
                            && time < s.target_range.start + s.target_range.duration
                    })
                    .map(|s| {
                        (
                            track.id.clone(),
                            track.kind,
                            s.id.clone(),
                            s.target_range.start,
                        )
                    })
            });
            if let Some((track_id, kind, segment_id, start)) = hit {
                self.selected = Some(segment_id.clone());
                self.drag = Some(Drag::Clip {
                    segment_id,
                    kind,
                    grab: time - start,
                    origin_track: track_id.clone(),
                    origin_start: start,
                    track: track_id,
                    start,
                });
                cx.notify();
                return;
            }
            self.selected = None;
        }
        if x >= HEADER_W {
            self.seek(time);
            self.drag = Some(Drag::Scrub);
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none() || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let (x, y) = self.local(event.position);
        let time = self.x_to_time(x);
        let hovered = self.track_at(y).map(|t| (t.id.clone(), t.kind));
        match &mut self.drag {
            Some(Drag::Scrub) => {
                self.seek(time);
            }
            Some(Drag::Clip {
                grab,
                kind,
                track,
                start,
                ..
            }) => {
                *start = (time - *grab).max(0);
                if let Some((id, hovered_kind)) = hovered {
                    if hovered_kind == *kind {
                        *track = id;
                    }
                }
            }
            None => {}
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Drag::Clip {
            segment_id,
            origin_track,
            origin_start,
            track,
            start,
            ..
        }) = self.drag.take()
        {
            if track != origin_track || start != origin_start {
                let command = edits::move_to(&self.project, &segment_id, &track, start);
                self.apply(command, cx);
            }
        }
        self.drag = None;
        cx.notify();
    }

    fn on_timeline_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(20.0));
        if event.modifiers.control {
            let (x, _) = self.local(event.position);
            let anchor = self.x_to_time(x);
            let factor = if f32::from(delta.y) > 0.0 {
                1.15
            } else {
                1.0 / 1.15
            };
            self.zoom = (self.zoom * factor).clamp(2.0, 2000.0);
            // Keep the time under the pointer where it was.
            self.scroll_x = (anchor as f32 / 1_000_000.0 * self.zoom - (x - HEADER_W)).max(0.0);
        } else {
            let step = if f32::from(delta.x).abs() > f32::from(delta.y).abs() {
                f32::from(delta.x)
            } else {
                f32::from(delta.y)
            };
            self.scroll_x = (self.scroll_x - step).max(0.0);
        }
        cx.notify();
    }

    // --- drawing --------------------------------------------------------------------

    fn material_name(&self, material_id: &str) -> String {
        let pool = &self.project.materials;
        let path = pool
            .videos
            .iter()
            .find(|m| m.id == material_id)
            .map(|m| m.path.as_str())
            .or_else(|| {
                pool.images
                    .iter()
                    .find(|m| m.id == material_id)
                    .map(|m| m.path.as_str())
            })
            .or_else(|| {
                pool.audios
                    .iter()
                    .find(|m| m.id == material_id)
                    .map(|m| m.path.as_str())
            });
        if let Some(path) = path {
            return file_name(path);
        }
        if let Some(text) = pool.texts.iter().find(|m| m.id == material_id) {
            return text.content.clone();
        }
        "clip".into()
    }

    fn clip_color(&self, kind: TrackKind, material_id: &str) -> u32 {
        match kind {
            TrackKind::Video
                if self
                    .project
                    .materials
                    .images
                    .iter()
                    .any(|m| m.id == material_id) =>
            {
                CLIP_IMAGE
            }
            TrackKind::Video => CLIP_VIDEO,
            TrackKind::Audio => CLIP_AUDIO,
            TrackKind::Text => CLIP_TEXT,
            _ => CLIP_OTHER,
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.project.name.clone();
        let saved = self
            .state
            .project_path
            .read()
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        div()
            .h(px(40.0))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .text_color(rgb(ACCENT))
                    .font_weight(gpui::FontWeight::BOLD)
                    .child("chukcut"),
            )
            .child(button(
                "import",
                "Import",
                cx.listener(|this, _, w, cx| this.on_import(&Import, w, cx)),
            ))
            .child(button(
                "open",
                "Open",
                cx.listener(|this, _, w, cx| this.on_open(&Open, w, cx)),
            ))
            .child(button(
                "save",
                "Save",
                cx.listener(|this, _, w, cx| this.on_save(&Save, w, cx)),
            ))
            .child(div().flex_1())
            .children(
                self.status
                    .clone()
                    .map(|status| div().text_xs().text_color(rgb(TEXT_DIM)).child(status)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(TEXT))
                    .child(saved.map(|p| file_name(&p)).unwrap_or(name)),
            )
    }

    fn render_media(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pool = &self.project.materials;
        let items: Vec<(String, String, Micros)> = pool
            .videos
            .iter()
            .map(|m| (m.id.clone(), file_name(&m.path), m.duration))
            .chain(
                pool.images
                    .iter()
                    .map(|m| (m.id.clone(), file_name(&m.path), 0)),
            )
            .chain(
                pool.audios
                    .iter()
                    .map(|m| (m.id.clone(), file_name(&m.path), m.duration)),
            )
            .collect();

        let list = items
            .into_iter()
            .enumerate()
            .map(|(index, (id, name, duration))| {
                let detail = if duration > 0 {
                    timecode(duration, 0.0)
                } else {
                    "still".into()
                };
                div()
                    .id(("media", index))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(PANEL_RAISED))
                    .hover(|style| style.bg(rgb(BORDER)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let command = edits::append(&this.project, &id);
                        this.apply(command, cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(name),
                    )
                    .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(detail))
                    .child(div().text_color(rgb(ACCENT)).child("+"))
            });

        div()
            .w(px(MEDIA_W))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .pb_1()
                    .child(div().text_sm().text_color(rgb(TEXT)).child("Media"))
                    .child(button("import-media", "+ Import", cx.listener(|this, _, w, cx| this.on_import(&Import, w, cx)))),
            )
            .children(list)
            .when(self.project.materials.videos.is_empty()
                && self.project.materials.images.is_empty()
                && self.project.materials.audios.is_empty(), |panel| {
                panel.child(
                    div()
                        .pt_4()
                        .text_xs()
                        .text_color(rgb(TEXT_DIM))
                        .child("Import video, images or audio (Ctrl+I). Click an item to add it to the timeline."),
                )
            })
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let viewer = Rc::clone(&self.viewer);
        let (cw, ch) = (
            self.project.canvas.width as f32,
            self.project.canvas.height as f32,
        );
        let bounds = self.viewer.get();
        let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let fit = if cw > 0.0 && ch > 0.0 {
            (bw / cw).min(bh / ch)
        } else {
            0.0
        };
        let (dw, dh) = (cw * fit, ch * fit);

        let picture = match (&self.frame, self.player.failure()) {
            (_, Some(failure)) => div()
                .text_color(rgb(TEXT_DIM))
                .child(failure)
                .into_any_element(),
            (Some(frame), None) => img(Arc::clone(frame))
                .w(px(dw))
                .h(px(dh))
                .into_any_element(),
            (None, None) => div()
                .w(px(dw))
                .h(px(dh))
                .bg(rgb(0x000000))
                .into_any_element(),
        };

        let playing = self.clock.is_playing();
        let position = self.clock.position();
        div()
            .flex_1()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .child(
                div()
                    .flex_1()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .m_3()
                    .child(
                        canvas(move |bounds, _, _| viewer.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(picture),
            )
            .child(
                div()
                    .h(px(40.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(TEXT))
                            .child(timecode(position, self.project.fps)),
                    )
                    .child(button(
                        "play",
                        if playing { "Pause" } else { "Play" },
                        cx.listener(|this, _, w, cx| this.on_play_pause(&PlayPause, w, cx)),
                    ))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(TEXT_DIM))
                            .child(timecode(self.project.duration(), self.project.fps)),
                    )
                    .child(div().text_xs().text_color(rgb(TEXT_DIM)).child(format!(
                        "{}×{} · {:.2} fps",
                        self.project.canvas.width, self.project.canvas.height, self.project.fps
                    ))),
            )
    }

    fn render_timeline(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let probe = Rc::clone(&self.timeline);
        let width = f32::from(self.timeline.get().size.width).max(400.0);

        // Ruler: a label every `step` seconds, `step` chosen so labels stay
        // at least ~90 px apart.
        let step = [
            0.5_f32, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
        ]
        .into_iter()
        .find(|s| s * self.zoom >= 90.0)
        .unwrap_or(1200.0);
        let first = (self.scroll_x / self.zoom / step).floor() as i64;
        let last = ((self.scroll_x + width) / self.zoom / step).ceil() as i64;
        let ticks = (first.max(0)..=last).map(|i| {
            let seconds = i as f32 * step;
            let x = self.time_to_x((seconds * 1_000_000.0) as Micros);
            div()
                .absolute()
                .left(px(x))
                .top(px(0.0))
                .h(px(RULER_H))
                .border_l_1()
                .border_color(rgb(BORDER))
                .pl_1()
                .text_xs()
                .text_color(rgb(TEXT_DIM))
                .child(clock_label(seconds))
        });

        let mut rows = Vec::new();
        let mut clips = Vec::new();
        let mut top = RULER_H;
        for track in &self.project.tracks {
            let height = row_height(track.kind);
            rows.push(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(top))
                    .w_full()
                    .h(px(height))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .w(px(HEADER_W))
                            .h_full()
                            .flex()
                            .items_center()
                            .px_2()
                            .bg(rgb(PANEL))
                            .border_r_1()
                            .border_color(rgb(BORDER))
                            .text_xs()
                            .text_color(rgb(TEXT_DIM))
                            .child(track.name.clone()),
                    ),
            );

            for segment in &track.segments {
                // While a clip is being dragged it is drawn where the pointer
                // has it, not where the document has it.
                let (track_top, start) = match &self.drag {
                    Some(Drag::Clip {
                        segment_id,
                        track: to,
                        start,
                        ..
                    }) if *segment_id == segment.id => (self.track_top(to).unwrap_or(top), *start),
                    _ => (top, segment.target_range.start),
                };
                let x = self.time_to_x(start).max(HEADER_W);
                let right = self.time_to_x(start + segment.target_range.duration);
                if right <= HEADER_W {
                    continue;
                }
                let selected = self.selected.as_deref() == Some(segment.id.as_str());
                clips.push(
                    div()
                        .absolute()
                        .left(px(x))
                        .top(px(track_top + 3.0))
                        .w(px((right - x - 1.0).max(2.0)))
                        .h(px(height - 6.0))
                        .rounded(px(4.0))
                        .bg(rgb(self.clip_color(track.kind, &segment.material_id)))
                        .border_2()
                        .border_color(rgb(if selected { PLAYHEAD } else { 0x000000 }))
                        .overflow_hidden()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(self.material_name(&segment.material_id)),
                );
            }
            top += height;
        }

        let playhead_x = self.time_to_x(self.clock.position());
        let playhead = (playhead_x >= HEADER_W).then(|| {
            div()
                .absolute()
                .left(px(playhead_x - 1.0))
                .top(px(0.0))
                .w(px(2.0))
                .h_full()
                .bg(rgb(PLAYHEAD))
        });

        div()
            .h(px(TIMELINE_H))
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(34.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(button(
                        "split",
                        "Split (S)",
                        cx.listener(|this, _, w, cx| this.on_split(&Split, w, cx)),
                    ))
                    .child(button(
                        "delete",
                        "Delete",
                        cx.listener(|this, _, w, cx| this.on_delete(&DeleteSelected, w, cx)),
                    ))
                    .child(button(
                        "undo",
                        "Undo",
                        cx.listener(|this, _, w, cx| this.on_undo(&Undo, w, cx)),
                    ))
                    .child(button(
                        "redo",
                        "Redo",
                        cx.listener(|this, _, w, cx| this.on_redo(&Redo, w, cx)),
                    ))
                    .child(div().flex_1())
                    .child(button(
                        "zoom-out",
                        "−",
                        cx.listener(|this, _, _, cx| this.zoom_by(1.0 / 1.4, cx)),
                    ))
                    .child(button(
                        "zoom-in",
                        "+",
                        cx.listener(|this, _, _, cx| this.zoom_by(1.4, cx)),
                    )),
            )
            .child(
                div()
                    .id("timeline")
                    .flex_1()
                    .relative()
                    .overflow_hidden()
                    .bg(rgb(BG))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_timeline_down))
                    .on_scroll_wheel(cx.listener(Self::on_timeline_scroll))
                    .child(
                        canvas(move |bounds, _, _| probe.set(bounds), |_, _, _, _| {})
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(0.0))
                            .top(px(0.0))
                            .w_full()
                            .h(px(RULER_H))
                            .bg(rgb(PANEL))
                            .border_b_1()
                            .border_color(rgb(BORDER))
                            .children(ticks),
                    )
                    .children(rows)
                    .children(clips)
                    .children(playhead),
            )
    }

    fn track_top(&self, track_id: &str) -> Option<f32> {
        let mut top = RULER_H;
        for track in &self.project.tracks {
            if track.id == track_id {
                return Some(top);
            }
            top += row_height(track.kind);
        }
        None
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale = window.scale_factor();
        div()
            .track_focus(&self.focus)
            .key_context("Editor")
            .on_action(cx.listener(Self::on_play_pause))
            .on_action(cx.listener(Self::on_split))
            .on_action(cx.listener(Self::on_delete))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_import))
            .on_action(cx.listener(Self::on_open))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(|this, _: &StepBack, _, cx| {
                this.step(-1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &StepForward, _, cx| {
                this.step(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoToStart, _, cx| {
                this.seek(0);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoToEnd, _, cx| {
                let end = this.project.duration();
                this.seek(end);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom_by(1.4, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom_by(1.0 / 1.4, cx)))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Noto Sans")
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .min_h(px(0.0))
                    .child(self.render_media(cx))
                    .child(self.render_preview(cx)),
            )
            .child(self.render_timeline(cx))
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.audio.shutdown();
    }
}

// --- small pieces ---------------------------------------------------------------------

fn button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2()
        .py(px(3.0))
        .rounded_md()
        .bg(rgb(PANEL_RAISED))
        .border_1()
        .border_color(rgb(BORDER))
        .hover(|style| style.bg(rgb(BORDER)))
        .cursor_pointer()
        .text_sm()
        .text_color(rgb(TEXT))
        .on_click(on_click)
        .child(label)
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// `mm:ss:ff` at `fps`, or `mm:ss` when `fps` is zero.
fn timecode(time: Micros, fps: f64) -> String {
    let total = time.max(0) as f64 / 1_000_000.0;
    let minutes = (total / 60.0).floor() as i64;
    let seconds = (total % 60.0).floor() as i64;
    if fps <= 0.0 {
        return format!("{minutes:02}:{seconds:02}");
    }
    let frames = ((total.fract()) * fps).floor() as i64;
    format!("{minutes:02}:{seconds:02}:{frames:02}")
}

fn clock_label(seconds: f32) -> String {
    let whole = seconds.floor() as i64;
    if seconds.fract() > 0.0 {
        format!("{:02}:{:02}.5", whole / 60, whole % 60)
    } else {
        format!("{:02}:{:02}", whole / 60, whole % 60)
    }
}
