//! The frame server.
//!
//! Owns the compositor, the live session, the ring buffer and the two threads
//! that keep them moving:
//!
//! - the **render thread** produces frames — ahead of the playhead during
//!   playback, exactly at it while scrubbing — encodes them and puts them in
//!   the ring;
//! - the **pacer thread** watches the clock and tells the frontend which frame
//!   number to display, at the frame rate rather than at the render rate.
//!
//! Splitting the two is what makes read-ahead work. If the frontend were told
//! about frames as they were rendered it would display them as fast as the GPU
//! produced them, which is the same as having no ring at all. The renderer runs
//! as far ahead as it likes; the pacer decides when each frame is due.
//!
//! ## The URL
//!
//! ```text
//! chukcut-frame://preview/<session>/<frame>
//! ```
//!
//! Three answers, and the difference between them matters:
//!
//! - **200** — the frame, as JPEG bytes.
//! - **410 Gone** — the session has been superseded. Never a stale image: the
//!   whole point of the session id is that a request from before a seek gets an
//!   error the frontend can ignore rather than a picture of the wrong moment.
//! - **404** — the right session, but the frame is not rendered yet. The
//!   handler waits [`FRAME_WAIT`] for it first, because "the frame is 10 ms
//!   away" should read as a slightly slow image and not as a blank viewer, but
//!   it does give up. A protocol handler that blocks indefinitely blocks the
//!   webview.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex, RwLock};
use serde::Serialize;
use tauri::ipc::Channel;

use super::cache::{CachedFrame, FrameCache, Lookup, DEFAULT_CAPACITY};
use super::clock::{
    frame_at, frame_interval, frame_time, pace, MonotonicSource, Pacing, PlaybackClock, TimeSource,
    DEFAULT_READ_AHEAD,
};
use super::encoder::encode_preview_jpeg;
use super::error::{PreviewError, Result};
use super::session::{PreviewOptions, PreviewSession};
use crate::modules::audio::AudioEngine;
use crate::modules::project::document::{Micros, Project};
use crate::modules::render::{Compositor, EmptySourceProvider, RenderContext, SourceProvider};

/// The registered URI scheme. Must match `register_uri_scheme_protocol` in
/// `lib.rs` and the `frameUrl` the frontend builds.
pub const SCHEME: &str = "chukcut-frame";

/// How long a frame request waits for a frame that is being rendered before it
/// gives up with a 404.
pub const FRAME_WAIT: Duration = Duration::from_millis(60);

/// How far off the requested frame a substitute may be, in frames.
///
/// Two frames is 66 ms at 30 fps — close enough that a viewer reads it as a
/// frame arriving slightly late, which is what it is. Beyond that the picture
/// visibly jumps backwards, and leaving the previous frame up is better.
const NEAREST_TOLERANCE: i64 = 2;

/// How long the render thread sleeps when the ring is full enough. Short, so
/// that a seek or a pause is acted on promptly.
const IDLE_TICK: Duration = Duration::from_millis(4);

/// Upper bound on the pacer's sleep, so shutdown is never more than this away.
const PACER_TICK: Duration = Duration::from_millis(8);

// ---------------------------------------------------------------------------
// What the frontend hears
// ---------------------------------------------------------------------------

/// Playback position and lifecycle, pushed over a [`Channel`].
///
/// Position updates are pushed rather than polled: at 30 fps a polling frontend
/// either asks too often and burns IPC on unchanged answers, or asks too rarely
/// and shows a playhead that lags the picture.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum PreviewEvent {
    /// The session is open and the renderer has a device.
    Ready {
        session: u64,
        width: u32,
        height: u32,
        fps: f64,
        duration: Micros,
    },
    /// Display this frame. The frontend turns it into a URL and sets an image
    /// source; it does not need to know anything else.
    Position {
        session: u64,
        frame: i64,
        time: Micros,
        playing: bool,
    },
    /// Playback reached the end of the project and stopped.
    Ended { session: u64 },
    /// Something the user should be told about — no GPU, or a frame that could
    /// not be produced while they were waiting on it.
    Error { message: String },
}

/// A snapshot of the preview for `preview_state`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStatus {
    /// `None` when no session is open.
    pub session: Option<u64>,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub fps: f64,
    pub duration: Micros,
    pub position: Micros,
    pub frame: i64,
    pub playing: bool,
    /// Frames currently in the ring, and how many it holds.
    pub cached: usize,
    pub capacity: usize,
    /// Prefix the frontend appends `/<frame>` to. Built here because the shape
    /// of a custom-protocol URL differs between platforms.
    pub frame_url: Option<String>,
}

/// What every command hands back, so the frontend can update its store from one
/// value regardless of which call it made.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub session: u64,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub fps: f64,
    pub duration: Micros,
    pub position: Micros,
    pub frame: i64,
    pub playing: bool,
    pub frame_url: String,
}

/// The base URL for a session's frames.
///
/// Custom protocols do not look the same everywhere: on Windows and Android
/// they are folded into `http://<scheme>.localhost/`, elsewhere the scheme is
/// used directly. Getting this wrong produces a viewer that works on one
/// developer's machine and not another's.
pub fn frame_url(session: u64) -> String {
    #[cfg(any(windows, target_os = "android"))]
    {
        format!("http://{SCHEME}.localhost/preview/{session}")
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        format!("{SCHEME}://preview/{session}")
    }
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// Everything the worker threads and the command surface share.
struct Shared {
    work: Mutex<Work>,
    /// Signalled whenever `work` changes in a way a thread might care about.
    wake: Condvar,
    cache: Arc<FrameCache>,
    clock: PlaybackClock,
    /// Injected by `media`. Behind an `RwLock` because the decoder is built
    /// after the preview server is managed by Tauri, and swapped when a project
    /// is closed.
    sources: RwLock<Arc<dyn SourceProvider>>,
    /// The audio engine, when there is one. The preview owns the playhead
    /// and audio owns the sound, so every transition of one has to be a
    /// transition of the other. Injected, so the preview still works on a
    /// machine with no output device.
    audio: RwLock<Option<Arc<AudioEngine>>>,
    events: RwLock<Option<Channel<PreviewEvent>>>,
}

#[derive(Default)]
struct Work {
    shutdown: bool,
    playing: bool,
    session: Option<Arc<PreviewSession>>,
    /// A single pending scrub. Newest wins: a superseded position is simply
    /// overwritten before the renderer ever sees it, which is what cancelling
    /// an in-flight render amounts to when a render is one frame long.
    scrub: Option<i64>,
    /// Next frame playback intends to render.
    cursor: i64,
    read_ahead: usize,
    /// One error per session is informative; one per frame is a stream of
    /// modals.
    reported_error: bool,
}

impl Shared {
    fn audio(&self) -> Option<Arc<AudioEngine>> {
        self.audio.read().clone()
    }

    fn emit(&self, event: PreviewEvent) {
        if let Some(channel) = self.events.read().as_ref() {
            if let Err(error) = channel.send(event) {
                tracing::warn!(%error, "preview event channel is gone");
            }
        }
    }

    fn emit_error_once(&self, message: String) {
        {
            let mut work = self.work.lock();
            if work.reported_error {
                tracing::warn!(message, "preview error (already reported)");
                return;
            }
            work.reported_error = true;
        }
        tracing::warn!(message, "preview error");
        self.emit(PreviewEvent::Error { message });
    }
}

pub struct PreviewServer {
    shared: Arc<Shared>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl PreviewServer {
    pub fn new() -> Arc<Self> {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Arc<Self> {
        Self::with_time_source(capacity, Arc::new(MonotonicSource::default()))
    }

    /// A server whose playback clock reads from `source`.
    ///
    /// Injected rather than fixed for the same reason [`PlaybackClock`] takes
    /// one: the audio device becomes the clock master, and when it does this is
    /// the only line that changes.
    ///
    /// It is also what makes read-ahead testable. Playback driven by wall time
    /// can only be checked by sleeping and hoping, which produces a test that
    /// passes on an idle machine and fails on a busy one — worse than no test,
    /// because it teaches people to rerun until it goes green. With a source
    /// that only moves when told to, how far the renderer runs ahead is an
    /// exact function of where the clock was put.
    pub fn with_time_source(capacity: usize, source: Arc<dyn TimeSource>) -> Arc<Self> {
        Arc::new(Self {
            shared: Arc::new(Shared {
                work: Mutex::new(Work {
                    read_ahead: DEFAULT_READ_AHEAD,
                    ..Work::default()
                }),
                wake: Condvar::new(),
                cache: Arc::new(FrameCache::new(capacity)),
                clock: PlaybackClock::with_source(source, 30.0, 0),
                sources: RwLock::new(Arc::new(EmptySourceProvider)),
                audio: RwLock::new(None),
                events: RwLock::new(None),
            }),
            threads: Mutex::new(Vec::new()),
        })
    }

    /// Wire in the real decoder.
    ///
    /// Injected rather than constructed here so this module never depends on
    /// `media`: the preview drives whatever can produce textures, which is also
    /// what lets the tests drive it with solid colours.
    pub fn set_source_provider(&self, sources: Arc<dyn SourceProvider>) {
        *self.shared.sources.write() = sources;
    }

    pub fn set_audio(&self, audio: Arc<AudioEngine>) {
        *self.shared.audio.write() = Some(audio);
    }

    pub fn cache(&self) -> &Arc<FrameCache> {
        &self.shared.cache
    }

    pub fn clock(&self) -> &PlaybackClock {
        &self.shared.clock
    }

    pub fn session(&self) -> Option<Arc<PreviewSession>> {
        self.shared.work.lock().session.clone()
    }

    /// Open a session on `project` and render the frame at `at`.
    ///
    /// Returns as soon as the session exists. Opening the GPU takes long enough
    /// to be visible in a command round-trip, so it happens on the render
    /// thread and its failure arrives as [`PreviewEvent::Error`].
    ///
    /// Calling this again replaces the session, which is also how the preview
    /// picks up an edit: the document is snapshotted here and the render thread
    /// never touches the live one.
    pub fn start(
        self: &Arc<Self>,
        project: Arc<Project>,
        options: PreviewOptions,
        at: Micros,
        events: Channel<PreviewEvent>,
    ) -> PreviewInfo {
        *self.shared.events.write() = Some(events);
        self.spawn_threads();

        if let Some(audio) = self.shared.audio() {
            audio.set_project(Arc::clone(&project));
        }

        let session = Arc::new(PreviewSession::new(project, options));
        self.adopt(session, at, false)
    }

    /// Move the playhead. Supersedes the session so no frame rendered for the
    /// old position can be served against the new one.
    pub fn seek(&self, to: Micros) -> Result<PreviewInfo> {
        let (previous, playing) = {
            let work = self.shared.work.lock();
            let session = work.session.clone().ok_or(PreviewError::NoSession)?;
            (session, work.playing)
        };
        Ok(self.adopt(Arc::new(previous.superseded()), to, playing))
    }

    pub fn play(&self) -> Result<PreviewInfo> {
        let session = self
            .shared
            .work
            .lock()
            .session
            .clone()
            .ok_or(PreviewError::NoSession)?;

        // Starting from the end would produce one frame and an immediate stop;
        // treat it as a replay, which is what every player does.
        if self.shared.clock.is_at_end() {
            self.shared.clock.seek(0);
        }
        if let Some(audio) = self.shared.audio() {
            audio.play(self.shared.clock.position());
        }
        self.shared.clock.play();

        let mut work = self.shared.work.lock();
        work.playing = true;
        work.cursor = self.shared.clock.frame();
        drop(work);
        self.shared.wake.notify_all();

        Ok(self.info(&session))
    }

    pub fn pause(&self) -> Result<PreviewInfo> {
        let session = self
            .shared
            .work
            .lock()
            .session
            .clone()
            .ok_or(PreviewError::NoSession)?;

        if let Some(audio) = self.shared.audio() {
            audio.pause();
        }
        self.shared.clock.pause();
        {
            let mut work = self.shared.work.lock();
            work.playing = false;
        }
        self.shared.wake.notify_all();

        let info = self.info(&session);
        self.shared.emit(PreviewEvent::Position {
            session: session.id,
            frame: info.frame,
            time: info.position,
            playing: false,
        });
        Ok(info)
    }

    /// Close the session and stop the threads. The ring is dropped: its frames
    /// describe a document state that is no longer on screen.
    pub fn stop(&self) {
        {
            let mut work = self.shared.work.lock();
            work.shutdown = true;
            work.playing = false;
            work.session = None;
            work.scrub = None;
        }
        self.shared.wake.notify_all();

        let threads: Vec<JoinHandle<()>> = std::mem::take(&mut *self.threads.lock());
        for thread in threads {
            let _ = thread.join();
        }

        if let Some(audio) = self.shared.audio() {
            audio.stop();
        }
        self.shared.cache.reset(0);
        self.shared.clock.pause();
        *self.shared.events.write() = None;
        self.shared.work.lock().shutdown = false;
    }

    pub fn status(&self) -> PreviewStatus {
        let work = self.shared.work.lock();
        let session = work.session.clone();
        let playing = work.playing;
        drop(work);

        let position = self.shared.clock.position();
        match session {
            Some(session) => PreviewStatus {
                session: Some(session.id),
                width: session.width(),
                height: session.height(),
                quality: session.quality,
                fps: session.fps,
                duration: session.duration,
                position,
                frame: frame_at(position, session.fps),
                playing,
                cached: self.shared.cache.len(),
                capacity: self.shared.cache.capacity(),
                frame_url: Some(frame_url(session.id)),
            },
            None => PreviewStatus {
                session: None,
                width: 0,
                height: 0,
                quality: 0,
                fps: 0.0,
                duration: 0,
                position: 0,
                frame: 0,
                playing: false,
                cached: 0,
                capacity: self.shared.cache.capacity(),
                frame_url: None,
            },
        }
    }

    /// Ask for a frame that is not in the ring.
    ///
    /// The protocol handler calls this so a frame URL is self-sufficient: the
    /// frontend can address any frame of the live session and get it, whether
    /// or not anything asked for it first.
    /// Ask for a frame the webview wanted and did not find in the ring.
    ///
    /// **Never during playback.** This is the fix for a feedback loop that made
    /// playback unusable, and it is worth spelling out because the obvious
    /// implementation is the broken one:
    ///
    /// The render thread serves scrub jobs before playback jobs, and a scrub
    /// job seeks the decoder to that exact frame. During playback the webview
    /// asks for frames slightly ahead of what has been rendered, so treating
    /// those requests as scrubs made the decoder jump forward — and the next
    /// playback frame, one frame after the cursor, was then *behind* the
    /// decoder and needed a full backward seek. Every frame cost a seek instead
    /// of a forward decode, the renderer fell further behind, which produced
    /// more misses, which produced more seeks. Measured: single frames taking
    /// 13.5 seconds, against a 33 ms budget.
    ///
    /// While playing, the renderer is already walking toward that frame in the
    /// cheapest possible order. The request is dropped and the handler answers
    /// with the nearest frame it has.
    pub fn request_frame(&self, session: u64, frame: i64) {
        let mut work = self.shared.work.lock();
        let Some(live) = work.session.as_ref() else {
            return;
        };
        if live.id != session {
            return;
        }
        if work.playing {
            return;
        }
        work.scrub = Some(frame);
        drop(work);
        self.shared.wake.notify_all();
    }

    /// Serve one frame URL.
    ///
    /// Takes the URL as a string rather than a parsed request so the whole
    /// decision — parse, staleness, wait, status — is exercisable without a
    /// Tauri app.
    pub fn serve_uri(&self, uri: &str) -> tauri::http::Response<Vec<u8>> {
        let Some((session, frame)) = parse_frame_uri(uri) else {
            return text_response(
                tauri::http::StatusCode::BAD_REQUEST,
                format!("{uri} is not a preview frame url"),
            );
        };

        match self.shared.cache.get(session, frame) {
            Lookup::Hit(bytes) => return jpeg_response(&bytes),
            Lookup::Stale => return gone(session),
            Lookup::Miss => {}
        }

        // Not rendered yet. Nudge the renderer and wait briefly.
        self.request_frame(session, frame);
        match self.shared.cache.wait(session, frame, FRAME_WAIT) {
            Lookup::Hit(bytes) => return jpeg_response(&bytes),
            Lookup::Stale => return gone(session),
            Lookup::Miss => {}
        }

        // Still not there. Answer with the closest frame we do have rather than
        // an error.
        //
        // This is not politeness, it is a crash fix: WebKitGTK reacts to a
        // stream of failed resource loads by tearing down its web process, and
        // at 30 requests a second a renderer that falls momentarily behind
        // produces exactly that stream. A neighbouring frame is a few
        // milliseconds stale and looks identical to a late one.
        if let Some((served, bytes)) = self.shared.cache.nearest(session, frame, NEAREST_TOLERANCE) {
            if served != frame {
                tracing::debug!(
                    requested = frame,
                    served,
                    "served a neighbouring frame; the renderer is behind"
                );
            }
            return jpeg_response(&bytes);
        }

        // Nothing has been rendered for this session at all — the very first
        // frame, on a cold decoder. 204 rather than 404: it is a *successful*
        // response carrying no image, so the webview keeps what it is showing
        // instead of recording a failure.
        tauri::http::Response::builder()
            .status(tauri::http::StatusCode::NO_CONTENT)
            .header("Access-Control-Allow-Origin", "*")
            .body(Vec::new())
            .expect("static response parts are valid")
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    /// Install a session, invalidate the ring, and queue the frame under the
    /// playhead.
    fn adopt(&self, session: Arc<PreviewSession>, at: Micros, keep_playing: bool) -> PreviewInfo {
        // The ring is reset before the session is installed so a render already
        // in flight for the old session cannot land in the new one's ring.
        self.shared.cache.reset(session.id);
        if !keep_playing {
            self.shared.clock.pause();
        }
        // Audio first, always. The engine flushes its ring and marks where
        // the new sound will be heard; the playhead has to anchor against
        // that mark rather than the reading from before the seek, or the
        // picture leads the sound by the buffer depth after every seek.
        if let Some(audio) = self.shared.audio() {
            audio.seek(at);
        }
        self.shared.clock.retime(session.fps, session.duration);
        self.shared.clock.seek(at);

        let frame = self.shared.clock.frame();
        {
            let mut work = self.shared.work.lock();
            work.session = Some(Arc::clone(&session));
            work.scrub = Some(frame);
            work.cursor = frame;
            work.playing = keep_playing;
            work.reported_error = false;
        }
        self.shared.wake.notify_all();

        self.info(&session)
    }

    fn info(&self, session: &PreviewSession) -> PreviewInfo {
        let position = self.shared.clock.position();
        PreviewInfo {
            session: session.id,
            width: session.width(),
            height: session.height(),
            quality: session.quality,
            fps: session.fps,
            duration: session.duration,
            position,
            frame: frame_at(position, session.fps),
            playing: self.shared.clock.is_playing(),
            frame_url: frame_url(session.id),
        }
    }

    fn spawn_threads(self: &Arc<Self>) {
        let mut threads = self.threads.lock();
        if !threads.is_empty() {
            return;
        }
        let render_shared = Arc::clone(&self.shared);
        threads.push(
            std::thread::Builder::new()
                .name("chukcut-preview-render".into())
                .spawn(move || render_loop(render_shared))
                .expect("spawn preview render thread"),
        );
        let pacer_shared = Arc::clone(&self.shared);
        threads.push(
            std::thread::Builder::new()
                .name("chukcut-preview-pacer".into())
                .spawn(move || pace_loop(pacer_shared))
                .expect("spawn preview pacer thread"),
        );
    }
}

impl Drop for PreviewServer {
    fn drop(&mut self) {
        {
            let mut work = self.shared.work.lock();
            work.shutdown = true;
        }
        self.shared.wake.notify_all();
        for thread in std::mem::take(&mut *self.threads.lock()) {
            let _ = thread.join();
        }
    }
}

impl std::fmt::Debug for PreviewServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let work = self.shared.work.lock();
        f.debug_struct("PreviewServer")
            .field("session", &work.session.as_ref().map(|s| s.id))
            .field("playing", &work.playing)
            .field("cache", &self.shared.cache)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The render thread
// ---------------------------------------------------------------------------

struct Job {
    session: Arc<PreviewSession>,
    frame: i64,
    /// Scrub frames are rendered at reduced quality and reported immediately;
    /// playback frames go into the ring and wait for the pacer.
    scrub: bool,
}

/// The device the render thread draws with.
///
/// The application opens its own here: the preview is the first thing that
/// wants a GPU, and opening it on this thread keeps the cost off the command
/// that started the session.
#[cfg(not(test))]
fn render_device() -> Option<Arc<RenderContext>> {
    RenderContext::try_new().map(Arc::new)
}

/// The unit-test binary's single shared device.
///
/// A test process runs the compositor tests and this server in one address
/// space. With two live Vulkan instances there, the loader's validation layer
/// reaches a state where a `write_buffer` on one of them segfaults — measured
/// at four runs in forty, and not reproducible with a single instance. Since
/// the shipped application only ever has one preview server, one instance is
/// also what it does; this seam exists so the *test binary* matches it.
#[cfg(test)]
fn render_device() -> Option<Arc<RenderContext>> {
    crate::modules::render::test_context()
}

fn render_loop(shared: Arc<Shared>) {
    let Some(ctx) = render_device() else {
        shared.emit_error_once(PreviewError::NoDevice.to_string());
        return;
    };
    let compositor = Compositor::new(Arc::clone(&ctx));

    while let Some(job) = next_job(&shared) {
        render_one(&shared, &ctx, &compositor, job);
    }
}

/// Block until there is something to render, or the server is shutting down.
fn next_job(shared: &Shared) -> Option<Job> {
    let mut work = shared.work.lock();
    loop {
        if work.shutdown {
            return None;
        }

        let Some(session) = work.session.clone() else {
            shared.wake.wait(&mut work);
            continue;
        };

        if let Some(frame) = work.scrub.take() {
            return Some(Job {
                session,
                frame,
                scrub: true,
            });
        }

        if !work.playing {
            shared.wake.wait(&mut work);
            continue;
        }

        match pace(
            work.cursor,
            shared.clock.position(),
            session.fps,
            work.read_ahead,
        ) {
            Pacing::Render(frame) => {
                // Never render past the end; the pacer stops playback there.
                if session.duration > 0 && frame_time(frame, session.fps) >= session.duration {
                    shared.wake.wait_for(&mut work, IDLE_TICK);
                    continue;
                }
                work.cursor = frame + 1;
                return Some(Job {
                    session,
                    frame,
                    scrub: false,
                });
            }
            Pacing::Skip { from, to } => {
                tracing::debug!(
                    dropped = to - from,
                    from,
                    to,
                    "renderer fell behind; dropping late frames"
                );
                work.cursor = to;
            }
            Pacing::Idle => {
                shared.wake.wait_for(&mut work, IDLE_TICK);
            }
        }
    }
}

/// How many frames may be waiting to be JPEG-encoded at once.
///
/// Encoding runs off the render thread so that compositing the next frame and
/// encoding the last one overlap — with compositing around 17 ms and encoding
/// around 10 ms, doing them in sequence left almost nothing of the 33 ms
/// budget, and any hiccup cascaded. The cap exists because an unbounded queue
/// would let a slow encoder turn into unbounded memory: each pending frame
/// holds a full RGBA buffer.
const MAX_PENDING_ENCODES: usize = 3;

static PENDING_ENCODES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn render_one(shared: &Arc<Shared>, ctx: &Arc<RenderContext>, compositor: &Compositor, job: Job) {
    let session = &job.session;
    let time = frame_time(job.frame, session.fps);
    let quality = if job.scrub {
        session.scrub_quality
    } else {
        session.quality
    };
    // One resolution, whatever the frame is for.
    //
    // Playback used to render smaller than a parked frame, because at 1080x1920
    // the JPEG encode alone overran the 33 ms budget. With the encode on the
    // GPU it is about 7 ms and runs on another thread besides, so the reason is
    // gone and playback is as sharp as the project is. What is left in the
    // budget is compositing and decoding.
    let size = ctx.clamp_size(session.size);
    let sources = Arc::clone(&*shared.sources.read());

    let composite_started = std::time::Instant::now();
    let rgba = match compositor.render_frame(&session.project, time, size, sources.as_ref()) {
        Ok(rgba) => rgba,
        Err(error) => {
            let message = format!("cannot render the preview frame: {error}");
            if job.scrub {
                shared.emit_error_once(message);
            } else {
                tracing::warn!(%error, frame = job.frame, "preview render failed");
            }
            return;
        }
    };

    let composite_micros = composite_started.elapsed().as_micros();

    // Encoding moves off this thread so the next frame can be composited while
    // this one is turned into JPEG. Ordering does not matter: the ring is keyed
    // by frame number, so a frame that finishes late simply lands in its own
    // slot.
    //
    // When too many are already queued the encode is done inline instead. That
    // is deliberately a stall: it is the back-pressure that stops a slow
    // encoder from growing an unbounded queue of full-size RGBA buffers.
    let pending = PENDING_ENCODES.load(std::sync::atomic::Ordering::Relaxed);
    let inline = pending >= MAX_PENDING_ENCODES;

    let finish = {
        let shared = Arc::clone(shared);
        let session_id = session.id;
        let fps = session.fps;
        let frame_no = job.frame;
        let scrub = job.scrub;
        move || {
            let encode_started = std::time::Instant::now();
            let (bytes, backend) = match encode_preview_jpeg(&rgba, size.0, size.1, quality) {
                Ok(encoded) => encoded,
                Err(error) => {
                    shared.emit_error_once(error.to_string());
                    return;
                }
            };
            let encode_micros = encode_started.elapsed().as_micros();

            // The two halves are reported separately because they now run in
            // parallel: what fits in the budget is the larger of them, not the
            // sum. `over_budget` is judged on that.
            let slowest_ms = composite_micros.max(encode_micros) as f64 / 1000.0;
            tracing::debug!(
                session = session_id,
                frame = frame_no,
                at_ms = time / 1000,
                scrub,
                composite_ms = composite_micros as f64 / 1000.0,
                encode_ms = encode_micros as f64 / 1000.0,
                slowest_ms,
                budget_ms = frame_interval(fps) as f64 / 1000.0,
                over_budget = slowest_ms > frame_interval(fps) as f64 / 1000.0,
                bytes = bytes.len(),
                encoder = backend.label(),
                "preview frame ready"
            );

            let stored = shared.cache.insert(CachedFrame {
                session: session_id,
                frame: frame_no,
                time,
                bytes: Arc::from(bytes.into_boxed_slice()),
            });

            // A scrub frame is the one somebody is waiting to look at, so say so
            // the moment it exists. Playback frames are announced by the pacer
            // when they come due, not when they are made.
            if stored && scrub {
                shared.emit(PreviewEvent::Position {
                    session: session_id,
                    frame: frame_no,
                    time,
                    playing: shared.clock.is_playing(),
                });
            }
        }
    };

    if inline {
        finish();
    } else {
        PENDING_ENCODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        rayon::spawn(move || {
            finish();
            PENDING_ENCODES.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        });
    }
}

// ---------------------------------------------------------------------------
// The pacer thread
// ---------------------------------------------------------------------------

/// Turns clock position into "display frame N".
///
/// This is the presentation clock, and it is deliberately not the render loop:
/// when the time source becomes the audio device, this is the only thing that
/// has to notice.
fn pace_loop(shared: Arc<Shared>) {
    let mut last: Option<(u64, i64)> = None;

    loop {
        // Nothing to pace unless something is playing.
        let session = {
            let mut work = shared.work.lock();
            loop {
                if work.shutdown {
                    return;
                }
                if work.playing {
                    if let Some(session) = work.session.clone() {
                        break session;
                    }
                }
                last = None;
                shared.wake.wait(&mut work);
            }
        };

        let position = shared.clock.position();
        let frame = frame_at(position, session.fps);
        if last != Some((session.id, frame)) {
            last = Some((session.id, frame));
            shared.emit(PreviewEvent::Position {
                session: session.id,
                frame,
                time: position,
                playing: true,
            });
        }

        if session.duration > 0 && position >= session.duration {
            if let Some(audio) = shared.audio() {
                audio.pause();
            }
            shared.clock.pause();
            {
                let mut work = shared.work.lock();
                work.playing = false;
            }
            shared.wake.notify_all();
            shared.emit(PreviewEvent::Ended {
                session: session.id,
            });
            last = None;
            continue;
        }

        // Half a frame, so no frame is ever displayed a whole interval late,
        // capped so shutdown is never far away.
        let nap = Duration::from_micros((frame_interval(session.fps) / 2).max(1) as u64);
        std::thread::sleep(nap.min(PACER_TICK));
    }
}

// ---------------------------------------------------------------------------
// The protocol handler
// ---------------------------------------------------------------------------

/// Pull `(session, frame)` out of a frame URL.
///
/// Written against the string rather than a URL parser because the same logical
/// URL arrives in three shapes: `chukcut-frame://preview/1/2` on Linux and
/// macOS, `chukcut-frame://localhost/preview/1/2` when the webview normalizes
/// the authority, and `http://chukcut-frame.localhost/preview/1/2` on Windows.
/// Anchoring on the `preview` segment handles all three without caring which
/// part the platform called the host.
pub fn parse_frame_uri(uri: &str) -> Option<(u64, i64)> {
    let rest = uri.split_once("://").map(|(_, rest)| rest).unwrap_or(uri);
    let rest = rest.split(['?', '#']).next()?;
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let at = segments.iter().position(|s| *s == "preview")?;
    let session = segments.get(at + 1)?.parse::<u64>().ok()?;
    let frame = segments.get(at + 2)?.parse::<i64>().ok()?;
    Some((session, frame))
}

fn jpeg_response(bytes: &[u8]) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(tauri::http::StatusCode::OK)
        .header(tauri::http::header::CONTENT_TYPE, "image/jpeg")
        // Frame numbers are reused across sessions and the bytes behind one
        // change on every edit; a cached preview frame is always the wrong one.
        .header(tauri::http::header::CACHE_CONTROL, "no-store")
        .header(tauri::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(bytes.to_vec())
        .expect("static response parts are valid")
}

fn gone(session: u64) -> tauri::http::Response<Vec<u8>> {
    text_response(
        tauri::http::StatusCode::GONE,
        format!("preview session {session} has been superseded"),
    )
}

fn text_response(
    status: tauri::http::StatusCode,
    message: String,
) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(status)
        .header(tauri::http::header::CONTENT_TYPE, "text/plain")
        .header(tauri::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(message.into_bytes())
        .expect("static response parts are valid")
}

/// The `chukcut-frame://` handler, for `register_uri_scheme_protocol`.
///
/// Synchronous: it can block for up to [`FRAME_WAIT`] waiting on a frame. See
/// [`frame_protocol_async`] for the version that does not occupy the caller's
/// thread while it waits, which is the one to prefer.
pub fn frame_protocol<R: tauri::Runtime>(
    ctx: tauri::UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
) -> tauri::http::Response<Vec<u8>> {
    use tauri::Manager;

    let uri = request.uri().to_string();
    match ctx.app_handle().try_state::<Arc<PreviewServer>>() {
        Some(server) => server.serve_uri(&uri),
        None => text_response(
            tauri::http::StatusCode::SERVICE_UNAVAILABLE,
            "the preview server is not running".into(),
        ),
    }
}

/// [`frame_protocol`], for `register_asynchronous_uri_scheme_protocol`.
///
/// Answers on a scratch thread so that the brief wait for a frame that is
/// mid-render never sits on a webview thread.
pub fn frame_protocol_async<R: tauri::Runtime>(
    ctx: tauri::UriSchemeContext<'_, R>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    use tauri::Manager;

    let uri = request.uri().to_string();
    let server = ctx
        .app_handle()
        .try_state::<Arc<PreviewServer>>()
        .map(|state| Arc::clone(&state));

    // A rayon task rather than a fresh OS thread.
    //
    // Every displayed frame is one request, so at 30 fps this path ran thirty
    // thread spawns a second, each of which then went on to wait on a condition
    // variable. Thread creation is cheap but not free, and the jitter it adds
    // lands directly on the frame the user is waiting to see. Rayon's pool is
    // already a dependency and already sized to the machine.
    let started = std::time::Instant::now();
    rayon::spawn(move || {
        let response = match server {
            Some(server) => server.serve_uri(&uri),
            None => text_response(
                tauri::http::StatusCode::SERVICE_UNAVAILABLE,
                "the preview server is not running".into(),
            ),
        };
        // How long the webview waited for this frame, measured from the moment
        // the request reached us. If playback stutters while the renderer is
        // comfortably inside its budget, this is where the time went.
        let waited = started.elapsed();
        if waited > Duration::from_millis(16) {
            tracing::debug!(
                waited_ms = waited.as_secs_f64() * 1000.0,
                status = response.status().as_u16(),
                "slow frame request"
            );
        }
        responder.respond(response);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Segment, TimeRange, Track, TrackKind, Transform};
    use crate::modules::render::{SolidColorProvider, SolidSource};

    fn project() -> Arc<Project> {
        let mut project = Project::new(
            "preview test",
            CanvasConfig {
                width: 1920,
                height: 1080,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        );
        project
            .materials
            .videos
            .push(crate::modules::project::document::VideoMaterial {
                id: "clip".into(),
                path: "/nonexistent/clip.mp4".into(),
                width: 1920,
                height: 1080,
                duration: 4_000_000,
                fps: 30.0,
                has_audio: false,
                rotation: 0,
            });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "seg".into(),
            material_id: "clip".into(),
            target_range: TimeRange::new(0, 2_000_000),
            source_range: TimeRange::new(0, 2_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        project.tracks.push(track);
        Arc::new(project)
    }

    // -----------------------------------------------------------------------
    // URL parsing
    // -----------------------------------------------------------------------

    #[test]
    fn frame_urls_parse_in_every_shape_a_webview_produces() {
        assert_eq!(parse_frame_uri("chukcut-frame://preview/7/42"), Some((7, 42)));
        assert_eq!(
            parse_frame_uri("chukcut-frame://localhost/preview/7/42"),
            Some((7, 42))
        );
        assert_eq!(
            parse_frame_uri("http://chukcut-frame.localhost/preview/7/42"),
            Some((7, 42))
        );
        assert_eq!(
            parse_frame_uri("chukcut-frame://preview/7/42?t=123"),
            Some((7, 42)),
            "a cache-busting query string is not part of the address"
        );
    }

    #[test]
    fn malformed_frame_urls_are_rejected_rather_than_guessed_at() {
        assert_eq!(parse_frame_uri("chukcut-frame://preview/7"), None);
        assert_eq!(parse_frame_uri("chukcut-frame://preview/seven/42"), None);
        assert_eq!(parse_frame_uri("chukcut-frame://thumb/7/42"), None);
        assert_eq!(parse_frame_uri("chukcut-frame://preview/7/4.2"), None);
        assert_eq!(parse_frame_uri(""), None);
    }

    #[test]
    fn the_frame_url_matches_what_the_parser_expects() {
        let url = format!("{}/{}", frame_url(3), 12);
        assert_eq!(parse_frame_uri(&url), Some((3, 12)));
    }

    // -----------------------------------------------------------------------
    // Serving, without a GPU
    // -----------------------------------------------------------------------

    /// A server with a session installed but no threads running, so the cache
    /// can be driven by hand.
    fn served(session: u64) -> Arc<PreviewServer> {
        let server = PreviewServer::with_capacity(8);
        server.shared.cache.reset(session);
        server
    }

    #[test]
    fn a_cached_frame_is_served_as_jpeg() {
        let server = served(5);
        server.shared.cache.insert(CachedFrame {
            session: 5,
            frame: 2,
            time: 66_667,
            bytes: Arc::from(vec![0xFF, 0xD8, 0x00].into_boxed_slice()),
        });

        let response = server.serve_uri("chukcut-frame://preview/5/2");
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert_eq!(
            response.headers().get(tauri::http::header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        assert_eq!(response.body(), &vec![0xFF, 0xD8, 0x00]);
    }

    #[test]
    fn a_frame_from_a_superseded_session_is_gone_not_a_stale_image() {
        let server = served(5);
        server.shared.cache.insert(CachedFrame {
            session: 5,
            frame: 2,
            time: 0,
            bytes: Arc::from(vec![1, 2, 3].into_boxed_slice()),
        });
        // The user seeked: session 6 is live now.
        server.shared.cache.reset(6);

        let response = server.serve_uri("chukcut-frame://preview/5/2");
        assert_eq!(response.status(), tauri::http::StatusCode::GONE);
        assert_ne!(response.body(), &vec![1, 2, 3]);
    }

    #[test]
    fn a_frame_that_will_never_arrive_gives_up_rather_than_hanging() {
        let server = served(5);
        let started = std::time::Instant::now();
        let response = server.serve_uri("chukcut-frame://preview/5/9");

        // No content, not an error: nothing has been rendered for this session
        // yet, and a failed resource load is what kills the WebKitGTK web
        // process. What matters as much is that the answer arrives at all —
        // a protocol handler that blocks indefinitely blocks the webview.
        assert_eq!(response.status(), tauri::http::StatusCode::NO_CONTENT);
        assert!(response.body().is_empty());
        assert!(
            started.elapsed() < FRAME_WAIT * 4,
            "waited {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_frame_that_is_not_ready_is_answered_with_the_nearest_one_that_is() {
        let server = served(5);
        for frame in [10i64, 12] {
            server.shared.cache.insert(CachedFrame {
                session: 5,
                frame,
                time: frame * 33_333,
                bytes: Arc::from(vec![0xFF, 0xD8, frame as u8].into_boxed_slice()),
            });
        }

        // Frame 11 was never rendered. Serving its neighbour is a few
        // milliseconds stale and indistinguishable from a late frame; serving
        // an error is a step towards a dead web process.
        let response = server.serve_uri("chukcut-frame://preview/5/11");
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert!(
            response.body() == &vec![0xFF, 0xD8, 10] || response.body() == &vec![0xFF, 0xD8, 12],
            "expected one of the two neighbours, got {:?}",
            response.body()
        );

        // A superseded session still wins over the nearest frame: a stale
        // picture from before a seek is the one thing that must never be shown.
        server.shared.cache.reset(6);
        assert_eq!(
            server.serve_uri("chukcut-frame://preview/5/11").status(),
            tauri::http::StatusCode::GONE
        );
    }

    #[test]
    fn a_frame_that_lands_while_the_request_waits_is_served() {
        let server = served(5);
        let writer = Arc::clone(&server);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            writer.shared.cache.insert(CachedFrame {
                session: 5,
                frame: 1,
                time: 33_333,
                bytes: Arc::from(vec![0xFF, 0xD8].into_boxed_slice()),
            });
        });

        let response = server.serve_uri("chukcut-frame://preview/5/1");
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        handle.join().unwrap();
    }

    #[test]
    fn a_malformed_url_is_a_bad_request() {
        let server = served(5);
        let response = server.serve_uri("chukcut-frame://preview/oops");
        assert_eq!(response.status(), tauri::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_request_before_any_session_exists_is_gone() {
        let server = PreviewServer::with_capacity(8);
        let response = server.serve_uri("chukcut-frame://preview/1/0");
        assert_eq!(response.status(), tauri::http::StatusCode::GONE);
    }

    // -----------------------------------------------------------------------
    // The whole pipeline, when the machine can render
    // -----------------------------------------------------------------------

    #[test]
    fn a_session_renders_and_serves_the_frame_under_the_playhead() {
        if crate::modules::render::test_context().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }
        let (server, _time, _exclusive) = shared_server();
        server.set_source_provider(Arc::new(
            SolidColorProvider::new().with("clip", SolidSource::new([1.0, 0.0, 0.0, 1.0], 1920, 1080)),
        ));

        // No Tauri app in a unit test, so the session is installed directly;
        // `start` differs only in that it also stores the event channel.
        let info = server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            1_000_000,
            false,
        );
        // Native, because the proxy table stops reducing anything at or below
        // a 1920 long edge. This assertion previously expected 960x540 and was
        // left behind when the table changed.
        assert_eq!(info.width, 1920, "a 1080p canvas previews natively");
        assert_eq!(info.height, 1080);
        assert_eq!(info.frame, 30);

        // Opening the device happens on the render thread and can take longer
        // than one request's patience, so retry rather than race it.
        let url = format!("{}/{}", info.frame_url, info.frame);
        let mut response = server.serve_uri(&url);
        for _ in 0..40 {
            if response.status() == tauri::http::StatusCode::OK {
                break;
            }
            response = server.serve_uri(&url);
        }
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert!(super::super::encoder::is_jpeg(response.body()));

        server.stop();
    }

    /// One live server at a time, and only ever one of them.
    ///
    /// `render_loop` opens its own `RenderContext`, so a server per test means
    /// a Vulkan instance created and torn down per test — and doing that
    /// alongside the instance the compositor tests share segfaults inside the
    /// driver often enough to make `cargo test` unreliable. Measured on this
    /// machine: four failures in forty runs with a server per test, none in
    /// forty without.
    ///
    /// The application runs exactly one preview server for its whole life, so
    /// one here is also the more honest arrangement. Tests take it in turn and
    /// install their own session with `adopt`, which supersedes whatever the
    /// last test left — which is all a seek does anyway.
    static ONE_SERVER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The shared server, its clock, and exclusive use of both.
    ///
    /// The lock is deliberately poison-tolerant: a test that failed while
    /// holding it must not turn every later test into a secondary failure.
    fn shared_server() -> (
        Arc<PreviewServer>,
        Arc<crate::modules::preview::clock::ManualSource>,
        std::sync::MutexGuard<'static, ()>,
    ) {
        static SERVER: std::sync::OnceLock<(Arc<PreviewServer>, Arc<crate::modules::preview::clock::ManualSource>)> =
            std::sync::OnceLock::new();

        let guard = ONE_SERVER.lock().unwrap_or_else(|e| e.into_inner());
        let (server, time) = SERVER.get_or_init(|| {
            let time = Arc::new(crate::modules::preview::clock::ManualSource::new());
            let server = PreviewServer::with_time_source(DEFAULT_CAPACITY, Arc::clone(&time) as _);
            (server, time)
        });
        // Threads are respawned per test because each test ends by stopping
        // them. That is what keeps a render from being in flight when the
        // process exits — an abrupt teardown in the middle of a `write_buffer`
        // is a segfault, and it is the reason every one of these tests ends
        // with `stop()`. The *device* survives it: `render_device` hands out a
        // clone of the binary's shared context, so stopping drops the
        // compositor and not the Vulkan instance.
        server.spawn_threads();
        (Arc::clone(server), Arc::clone(time), guard)
    }

    /// How long a test will wait for the renderer before calling it hung.
    ///
    /// Not a synchronisation device — every wait below blocks on the ring's
    /// condvar and returns the instant the frame lands. This is only the bound
    /// that turns "the render thread died" into a failure rather than a suite
    /// that never finishes.
    const RENDER_DEADLINE: Duration = Duration::from_secs(30);

    /// Block until every frame in `frames` is in the ring.
    ///
    /// Every one of them, not just the last: encoding runs off the render
    /// thread, so frames finish out of order and waiting on the highest number
    /// says nothing about the ones below it.
    #[track_caller]
    fn await_frames(server: &PreviewServer, session: u64, frames: std::ops::Range<i64>) {
        for frame in frames {
            let found = server.cache().wait(session, frame, RENDER_DEADLINE);
            assert!(
                found.is_hit(),
                "frame {frame} never arrived (ring holds {:?})",
                server.cache().frames()
            );
        }
    }

    #[test]
    fn playback_fills_the_ring_ahead_of_the_playhead() {
        if crate::modules::render::test_context().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }
        // The clock reads from a source this test owns, so "the renderer runs
        // twelve frames ahead of the playhead" is an exact statement rather
        // than a guess about how long a contended GPU takes. The old version
        // of this test slept in fifty-millisecond steps and compared the ring
        // size against a wall-clock playhead, which made it a coin toss
        // whenever the machine was busy.
        let (server, time, _exclusive) = shared_server();
        server.set_source_provider(Arc::new(EmptySourceProvider));
        let info = server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            0,
            false,
        );
        server.play().expect("session is open");

        // The playhead is at frame 0 and stays there. `pace` renders frames
        // `cursor .. current + read_ahead` and idles beyond, so the ring must
        // settle at exactly frames 0..12 — no more, however fast the GPU is,
        // and no fewer, however slow.
        let lead = DEFAULT_READ_AHEAD as i64;
        await_frames(&server, info.session, 0..lead);
        assert_eq!(
            server.cache().frames(),
            (0..lead).collect::<Vec<i64>>(),
            "the renderer did not stop at its read-ahead limit"
        );
        assert_eq!(
            server.clock().position(),
            0,
            "the clock moved on its own; the injected source is not in control"
        );

        // Move the playhead ten frames. The renderer must extend the ring by
        // ten, not restart it and not run away.
        time.set(frame_time(10, 30.0));
        assert_eq!(server.clock().frame(), 10);
        await_frames(&server, info.session, lead..10 + lead);
        assert_eq!(
            server.cache().frames(),
            (0..10 + lead).collect::<Vec<i64>>(),
            "the ring did not follow the playhead"
        );

        // Every frame the ring holds is at or ahead of the playhead, which is
        // the property that makes playback smooth rather than merely possible.
        assert!(
            server
                .cache()
                .frames()
                .iter()
                .all(|frame| *frame >= server.clock().frame() - 10),
            "the ring is behind the playhead"
        );

        server.pause().expect("session is open");
        assert!(!server.clock().is_playing());

        // Pausing stops the renderer where it is: time can pass without the
        // ring growing, because a paused clock does not advance.
        let frozen = server.cache().frames();
        time.advance(5_000_000);
        assert_eq!(server.clock().frame(), 10, "a paused clock does not move");
        assert_eq!(server.cache().frames(), frozen);

        server.stop();
    }

    #[test]
    fn seeking_supersedes_the_session_and_the_old_url_stops_working() {
        if crate::modules::render::test_context().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }
        let (server, _time, _exclusive) = shared_server();
        let first = server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            0,
            false,
        );
        let second = server.seek(1_000_000).expect("session is open");
        assert!(second.session > first.session);

        let stale = server.serve_uri(&format!("{}/{}", first.frame_url, first.frame));
        assert_eq!(stale.status(), tauri::http::StatusCode::GONE);

        server.stop();
    }
}
