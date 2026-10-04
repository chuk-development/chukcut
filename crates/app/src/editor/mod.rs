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

use chukcut_engine::modules::audio::AudioEngine;
use chukcut_engine::modules::export::commands as export_commands;
use chukcut_engine::modules::export::{ExportProgress, ExportStage};
use chukcut_engine::modules::preview::clock::{
    frame_at, frame_start, nearest_frame_time, PlaybackClock,
};
use chukcut_engine::modules::project::commands as project_commands;
use chukcut_engine::modules::project::{Micros, Project, Track, TrackKind};
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use chukcut_engine::shell::Channel;
use chukcut_engine::state::AppState;
use gpui::prelude::*;
use gpui::{
    actions, canvas, div, img, px, rgb, App, Bounds, Context, FocusHandle, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, RenderImage, ScrollWheelEvent,
    SharedString, Task, Window,
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
        Export,
        StepBack,
        StepForward,
        GoToStart,
        GoToEnd,
        ZoomIn,
        ZoomOut,
        Quit
    ]
);

mod accounts;
mod analysis;
mod assets;
mod audio_tools;
mod captions;
mod cloud;
mod export;
mod files;
mod font_picker;
mod home;
mod inspector;
pub(crate) mod keymap;
mod lifecycle;
mod ml_settings;
mod playback;
mod preview;
mod settings;
mod shell;
mod shortcuts;
mod silence;
mod templates;
mod timeline;
pub(crate) use keymap::install as install_keymap;
pub(crate) use shell::{quit, startup, Shell};
use shortcuts::{NewProject, OpenSettings, ShowShortcuts};
mod title_bar;
mod tracking;
mod widgets;

use crate::theme::*;
use files::{FileRequest, Filter};
use widgets::*;

// --- state -------------------------------------------------------------------

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

    frame: Option<crate::player::Picture>,
    last_request: Option<(i64, (u32, u32), u64, bool)>,
    scale: f32,

    selected: Option<String>,
    status: Option<SharedString>,
    /// The status line as last seen by the tick, and since when; see
    /// `expire_status`.
    status_since: Option<(SharedString, std::time::Instant)>,

    viewer: Rc<Cell<Bounds<Pixels>>>,
    timeline: timeline::TimelineState,
    /// The newest progress message of a running export, written from the
    /// export thread and read by [`Self::tick`].
    export_progress: Arc<parking_lot::Mutex<Option<ExportProgress>>>,
    /// The export queue's events, for the status line and notifications.
    export_queue: export::QueueWatch,
    inspector: inspector::Inspector,
    /// The title bar's save state.
    title: title_bar::TitleState,
    /// The asset panel's tabs, search and thumbnails.
    assets: assets::AssetPanel,
    /// The player's preview quality.
    preview: preview::PreviewState,
    /// Unsaved-changes tracking, settings, transport extras (`lifecycle.rs`,
    /// `playback.rs`).
    shell: lifecycle::ShellState,
    /// The Captions tab: transcription, caption editing and styling.
    captions: captions::CaptionsPanel,
    /// Motion tracking: the box on the player and the running analysis.
    tracking: tracking::TrackingUi,
    /// Scene, stabilisation, beat and reframe jobs (`analysis.rs`).
    analysis: analysis::AnalysisUi,
    /// Ducking jobs and the voiceover take (`audio_tools.rs`).
    audio_tools: audio_tools::AudioToolsUi,
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

        let assets = assets::AssetPanel::new(window, cx);
        let shell = lifecycle::ShellState::new(&project);
        let preview = preview::PreviewState {
            quality: settings::quality_for_scale(shell.settings.preview_scale()),
        };
        let captions = captions::CaptionsPanel::new(window, cx);
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
            status: None,
            status_since: None,
            viewer: Rc::new(Cell::new(Bounds::default())),
            timeline: timeline::TimelineState::new(cx),
            export_progress: Arc::new(parking_lot::Mutex::new(None)),
            export_queue: export::QueueWatch::default(),
            inspector: inspector::Inspector::default(),
            title: Default::default(),
            assets,
            preview,
            shell,
            captions,
            tracking: Default::default(),
            analysis: Default::default(),
            audio_tools: Default::default(),
            _ticker: ticker,
        };
        // Hardware encoder detection opens each device and encodes a test
        // frame. Do it now, off the UI thread, so the Export button does not
        // pay for it.
        std::thread::spawn(|| {
            let _ = chukcut_engine::modules::export::hwaccel::detect();
        });
        editor.consider_proxies();
        if !startup.is_empty() {
            editor.import_paths(startup, cx);
        }
        editor
    }

    /// Apply the settings' proxy policy: preview from proxies unless it is
    /// `Off`, and queue a proxy for every video the policy says needs one.
    /// The queue skips files it has a proxy for or already decided against,
    /// but deciding probes each file, so it runs off the UI thread.
    pub(crate) fn consider_proxies(&mut self) {
        use chukcut_engine::modules::workspace::settings::ProxyPolicy;
        let policy = self.shell.settings.proxy_policy;
        self.player.use_proxies(policy != ProxyPolicy::Off);
        if policy == ProxyPolicy::Off {
            return;
        }
        let paths: Vec<String> = self
            .project
            .materials
            .videos
            .iter()
            .map(|video| video.path.clone())
            .collect();
        std::thread::spawn(move || {
            for path in paths {
                if let Err(error) =
                    chukcut_engine::modules::proxy::commands::proxy_consider(path.clone(), policy)
                {
                    tracing::debug!(%path, %error, "no proxy");
                }
            }
        });
    }

    // --- the clock and the picture --------------------------------------------

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.tick_playback();
        let playing = self.clock.is_playing() || self.shell.playback_driven();
        if playing && self.clock.is_at_end() {
            self.pause();
        }
        self.request_frame();

        let mut changed = playing;
        if let Some(progress) = self.export_progress.lock().take() {
            self.status = Some(export::export_status(&progress).into());
            changed = true;
        }
        changed |= self.poll_export_queue();
        changed |= self.expire_status();
        changed |= self.poll_tracking(cx);
        changed |= self.poll_matting(cx);
        changed |= self.poll_flow(cx);
        changed |= self.poll_analysis(cx);
        changed |= self.poll_voiceover(cx);
        if let Some(frame) = self.player.take(self.clock.position()) {
            if let Some(crate::player::Picture::Image(old)) = self.frame.replace(frame.picture) {
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
        let mut fit = (bw / cw).min(bh / ch) * self.scale;
        // A long-edge cap from the settings, for weak machines. 0 = none.
        let cap = self.shell.settings.preview_max_edge;
        if cap > 0 {
            fit = fit.min(cap as f32 / cw.max(ch));
        }
        let fit = fit.min(1.0) * self.preview.quality.scale();
        Some(((cw * fit).round() as u32, (ch * fit).round() as u32))
    }

    fn request_frame(&mut self) {
        let Some(size) = self.render_size() else {
            return;
        };
        let time = self.clock.position();
        // Normal-speed playback renders ahead on the audio clock; a shuttle
        // or a scrub renders exactly where the playhead is.
        let playing = self.clock.is_playing() && !self.shell.playback_driven();
        let key = (
            frame_at(time, self.project.fps),
            size,
            self.generation,
            playing,
        );
        if self.last_request == Some(key) {
            return;
        }
        self.last_request = Some(key);
        // Just inside the frame, like the export: see `SAMPLE_SLACK`.
        self.player.request(
            // The document, or a copy showing the selected clip's matte.
            self.preview_project(),
            self.generation,
            time + chukcut_engine::modules::project::SAMPLE_SLACK,
            size,
            playing,
        );
    }

    fn play(&mut self) {
        if self.project.duration() <= 0 {
            return;
        }
        if self.clock.is_at_end() {
            self.clock.seek(0);
        }
        self.before_play();
        self.audio.set_project(Arc::clone(&self.project));
        self.audio.play(self.clock.position());
        self.clock.play();
    }

    fn pause(&mut self) {
        self.clock.pause();
        self.audio.pause();
        self.after_pause();
    }

    fn seek(&mut self, to: Micros) {
        let to = to.clamp(0, self.project.duration().max(0));
        self.clock.seek(to);
        if self.clock.is_playing() {
            self.audio.seek(to);
        } else {
            self.scrub_sound(to);
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
        // An edit can leave a removed background with frames to bake (a
        // trim made the clip longer); bake them in the background.
        self.queue_missing_mattes(cx);
        self.queue_missing_flow(cx);
        cx.notify();
    }

    /// Let a finished message go after a while, so an old error ("cannot
    /// open …") does not sit in the title bar until something else is said.
    /// A message of work in progress ("Importing…", "Exporting 40 %") stays
    /// until the work replaces it. Answers whether the line changed.
    fn expire_status(&mut self) -> bool {
        const SHOWN_FOR: std::time::Duration = std::time::Duration::from_secs(8);
        let Some(status) = self.status.clone() else {
            self.status_since = None;
            return false;
        };
        match &self.status_since {
            Some((seen, since)) if *seen == status => {
                let ongoing = status.ends_with('\u{2026}') || status.starts_with("Exporting");
                if !ongoing && since.elapsed() >= SHOWN_FOR {
                    self.status = None;
                    self.status_since = None;
                    return true;
                }
                false
            }
            _ => {
                self.status_since = Some((status, std::time::Instant::now()));
                false
            }
        }
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
                editor.consider_proxies();
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
        // On the start of the frame on screen, never between frames: the
        // playhead may be anywhere inside a frame while playing.
        let at = frame_start(self.clock.position(), self.project.fps);
        let result = match self.selected.clone() {
            Some(id) => timeline_commands::timeline_split(&self.state, id, at),
            None => timeline_commands::timeline_split_all(&self.state, at),
        }
        .map(|_| ());
        self.refresh(cx);
        self.report(result, cx);
    }

    fn on_delete(&mut self, _: &DeleteSelected, window: &mut Window, cx: &mut Context<Self>) {
        // A tracked clip takes its followers' motion with it; ask first.
        if self.offer_bake_before_delete(window, cx) {
            return;
        }
        // The timeline owns what is selected: clips, a keyframe, a transition.
        self.delete_selection(cx);
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
        let picked = files::choose(FileRequest::open_many("Import", Filter::Media), cx);
        cx.spawn(async move |this, cx| {
            if let Some(paths) = picked.await {
                let _ = this.update(cx, |editor, cx| editor.import_to_library(paths, cx));
            }
        })
        .detach();
    }

    fn on_open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.request_open(window, cx);
    }

    fn on_save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save_interactively(cx).detach();
    }

    fn step(&mut self, frames: i64) {
        self.step_frames(frames);
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
        if let Some(sequence) = pool.sequence(material_id) {
            return sequence.name.clone();
        }
        if let Some(effect) = pool.effect(material_id) {
            return chukcut_engine::modules::fx::descriptor(&effect.kind)
                .map_or_else(|| effect.kind.clone(), |d| d.label.to_string());
        }
        "clip".into()
    }

    fn clip_color(&self, kind: TrackKind, material_id: &str) -> u32 {
        if self.project.materials.sequence(material_id).is_some() {
            return CLIP_COMPOUND;
        }
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
            TrackKind::Effect => CLIP_EFFECT,
            _ => CLIP_OTHER,
        }
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.scale = window.scale_factor();
        self.sync_window_title(window);
        let (media_w, inspector_w) = side_widths(f32::from(window.viewport_size().width));
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
            .on_action(cx.listener(Self::on_export))
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
            .map(|root| self.timeline_actions(root, cx))
            .map(|root| self.playback_actions(root, cx))
            .map(|root| self.analysis_actions(root, cx))
            .on_action(
                cx.listener(|this, _: &NewProject, window, cx| this.request_home(window, cx)),
            )
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_drop(cx.listener(Self::on_media_drop))
            .on_drop(cx.listener(Self::on_effect_drop))
            .on_drop(cx.listener(Self::on_title_drop))
            // A click anywhere gives the keyboard back to the editor; a text
            // field under the pointer takes it again in its own handler,
            // which runs after this capture-phase one.
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                window.focus(&this.focus, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Noto Sans")
            .child(self.render_toolbar(cx))
            // CapCut's arrangement: assets | player | inspector over the
            // timeline, as separate rounded panels on the window colour.
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .gap(px(6.0))
                    .px(px(6.0))
                    .pt(px(6.0))
                    .min_h(px(0.0))
                    .child(
                        div()
                            .w(px(media_w))
                            .flex_none()
                            .flex()
                            .child(self.render_media(cx)),
                    )
                    .child(self.render_preview(cx))
                    .child(
                        div()
                            .w(px(inspector_w))
                            .flex_none()
                            .flex()
                            .child(self.render_inspector(window, cx)),
                    ),
            )
            .child(self.render_timeline(window, cx))
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.audio.shutdown();
    }
}
