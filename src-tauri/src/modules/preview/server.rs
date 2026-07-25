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
    frame_at, frame_interval, frame_time, pace, Pacing, PlaybackClock, DEFAULT_READ_AHEAD,
};
use super::encoder::encode_jpeg;
use super::error::{PreviewError, Result};
use super::session::{PreviewOptions, PreviewSession};
use crate::modules::project::document::{Micros, Project};
use crate::modules::render::{Compositor, EmptySourceProvider, RenderContext, SourceProvider};

/// The registered URI scheme. Must match `register_uri_scheme_protocol` in
/// `lib.rs` and the `frameUrl` the frontend builds.
pub const SCHEME: &str = "chukcut-frame";

/// How long a frame request waits for a frame that is being rendered before it
/// gives up with a 404.
pub const FRAME_WAIT: Duration = Duration::from_millis(60);

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
        Arc::new(Self {
            shared: Arc::new(Shared {
                work: Mutex::new(Work {
                    read_ahead: DEFAULT_READ_AHEAD,
                    ..Work::default()
                }),
                wake: Condvar::new(),
                cache: Arc::new(FrameCache::new(capacity)),
                clock: PlaybackClock::new(30.0, 0),
                sources: RwLock::new(Arc::new(EmptySourceProvider)),
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
    pub fn request_frame(&self, session: u64, frame: i64) {
        let mut work = self.shared.work.lock();
        let Some(live) = work.session.as_ref() else {
            return;
        };
        if live.id != session {
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

        // Not rendered yet. Nudge the renderer, wait briefly, then admit it.
        self.request_frame(session, frame);
        match self.shared.cache.wait(session, frame, FRAME_WAIT) {
            Lookup::Hit(bytes) => jpeg_response(&bytes),
            Lookup::Stale => gone(session),
            Lookup::Miss => text_response(
                tauri::http::StatusCode::NOT_FOUND,
                format!("frame {frame} of session {session} is not ready"),
            ),
        }
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

fn render_loop(shared: Arc<Shared>) {
    let Some(ctx) = RenderContext::try_new() else {
        shared.emit_error_once(PreviewError::NoDevice.to_string());
        return;
    };
    let ctx = Arc::new(ctx);
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

fn render_one(shared: &Shared, ctx: &Arc<RenderContext>, compositor: &Compositor, job: Job) {
    let session = &job.session;
    let time = frame_time(job.frame, session.fps);
    let quality = if job.scrub {
        session.scrub_quality
    } else {
        session.quality
    };
    // The proxy size comes from the canvas, which the device may not be able to
    // allocate on a very restricted adapter.
    let size = ctx.clamp_size(session.size);
    let sources = Arc::clone(&*shared.sources.read());

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

    let bytes = match encode_jpeg(&rgba, size.0, size.1, quality) {
        Ok(bytes) => bytes,
        Err(error) => {
            shared.emit_error_once(error.to_string());
            return;
        }
    };

    let stored = shared.cache.insert(CachedFrame {
        session: session.id,
        frame: job.frame,
        time,
        bytes: Arc::from(bytes.into_boxed_slice()),
    });

    // A scrub frame is the one somebody is waiting to look at, so say so the
    // moment it exists. Playback frames are announced by the pacer when they
    // come due, not when they are made.
    if stored && job.scrub {
        shared.emit(PreviewEvent::Position {
            session: session.id,
            frame: job.frame,
            time,
            playing: shared.clock.is_playing(),
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

    std::thread::spawn(move || {
        let response = match server {
            Some(server) => server.serve_uri(&uri),
            None => text_response(
                tauri::http::StatusCode::SERVICE_UNAVAILABLE,
                "the preview server is not running".into(),
            ),
        };
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
    fn a_frame_that_will_never_arrive_is_a_404_rather_than_a_hang() {
        let server = served(5);
        let started = std::time::Instant::now();
        let response = server.serve_uri("chukcut-frame://preview/5/9");
        assert_eq!(response.status(), tauri::http::StatusCode::NOT_FOUND);
        assert!(
            started.elapsed() < FRAME_WAIT * 4,
            "waited {:?}",
            started.elapsed()
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
        if RenderContext::try_new().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }

        let server = PreviewServer::new();
        server.set_source_provider(Arc::new(
            SolidColorProvider::new().with("clip", SolidSource::new([1.0, 0.0, 0.0, 1.0], 1920, 1080)),
        ));

        // No Tauri app in a unit test, so the session is installed directly;
        // `start` differs only in that it also stores the event channel.
        server.spawn_threads();
        let info = server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            1_000_000,
            false,
        );
        assert_eq!(info.width, 960, "1080p canvas previews at a 960 long edge");
        assert_eq!(info.height, 540);
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

    #[test]
    fn playback_fills_the_ring_ahead_of_the_playhead() {
        if RenderContext::try_new().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }

        let server = PreviewServer::new();
        server.spawn_threads();
        server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            0,
            false,
        );
        server.play().expect("session is open");

        // Long enough for the device to open and the renderer to get going.
        for _ in 0..40 {
            if server.cache().len() > 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        let cached = server.cache().len();
        let position = server.clock().position();
        assert!(cached > 1, "the ring only holds {cached} frames");
        assert!(position > 0, "the clock did not advance");
        assert!(
            cached <= DEFAULT_READ_AHEAD + 2,
            "the renderer ran {cached} frames ahead of a {DEFAULT_READ_AHEAD}-frame lead"
        );

        server.pause().expect("session is open");
        assert!(!server.clock().is_playing());
        server.stop();
    }

    #[test]
    fn seeking_supersedes_the_session_and_the_old_url_stops_working() {
        if RenderContext::try_new().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }

        let server = PreviewServer::new();
        server.spawn_threads();
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
