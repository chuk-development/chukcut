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
use super::ladder::{self, Ladder};
use super::probe::{self, PROBE};
use super::session::{PreviewOptions, PreviewSession, Viewport};
use super::stats::{self, PlaybackStats, Rendered, SeekKind, SeekWatch, SessionFacts};
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

/// How far behind the playhead a *composited* frame may be and still be worth
/// a JPEG.
///
/// One frame, tied to [`NEAREST_TOLERANCE`]: a frame one interval late is still
/// served — as the nearest neighbour of the frame the webview asked for — so
/// encoding it is not waste. It also keeps the renderer out of a livelock. A
/// renderer that is consistently one frame behind would, at zero tolerance,
/// discard every frame it ever finished and the picture would stop entirely
/// while the machine stayed busy.
const LATE_ENCODE_TOLERANCE: i64 = 1;

/// How long the render thread sleeps when the ring is full enough. Short, so
/// that a seek or a pause is acted on promptly.
const IDLE_TICK: Duration = Duration::from_millis(4);

/// The two savings that can be turned off, so that both arms of them can be
/// measured in one process.
///
/// A claim that a change made something faster is worth what its measurement
/// is worth, and a measurement taken against a build that no longer exists is
/// worth very little. `examples/preview_waste.rs` runs the same pipeline with
/// each of these on and off and prints the difference; nothing else ever calls
/// them, and both default to on.
pub mod experiment {
    use std::sync::atomic::{AtomicBool, Ordering};

    static SKIP_LATE_ENCODES: AtomicBool = AtomicBool::new(true);
    static DEDUPE_SCRUBS: AtomicBool = AtomicBool::new(true);

    /// Throw away a composited frame the playhead has already passed instead of
    /// spending a JPEG on it.
    pub fn set_skip_late_encodes(on: bool) {
        SKIP_LATE_ENCODES.store(on, Ordering::Relaxed);
    }

    pub fn skip_late_encodes() -> bool {
        SKIP_LATE_ENCODES.load(Ordering::Relaxed)
    }

    /// Refuse a frame request for a frame that is already being rendered.
    pub fn set_dedupe_scrubs(on: bool) {
        DEDUPE_SCRUBS.store(on, Ordering::Relaxed);
    }

    pub fn dedupe_scrubs() -> bool {
        DEDUPE_SCRUBS.load(Ordering::Relaxed)
    }
}

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
    /// `(session, frame)` of the last position announced to the frontend.
    ///
    /// Exists so that [`Shared::emit_position`] can decide and send under one
    /// lock. See that function for why a decision taken outside it is not
    /// enough.
    announced: Mutex<Option<(u64, i64)>>,
    /// What the log says about playback when nobody has `RUST_LOG` set. See
    /// [`stats`].
    stats: PlaybackStats,
    /// How long the picture takes to catch up with a moved playhead.
    seeks: SeekWatch,
    /// What playback is currently giving up to keep up. See [`ladder`].
    ///
    /// Its own mutex rather than a field of `Work` because the encode thread
    /// steps it, and that thread must not take the lock the render thread holds
    /// while it waits for work. A leaf, like `stats`: nothing under it takes
    /// another lock.
    ladder: Mutex<Ladder>,
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
    /// The scrub frame the render thread is working on right now, cleared when
    /// its bytes reach the ring.
    ///
    /// Exists so a second request for a frame that is already being rendered
    /// does not queue a second render of it. The webview asks again while it
    /// waits — `serve_uri` answers a miss and the viewer retries — and without
    /// this each retry composited the same frame again, which on a cold seek is
    /// the most expensive frame there is.
    rendering: Option<i64>,
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
            let sent = std::time::Instant::now();
            if let Err(error) = channel.send(event) {
                tracing::warn!(%error, "preview event channel is gone");
            }
            probe::add(&PROBE.emit_ns, sent);
            probe::bump(&PROBE.emits);
        }
    }

    /// Announce where the playhead is, without ever moving it backwards by
    /// accident.
    ///
    /// Two threads announce positions and they do not agree about time. The
    /// **pacer** says where playback is, at the frame rate. The **render
    /// thread** announces a *scrub* frame the moment its JPEG exists, from the
    /// encode thread, after the encode — because a parked frame is the one
    /// somebody is waiting to look at and it should not have to wait for the
    /// pacer's next tick.
    ///
    /// The parked frame of a new session is a scrub. Press play immediately and
    /// the pacer can announce frames 1 and 2 before frame 0's announcement is
    /// emitted, and the frontend's playhead jumps backwards for one frame.
    /// That was latent for as long as opening a render device per server cost
    /// enough to order the two; `gpu::render_context` made it free, and
    /// `tests/preview.rs::position_updates_arrive_in_order_as_the_playhead_advances`
    /// began failing almost every run.
    ///
    /// The first fix compared the frame against the clock and then emitted.
    /// **That is a check the pacer can invalidate before the send happens**:
    /// both threads reach `Channel::send` through an `RwLock` *read* guard, so
    /// nothing serialises them and the window between deciding and sending is
    /// exactly the window the pacer needs. Deciding and sending under one lock
    /// is what closes it.
    ///
    /// `authoritative` marks an announcement that *defines* the playhead — the
    /// pacer's, and a transport command's. Those always go out and reset the
    /// mark, so a backwards seek or a replay from the end is still announced. A
    /// scrub is subordinate: it is dropped if a later frame of the same session
    /// has already been sent, because the pacer has already said everything it
    /// could add. A different session id is never compared against, since a
    /// seek supersedes the session and starts the ordering again.
    fn emit_position(
        &self,
        session: u64,
        frame: i64,
        time: Micros,
        playing: bool,
        authoritative: bool,
    ) {
        let mut announced = self.announced.lock();
        if !authoritative {
            if let Some((last_session, last_frame)) = *announced {
                if last_session == session && last_frame > frame {
                    return;
                }
            }
        }
        *announced = Some((session, frame));
        // Inside the lock on purpose: the order these reach the channel has to
        // be the order they were decided in, or the guard above is decoration.
        self.emit(PreviewEvent::Position {
            session,
            frame,
            time,
            playing,
        });
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
                announced: Mutex::new(None),
                stats: PlaybackStats::new(),
                seeks: SeekWatch::new(),
                ladder: Mutex::new(Ladder::new()),
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

    /// The playback counters behind the summary line.
    ///
    /// For a harness that wants the numbers rather than the log: the counts of
    /// what was shown, dropped, discarded and rendered are the only way to say
    /// whether a change to the pipeline did what it claims.
    /// `examples/preview_waste.rs` reads them; the application only ever emits
    /// them.
    pub fn stats(&self) -> &PlaybackStats {
        &self.shared.stats
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
        self.adopt(session, at, false, SeekKind::SessionStart)
    }

    /// Move the playhead. Supersedes the session so no frame rendered for the
    /// old position can be served against the new one.
    pub fn seek(&self, to: Micros) -> Result<PreviewInfo> {
        let (previous, playing) = {
            let work = self.shared.work.lock();
            let session = work.session.clone().ok_or(PreviewError::NoSession)?;
            (session, work.playing)
        };
        Ok(self.adopt(Arc::new(previous.superseded()), to, playing, SeekKind::Seek))
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

        // Every run starts at the top of the ladder. Carrying a rung over from
        // the last one would mean a machine that stuttered once previews softly
        // for the rest of the session, and the user has no way to ask for it
        // back.
        self.shared.ladder.lock().reset();

        // The window starts here, not when the session opened: a session that
        // sat parked for a minute would otherwise put that minute in the first
        // summary's `window_ms`.
        self.shared.stats.start_window(std::time::Instant::now());

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

        // The last word on the run that just ended. Emitted with no lock held —
        // `finish` folds and resets under its own and hands the numbers back.
        if let Some(summary) = self.shared.stats.finish(std::time::Instant::now()) {
            stats::emit(&summary, stats::Reason::Stopped);
        }

        // A paused frame is the one the user sits and looks at, so it is the
        // one thing the quality ladder is never allowed to affect. If playback
        // gave anything up, the ring is full of frames that are softer than the
        // project and the frame under the playhead is one of them — so the
        // session is superseded (which empties the ring) and that frame is
        // rendered again at the session's own size and `scrub_quality`.
        //
        // Only when it actually degraded: a run that held the top rung
        // throughout already has the right picture in the ring, and superseding
        // for it would throw away the read-ahead a resume is about to need.
        if self.shared.ladder.lock().reset() {
            let refreshed = Arc::new(session.superseded());
            tracing::debug!(
                session = refreshed.id,
                width = refreshed.width(),
                height = refreshed.height(),
                "re-rendering the paused frame at full quality"
            );
            let info = self.readopt(refreshed, false);
            self.shared
                .emit_position(info.session, info.frame, info.position, false, true);
            return Ok(info);
        }

        let info = self.info(&session);
        // Authoritative: pausing *is* where the playhead is now.
        self.shared
            .emit_position(session.id, info.frame, info.position, false, true);
        Ok(info)
    }

    /// Tell the preview how big the panel showing it is, in device pixels.
    ///
    /// This is what stops the compositor and the JPEG encoder from working at
    /// eight times the pixels the screen can display. `None` goes back to
    /// sizing from the canvas alone.
    ///
    /// A no-op when the size does not change, which is most calls: the frontend
    /// debounces a window drag, but a debounce still delivers several sizes and
    /// only some of them cross a rounding boundary. Restarting the pipeline for
    /// the rest would empty the ring for nothing.
    pub fn set_viewport(&self, viewport: Option<Viewport>) -> Result<PreviewInfo> {
        let (session, playing) = {
            let work = self.shared.work.lock();
            let session = work.session.clone().ok_or(PreviewError::NoSession)?;
            (session, work.playing)
        };
        let Some(resized) = session.with_viewport(viewport) else {
            return Ok(self.info(&session));
        };
        tracing::info!(
            width = resized.width(),
            height = resized.height(),
            was_width = session.width(),
            was_height = session.height(),
            canvas_width = resized.project.canvas.width,
            canvas_height = resized.project.canvas.height,
            "preview resized to the panel"
        );
        Ok(self.readopt(Arc::new(resized), playing))
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

        // After the threads have joined, so nothing can still be counting into
        // the window this summarises.
        self.shared.seeks.clear();
        if let Some(summary) = self.shared.stats.finish(std::time::Instant::now()) {
            stats::emit(&summary, stats::Reason::Stopped);
        }
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
        // Already being made. The webview asks again while it waits — the
        // handler answers a miss after `FRAME_WAIT` and the viewer retries —
        // and queueing a second render of a frame that is halfway through the
        // first one is the most expensive possible way to answer, because a
        // scrub job seeks the decoder and the frame it is seeking to is the one
        // the decoder is already on.
        if work.rendering == Some(frame) && experiment::dedupe_scrubs() {
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

        let served = std::time::Instant::now();
        probe::bump(&PROBE.serve_calls);
        match self.shared.cache.get(session, frame) {
            Lookup::Hit(bytes) => {
                probe::bump(&PROBE.serve_hit);
                probe::add(&PROBE.serve_ns, served);
                return jpeg_response(&bytes);
            }
            Lookup::Stale => {
                probe::bump(&PROBE.serve_stale);
                probe::add(&PROBE.serve_ns, served);
                return gone(session);
            }
            Lookup::Miss => {}
        }

        // Not rendered yet. Nudge the renderer and wait briefly.
        self.request_frame(session, frame);
        let waited = std::time::Instant::now();
        let found = self.shared.cache.wait(session, frame, FRAME_WAIT);
        probe::add(&PROBE.serve_wait_ns, waited);
        match found {
            Lookup::Hit(bytes) => {
                probe::bump(&PROBE.serve_wait_hit);
                probe::add(&PROBE.serve_ns, served);
                return jpeg_response(&bytes);
            }
            Lookup::Stale => {
                probe::bump(&PROBE.serve_stale);
                probe::add(&PROBE.serve_ns, served);
                return gone(session);
            }
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
        if let Some((neighbour, bytes)) = self.shared.cache.nearest(session, frame, NEAREST_TOLERANCE)
        {
            if neighbour != frame {
                tracing::debug!(
                    requested = frame,
                    served = neighbour,
                    "served a neighbouring frame; the renderer is behind"
                );
            }
            probe::bump(&PROBE.serve_nearest);
            probe::add(&PROBE.serve_ns, served);
            return jpeg_response(&bytes);
        }
        probe::bump(&PROBE.serve_empty);
        probe::add(&PROBE.serve_ns, served);

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
    fn adopt(
        &self,
        session: Arc<PreviewSession>,
        at: Micros,
        keep_playing: bool,
        kind: SeekKind,
    ) -> PreviewInfo {
        // Read before anything moves the clock: this is where the playhead was,
        // and the *direction* is what makes a seek expensive — a backward one
        // discards the ring and costs the decoder a real seek.
        let from = self.shared.clock.position();

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

        // Armed before the render thread is woken, or a decoder that is already
        // warm could deliver the frame before there is anything timing it.
        self.shared.seeks.arm(
            session.id,
            frame,
            from,
            self.shared.clock.position(),
            kind,
            std::time::Instant::now(),
        );
        self.shared.stats.begin_session(
            SessionFacts {
                width: session.width(),
                height: session.height(),
                canvas: (session.project.canvas.width, session.project.canvas.height),
            },
            session.fps,
            std::time::Instant::now(),
        );

        {
            let mut work = self.shared.work.lock();
            work.session = Some(Arc::clone(&session));
            work.scrub = Some(frame);
            work.rendering = None;
            work.cursor = frame;
            work.playing = keep_playing;
            work.reported_error = false;
        }
        self.shared.wake.notify_all();

        self.info(&session)
    }

    /// Replace the live session with one that differs only in how it is
    /// rendered, leaving the playhead exactly where it is.
    ///
    /// [`Self::adopt`] is for a *move*: it seeks the clock and the audio engine
    /// and arms the seek watch. A resize and the re-render after a pause move
    /// nothing — the picture has to change, the time does not — and putting
    /// them through `adopt` would seek the audio engine for a window drag.
    ///
    /// What it still has to do is supersede: the ring holds frames at the old
    /// size or the old quality, and a frame URL is answered from the ring
    /// without anything looking at how big the picture in it is.
    fn readopt(&self, session: Arc<PreviewSession>, keep_playing: bool) -> PreviewInfo {
        self.shared.cache.reset(session.id);
        self.shared.stats.begin_session(
            SessionFacts {
                width: session.width(),
                height: session.height(),
                canvas: (session.project.canvas.width, session.project.canvas.height),
            },
            session.fps,
            std::time::Instant::now(),
        );

        let frame = self.shared.clock.frame();
        {
            let mut work = self.shared.work.lock();
            work.session = Some(Arc::clone(&session));
            work.scrub = Some(frame);
            work.rendering = None;
            work.cursor = frame;
            work.playing = keep_playing;
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

/// The device the render thread draws with: the process's, not its own.
///
/// This used to open one here, with a `cfg(test)` seam that shared instead,
/// because two live Vulkan instances in one address space reach a state where a
/// `write_buffer` on one of them segfaults — measured at four runs in forty,
/// and not reproducible with a single instance. The seam has been removed
/// because the *application* had the same shape the test binary did: an export
/// builds its own compositor while this thread is running, so a preview and an
/// export meant two devices. `gpu` owns the only one now and there is nothing
/// left to keep in step.
///
/// Still lazy and still opened from this thread the first time a session needs
/// a frame, which keeps the cost off the command that started the session.
fn render_device() -> Option<Arc<RenderContext>> {
    crate::modules::gpu::render_context()
}

fn render_loop(shared: Arc<Shared>) {
    let Some(ctx) = render_device() else {
        shared.emit_error_once(PreviewError::NoDevice.to_string());
        return;
    };
    let compositor = Compositor::new(Arc::clone(&ctx));

    // The decode path, once, from the only thread that has a device to ask.
    //
    // At INFO with its reason because "software" here is a 20× per-frame
    // regression (`docs/research/hardware-decode.md`) and it is invisible from
    // the outside: the preview looks identical, it is only slower. A log that
    // does not say which decoder ran cannot answer "why was it slow".
    let (path, why) = stats::decode_path(ctx.can_import_dmabuf());
    shared.stats.set_decode_path(path);
    tracing::info!(decode = path.label(), reason = why, "preview decode path");

    // Whether the frame before this one was composited and then thrown away for
    // being late. The render thread's own state, so no lock and no atomic: it
    // is the only thread that reads or writes it. See [`render_one`] for what
    // it guarantees.
    let mut discarded_last = false;

    loop {
        // Timed around `next_job` rather than inside it, so one counter covers
        // every reason this thread is not rendering: no session, paused, or the
        // read-ahead window already full. See `probe`: that last case is the
        // one that says the renderer is not the bottleneck.
        let waited = std::time::Instant::now();
        let Some(job) = next_job(&shared) else {
            return;
        };
        probe::add(&PROBE.render_wait_ns, waited);
        if job.scrub {
            probe::bump(&PROBE.scrub_jobs);
        }
        discarded_last = render_one(&shared, &ctx, &compositor, job, discarded_last);
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
            // Held until the bytes are in the ring, so a retry for the same
            // frame while this one is in flight is answered by the ring rather
            // than by a second render. See `Work::rendering`.
            work.rendering = Some(frame);
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
                // Counted under the `work` guard, which is safe because the
                // stats mutex is a leaf: nothing under it takes another lock,
                // so there is no order to invert. It is a single add and it
                // never logs — the count reaches the file in the next summary.
                shared.stats.record_dropped(to - from);
                // The ladder is a leaf for the same reason. A drop is the
                // loudest evidence there is that the renderer cannot hold this
                // size, so it steps down at once rather than after a streak.
                // Silently: this runs under the `work` guard, and a `tracing`
                // call here would be a `write` syscall with the render thread's
                // own lock held. The rung reaches the file on the next summary
                // line, and the step itself is logged from the encode thread.
                let _ = shared.ladder.lock().dropped(to - from);
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

/// One encode, ready to run on the encode thread.
type EncodeJob = Box<dyn FnOnce() + Send + 'static>;

/// The thread that turns composited frames into JPEG, and the queue into it.
///
/// **This used to be `rayon::spawn`, and that is what made the preview hang.**
/// The symptom pointed nowhere near the cause, so it is worth the space:
///
/// - `encoder.rs` holds one process-wide mutex around the one hardware JPEG
///   encoder, and the RGBA→NV12 conversion used to run *inside* it — and that
///   conversion is a **rayon parallel iterator**.
/// - A rayon worker that blocks inside a parallel iterator does not idle. It
///   joins the work-stealing loop and runs *any* other job in the pool —
///   including another encode `rayon::spawn`ed here, which asks for the mutex
///   the very same thread is holding. `parking_lot::Mutex` is not reentrant, so
///   the thread parks forever and never releases the lock.
///
/// From the outside: five frames and then nothing for thirty seconds with
/// `playing=true`, every thread in `futex_wait`, no GPU work in flight.
/// `PENDING_ENCODES` never falls, so [`render_one`] switches to encoding inline
/// and the render thread blocks on the same lock too.
/// `CHUKCUT_PREVIEW_JPEG=hardware` on `tests/preview.rs` hung 7 runs out of 7.
///
/// The conversion has since moved out from under the lock, which is the fix
/// that actually closes the class — see `encoder.rs`'s `HARDWARE`. **This
/// change is still worth having on its own**, and the reason is the second
/// deadlock that one revealed: a *non*-rayon thread that dispatches under the
/// lock waits for a worker, and if the pool is meanwhile full of jobs waiting
/// for that lock, nobody moves. Keeping encodes out of the pool means the pool
/// cannot fill with them.
///
/// One thread, not several: the hardware encoder is one non-reentrant device
/// whose mutex serialised these encodes anyway, and one encode is 6 ms hardware
/// or 10 ms software against a 33 ms budget, so a second thread would only wait
/// for the first.
///
/// The queue is bounded at [`MAX_PENDING_ENCODES`]; a full queue means the
/// caller encodes inline, which is the same back-pressure as before.
fn encode_queue() -> &'static std::sync::mpsc::SyncSender<EncodeJob> {
    static QUEUE: std::sync::OnceLock<std::sync::mpsc::SyncSender<EncodeJob>> =
        std::sync::OnceLock::new();
    QUEUE.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel::<EncodeJob>(MAX_PENDING_ENCODES);
        std::thread::Builder::new()
            .name("chukcut-preview-encode".into())
            .spawn(move || {
                // Ends when the sender is dropped, which only happens at
                // process exit: the queue outlives any one session, exactly as
                // rayon's pool did.
                loop {
                    let idle = std::time::Instant::now();
                    let Ok(job) = receiver.recv() else { return };
                    probe::add(&PROBE.encode_idle_ns, idle);
                    job();
                    PENDING_ENCODES.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                }
            })
            .expect("spawn the preview encode thread");
        sender
    })
}

/// Release the in-flight scrub mark, however the frame ended.
///
/// Every exit from [`render_one`] has to reach this for a scrub, or a frame
/// that failed to render would be refused forever after: `request_frame` would
/// keep seeing it as still in flight and never queue it again.
fn done_rendering(shared: &Shared, frame: i64) {
    let mut work = shared.work.lock();
    if work.rendering == Some(frame) {
        work.rendering = None;
    }
}

/// One line when the quality ladder moves.
///
/// Worth INFO and not DEBUG for the same reason the encoder's backend change
/// is: the picture got softer or sharper on its own, and without this the only
/// evidence is a number in a summary line that a reader has to notice changed.
/// Rare by construction — a step needs three bad frames or three good seconds.
fn log_rung(rung: usize, why: &'static str) {
    tracing::info!(
        rung,
        ladder = ladder::rung_label(rung),
        reason = why,
        "preview quality ladder moved"
    );
}

/// Whether a frame that has just been composited is already past showing.
///
/// The tolerance is not zero, and that is deliberate. A frame one interval
/// behind is still shown: `serve_uri` answers a request for the current frame
/// with a neighbour within [`NEAREST_TOLERANCE`], so it has a reader. Two or
/// more and the pacer has moved past anything it could substitute for.
///
/// `discarded_last` is the guarantee of progress: see [`render_one`].
fn too_late_to_encode(frame: i64, position: Micros, fps: f64, discarded_last: bool) -> bool {
    if discarded_last {
        return false;
    }
    frame_at(position, fps) - frame > LATE_ENCODE_TOLERANCE
}

/// Composite one frame, and encode it unless nobody can still see it.
///
/// Returns whether the frame was thrown away for being late, which the caller
/// hands back on the next call. That is the whole of the guarantee that this
/// cannot stop the picture: **two composited frames are never discarded in a
/// row**. A renderer that is consistently more than a frame behind would
/// otherwise discard every frame it ever finished — each one is late by the
/// same margin as the last — and the viewer would sit in front of a frozen
/// picture while the machine stayed busy. Measured, in
/// `examples/preview_waste.rs`: at eight times the frame budget the unguarded
/// version encoded 1 frame in 14.
fn render_one(
    shared: &Arc<Shared>,
    ctx: &Arc<RenderContext>,
    compositor: &Compositor,
    job: Job,
    discarded_last: bool,
) -> bool {
    let session = &job.session;
    let time = frame_time(job.frame, session.fps);

    // What this frame is made of, and the one asymmetry that matters.
    //
    // A **scrub** — a seek, a pause, the first frame of a session — is the
    // frame somebody sits and looks at, so it is always the session's own size
    // and `scrub_quality`. The ladder cannot touch it.
    //
    // A **playback** frame may be smaller and softer than that, because the
    // alternative when the machine cannot hold the size is not a sharper
    // picture, it is a picture that stops moving. See [`ladder`].
    let (target, quality, rung) = if job.scrub {
        (session.size, session.scrub_quality, 0)
    } else {
        let ladder = *shared.ladder.lock();
        (
            ladder.size(session.size),
            ladder.quality(session.quality),
            ladder.rung(),
        )
    };
    let size = ctx.clamp_size(target);
    let sources = Arc::clone(&*shared.sources.read());

    let composite_started = std::time::Instant::now();
    let rgba = match compositor.render_frame(&session.project, time, size, sources.as_ref()) {
        Ok(rgba) => rgba,
        Err(error) => {
            let message = format!("cannot render the preview frame: {error}");
            if job.scrub {
                done_rendering(shared, job.frame);
                shared.emit_error_once(message);
            } else {
                tracing::warn!(%error, frame = job.frame, "preview render failed");
            }
            return false;
        }
    };

    let composite_micros = composite_started.elapsed().as_micros();
    probe::add(&PROBE.composite_ns, composite_started);
    probe::bump(&PROBE.composites);

    // The frame is finished and already too late to show. Do not spend a JPEG
    // on it.
    //
    // `pace` drops frames it has not started; this is the other end of the same
    // rule, for a frame the clock passed *while* it was being composited. The
    // two never count the same frame — this one was taken off the queue before
    // `pace` looked at it — so it has its own counter, `discarded`.
    //
    if !job.scrub && shared.clock.is_playing() && experiment::skip_late_encodes() {
        let position = shared.clock.position();
        if too_late_to_encode(job.frame, position, session.fps, discarded_last) {
            shared.stats.record_discarded(1);
            tracing::debug!(
                frame = job.frame,
                behind = frame_at(position, session.fps) - job.frame,
                composite_ms = composite_micros as f64 / 1000.0,
                "the playhead passed this frame while it was being composited; \
                 not encoding it"
            );
            if let Some(rung) = shared.ladder.lock().dropped(1) {
                log_rung(rung, "a composited frame was already too late to show");
            }
            return true;
        }
    }

    let dispatch_started = std::time::Instant::now();

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
                    if scrub {
                        done_rendering(&shared, frame_no);
                    }
                    shared.emit_error_once(error.to_string());
                    return;
                }
            };
            let encode_micros = encode_started.elapsed().as_micros();
            probe::add(&PROBE.encode_ns, encode_started);
            probe::bump(&PROBE.encodes);
            PROBE
                .encoded_bytes
                .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);

            // The two halves are reported separately because they now run in
            // parallel: what fits in the budget is the larger of them, not the
            // sum. `over_budget` is judged on that.
            let slowest = composite_micros.max(encode_micros) as i64;
            let slowest_ms = slowest as f64 / 1000.0;

            // What playback gave up for this frame, so the summary line can say
            // it. `None` for a scrub: a scrub never goes through the ladder.
            let rendered = (!scrub).then_some(Rendered {
                rung: rung as u8,
                width: size.0,
                height: size.1,
                quality,
            });

            // Three integer adds and a bucket increment. Everything that costs
            // anything — the divisions, the percentile, the formatting —
            // happens once a second in the pacer, not here.
            if let Some(fallback) = shared.stats.record_frame(scrub, slowest, backend, rendered) {
                // The encoder changed backend under us. `encode_preview_jpeg`
                // falls back silently by design, and on a 1080x1920 frame that
                // is 31 ms of CPU against 6.2 ms of GPU — enough on its own to
                // turn smooth playback into a stutter, and otherwise invisible.
                tracing::info!(
                    from = fallback.from.label(),
                    to = fallback.to.label(),
                    width = size.0,
                    height = size.1,
                    quality,
                    reason = if super::vaapi::size_is_encodable(size.0, size.1) {
                        "the hardware encoder refused the frame or has been written off"
                    } else {
                        "NV12 cannot represent this frame size"
                    },
                    "preview JPEG encoder changed backend"
                );
            }

            let budget_ms = frame_interval(fps) as f64 / 1000.0;
            let over_budget = slowest_ms > budget_ms;

            tracing::debug!(
                session = session_id,
                frame = frame_no,
                at_ms = time / 1000,
                scrub,
                width = size.0,
                height = size.1,
                quality,
                rung,
                composite_ms = composite_micros as f64 / 1000.0,
                encode_ms = encode_micros as f64 / 1000.0,
                slowest_ms,
                budget_ms,
                over_budget,
                bytes = bytes.len(),
                encoder = backend.label(),
                "preview frame ready"
            );

            // The ladder is stepped from here rather than from the render
            // thread because this is where a frame's real cost is known — the
            // encode is half of it and it happens on this thread — and because
            // nothing is locked here, so the one INFO line a step is worth can
            // be written without a `write` syscall under somebody's mutex.
            if !scrub {
                if let Some(moved) = shared.ladder.lock().frame(over_budget) {
                    log_rung(
                        moved,
                        if over_budget {
                            "playback has been over budget for several frames"
                        } else {
                            "playback has had room to spare for three seconds"
                        },
                    );
                }
            }

            let inserted = std::time::Instant::now();
            let stored = shared.cache.insert(CachedFrame {
                session: session_id,
                frame: frame_no,
                time,
                bytes: Arc::from(bytes.into_boxed_slice()),
            });
            probe::add(&PROBE.insert_ns, inserted);

            // The picture has caught up with wherever the playhead was moved
            // to. This — and not how long `seek` took to return — is the wait
            // the user experienced; the command itself returns in microseconds
            // because the decoder seek happens here, on another thread.
            if let Some(slow) =
                shared
                    .seeks
                    .complete(session_id, frame_no, std::time::Instant::now())
            {
                stats::emit_slow_seek(&slow);
            }

            // A scrub frame is the one somebody is waiting to look at, so say so
            // the moment it exists. Playback frames are announced by the pacer
            // when they come due, not when they are made.
            //
            // Subordinate, not authoritative: this runs on the encode thread
            // *after* the encode, so the pacer may already have announced a
            // later frame — `Shared::emit_position` drops it if so, and that
            // decision is taken under the same lock as the send. The whole
            // account, and why the obvious version of the check is not enough,
            // is on that function.
            if scrub {
                // Cleared only now, not when the composite finished: the point
                // of the mark is that a request arriving *while* this frame is
                // being made does not start a second one, and the ring is the
                // first moment a request can be answered without one.
                done_rendering(&shared, frame_no);
            }
            if stored && scrub {
                shared.emit_position(
                    session_id,
                    frame_no,
                    time,
                    shared.clock.is_playing(),
                    false,
                );
            }
        }
    };

    if inline {
        finish();
        probe::add(&PROBE.inline_encode_ns, dispatch_started);
        probe::bump(&PROBE.inline_encodes);
        return false;
    }

    PENDING_ENCODES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if let Err(rejected) = encode_queue().try_send(Box::new(finish)) {
        PENDING_ENCODES.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        // A full queue is the back-pressure `inline` above provides, arrived at
        // one frame later; a disconnected one means the encode thread is gone,
        // and dropping the frame silently would be worse than a stall.
        let job = match rejected {
            std::sync::mpsc::TrySendError::Full(job) => job,
            std::sync::mpsc::TrySendError::Disconnected(job) => job,
        };
        job();
        probe::add(&PROBE.inline_encode_ns, dispatch_started);
        probe::bump(&PROBE.inline_encodes);
        return false;
    }
    probe::add(&PROBE.dispatch_ns, dispatch_started);
    false
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
            // The pacer is the authority on where playback is, which also makes
            // it the authority on how many frames the user actually saw.
            shared.stats.record_shown();
            shared.emit_position(session.id, frame, position, true, true);
        }

        // The summary is driven from here rather than from the render thread on
        // purpose. This loop runs every few milliseconds for as long as playback
        // is running, whatever the renderer is doing — so a renderer that has
        // stalled completely still produces a line, and that line says
        // `shown=30 rendered=0`, which is the clearest possible statement of
        // what went wrong. Driven from the render thread, a stall would produce
        // no line at all: silence exactly when there is something to say.
        if let Some(summary) = shared.stats.tick(std::time::Instant::now()) {
            stats::emit(&summary, stats::Reason::Tick);
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
            if let Some(summary) = shared.stats.finish(std::time::Instant::now()) {
                stats::emit(&summary, stats::Reason::Stopped);
            }
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

/// Threads that answer frame-URL requests.
///
/// Small on purpose: these threads spend their time parked on a condition
/// variable, not working, so the count is "how many frame requests may be
/// waiting at once" and not "how many cores are there". Four covers a webview
/// that has several images in flight during a stall; a fifth request runs on
/// whichever thread frees up first, a few milliseconds later.
const FRAME_REQUEST_THREADS: usize = 4;

fn frame_request_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(FRAME_REQUEST_THREADS)
            .thread_name(|i| format!("chukcut-frame-request-{i}"))
            .build()
            .expect("build the frame request pool")
    })
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

    // A pooled task rather than a fresh OS thread.
    //
    // Every displayed frame is one request, so at 30 fps this path ran thirty
    // thread spawns a second, each of which then went on to wait on a condition
    // variable. Thread creation is cheap but not free, and the jitter it adds
    // lands directly on the frame the user is waiting to see.
    //
    // **Its own pool, not rayon's global one**, and that is the whole reason
    // this function is not two lines shorter. `serve_uri` parks on a condition
    // variable for up to `FRAME_WAIT` when the frame is not ready yet, and
    // `vaapi::rgba_to_nv12` — the first stage of every preview encode — is a
    // job *in* the global pool. Answering requests there means that exactly
    // when the renderer falls behind, every miss takes a worker out of the pool
    // the encode needs, which slows the encode, which produces more misses.
    // That is the same feedback loop `request_frame` documents, arrived at
    // through the scheduler instead of the decoder. A separate registry cannot
    // starve the one doing the work.
    let started = std::time::Instant::now();
    frame_request_pool().spawn(move || {
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
    // Rendering only what the screen can show
    // -----------------------------------------------------------------------

    /// A server with a session installed and no threads, so the decisions
    /// around a session can be checked on a machine with no GPU.
    fn parked() -> (Arc<PreviewServer>, PreviewInfo) {
        let server = PreviewServer::with_capacity(8);
        let info = server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            0,
            false,
            SeekKind::SessionStart,
        );
        (server, info)
    }

    #[test]
    fn the_panel_decides_the_render_size_and_supersedes_when_it_changes() {
        let (server, info) = parked();
        assert_eq!(
            (info.width, info.height),
            (1920, 1080),
            "no panel measured yet, so the canvas stands"
        );

        let resized = server
            .set_viewport(Some(Viewport::new(700, 394)))
            .expect("a session is open");
        assert_eq!(
            (resized.width, resized.height),
            (700, 394),
            "the panel, not the canvas"
        );
        assert!(resized.session > info.session, "a new size is a new session");
        assert_eq!(
            server.cache().session(),
            resized.session,
            "and the ring holding frames at the old size was emptied"
        );

        // A window drag is hundreds of layouts and only a few of them change
        // the rounded size. The rest must cost nothing at all.
        let again = server
            .set_viewport(Some(Viewport::new(701, 395)))
            .expect("a session is open");
        assert_eq!(
            again.session, resized.session,
            "a resize that rounds to the same size did not restart the pipeline"
        );

        // Going back to no panel restores the canvas.
        let full = server.set_viewport(None).expect("a session is open");
        assert_eq!((full.width, full.height), (1920, 1080));
        assert!(full.session > resized.session);
    }

    #[test]
    fn the_playhead_survives_a_resize() {
        let (server, _) = parked();
        server.seek(1_000_000).expect("a session is open");
        let before = server.clock().position();
        let resized = server
            .set_viewport(Some(Viewport::new(640, 360)))
            .expect("a session is open");
        assert_eq!(server.clock().position(), before, "a resize is not a seek");
        assert_eq!(resized.frame, 30);
        assert_eq!(server.shared.work.lock().scrub, Some(30), "and it re-renders");
    }

    #[test]
    fn pausing_after_a_degraded_run_re_renders_the_frame_at_full_quality() {
        let (server, info) = parked();
        server.play().expect("a session is open");

        // What a machine that cannot hold the size does to the ladder. Every
        // frame in the ring is now smaller and softer than the session's own
        // size, and one of them is the frame the user is about to sit and look
        // at.
        assert!(server.shared.ladder.lock().dropped(1).is_some());

        let paused = server.pause().expect("a session is open");
        assert!(
            paused.session > info.session,
            "the degraded ring was not thrown away"
        );
        assert_eq!(
            (paused.width, paused.height),
            (info.width, info.height),
            "the paused frame is the session's own size"
        );
        assert_eq!(server.shared.ladder.lock().rung(), 0, "and back at the top");
        assert_eq!(
            server.shared.work.lock().scrub,
            Some(paused.frame),
            "the frame under the playhead was queued for a fresh render"
        );

        // A run that never had to give anything up keeps its ring: superseding
        // for it would throw away the read-ahead a resume needs.
        server.play().expect("a session is open");
        let again = server.pause().expect("a session is open");
        assert_eq!(again.session, paused.session);
    }

    #[test]
    fn a_paused_frame_is_never_encoded_worse_than_a_playing_one() {
        // The owner's requirement, as an assertion: whatever the settings say
        // about playback quality, the frame he stops on is at least as good.
        for quality in [40u8, 88, 94, 100] {
            let session = PreviewSession::new(
                project(),
                PreviewOptions {
                    quality: Some(quality),
                    ..PreviewOptions::default()
                },
            );
            assert!(
                session.scrub_quality >= session.quality,
                "quality {quality}: scrub {} against playback {}",
                session.scrub_quality,
                session.quality
            );
        }
    }

    // -----------------------------------------------------------------------
    // Work that reaches nobody
    // -----------------------------------------------------------------------

    #[test]
    fn a_frame_the_playhead_has_passed_is_not_worth_a_jpeg() {
        let fps = 30.0;
        let now = frame_time(40, fps);
        // On time, and one frame late, are both still worth encoding: the
        // handler serves a neighbour within `NEAREST_TOLERANCE`, so somebody
        // sees it.
        assert!(!too_late_to_encode(40, now, fps, false));
        assert!(!too_late_to_encode(41, now, fps, false));
        assert!(!too_late_to_encode(39, now, fps, false));
        // Two or more behind and nothing will ask for it.
        assert!(too_late_to_encode(38, now, fps, false));
        assert!(too_late_to_encode(10, now, fps, false));
    }

    #[test]
    fn two_frames_in_a_row_are_never_discarded() {
        // Otherwise a renderer that is consistently three frames behind throws
        // away every frame it finishes and the picture stops entirely — which
        // is worse than the softness the ladder would have traded for it.
        let fps = 30.0;
        let now = frame_time(40, fps);
        assert!(too_late_to_encode(30, now, fps, false));
        assert!(!too_late_to_encode(30, now, fps, true));
    }

    #[test]
    fn a_request_for_a_frame_already_being_rendered_does_not_queue_a_second_render() {
        let (server, info) = parked();

        // What the render thread does when it picks the job up.
        {
            let mut work = server.shared.work.lock();
            assert_eq!(work.scrub.take(), Some(0), "the session queued its first frame");
            work.rendering = Some(0);
        }

        server.request_frame(info.session, 0);
        assert_eq!(
            server.shared.work.lock().scrub,
            None,
            "the webview asking again while it waits must not start a second render"
        );

        server.request_frame(info.session, 1);
        assert_eq!(
            server.shared.work.lock().scrub,
            Some(1),
            "a different frame is still a real request"
        );

        // Once the bytes are in the ring the mark is released, so a later
        // request for the same frame is honoured again.
        done_rendering(&server.shared, 0);
        server.request_frame(info.session, 0);
        assert_eq!(server.shared.work.lock().scrub, Some(0));
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
            SeekKind::SessionStart,
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

        // Rewind the injected clock.
        //
        // The source is shared through the `OnceLock` along with the server,
        // so a test that moves the playhead leaves it moved for every test
        // after it in this process. That made these tests order-dependent:
        // one of them asserts the ring settles at exactly the read-ahead
        // limit, which is only true when the playhead is where the test
        // thinks it is, and inheriting a position from a previous test put
        // the window further out and produced one frame too many. It failed
        // at two different assertions on two runs, which is the signature of
        // shared state rather than of a race.
        //
        // A test that asserts the injected source is in control cannot start
        // by inheriting somebody else's position.
        time.set(0);

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
            // The message carries the whole decision state, because "frame N
            // never arrived" on its own says nothing about *why*. The renderer
            // stops for exactly two reasons — it reached its read-ahead limit,
            // or playback is no longer running — and telling those apart from
            // the outside is otherwise guesswork. `pace` idles when
            // `cursor >= frame_at(position) + read_ahead`, so with the position
            // and the frame count printed the arithmetic is checkable by hand.
            let status = server.status();
            assert!(
                found.is_hit(),
                "frame {frame} never arrived (ring holds {:?}; \
                 playing={} position={}µs playhead=frame {} session fps={} \
                 cached={}/{})",
                server.cache().frames(),
                status.playing,
                status.position,
                status.frame,
                status.fps,
                status.cached,
                status.capacity,
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
            SeekKind::SessionStart,
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

    /// The wiring, not the arithmetic: the arithmetic is covered in
    /// `stats::tests`, but nothing there proves that the render thread, the
    /// encode thread and the pacer are actually counting into it. Without this
    /// the whole module could be correct and never called.
    #[test]
    fn playback_leaves_numbers_a_log_line_can_be_built_from() {
        if crate::modules::render::test_context().is_none() {
            eprintln!("skipping: no GPU adapter");
            return;
        }
        let (server, _time, _exclusive) = shared_server();
        server.set_source_provider(Arc::new(EmptySourceProvider));
        let info = server.adopt(
            Arc::new(PreviewSession::new(project(), PreviewOptions::default())),
            0,
            false,
            SeekKind::SessionStart,
        );
        server.play().expect("session is open");

        // Synchronises on the ring, so every frame below has been through
        // `record_frame` by the time this returns.
        let lead = DEFAULT_READ_AHEAD as i64;
        await_frames(&server, info.session, 0..lead);

        let summary = server
            .shared
            .stats
            .finish(std::time::Instant::now())
            .expect("playback rendered frames, so there is something to say");

        assert!(
            summary.rendered >= lead as u64,
            "the render thread counted {} frames against {lead} in the ring",
            summary.rendered
        );
        assert_eq!(
            summary.dropped, 0,
            "the clock never moved, so nothing can have been late"
        );
        assert_eq!((summary.width, summary.height), (info.width, info.height));
        assert!(
            summary.decode.is_some(),
            "the render thread reports the decode path before its first frame"
        );
        assert!(summary.encode.is_some(), "and the encode thread reports its backend");
        assert!(
            summary.mean_ms > 0.0 && summary.mean_ms.is_finite(),
            "mean {}",
            summary.mean_ms
        );
        assert!(
            summary.p99_ms >= summary.mean_ms,
            "p99 {} is below the mean {}",
            summary.p99_ms,
            summary.mean_ms
        );
        // One bucket of slack: the maximum is exact and the percentile is the
        // upper edge of the bucket its sample fell in, so p99 may legitimately
        // sit up to `BUCKET_MICROS` above the largest sample.
        let bucket_ms = super::stats::BUCKET_MICROS as f64 / 1000.0;
        assert!(
            summary.max_ms + bucket_ms >= summary.p99_ms,
            "max {} is more than one bucket below p99 {}",
            summary.max_ms,
            summary.p99_ms
        );
        assert!(
            (summary.budget_ms - 33.333).abs() < 0.01,
            "a 30 fps project has a 33.3 ms budget, not {}",
            summary.budget_ms
        );

        // Draining is not idempotent by accident: `finish` resets, so a stop
        // straight after a final summary must not print a second empty one.
        assert!(
            server.shared.stats.finish(std::time::Instant::now()).is_none(),
            "the window was drained"
        );

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
            SeekKind::SessionStart,
        );
        let second = server.seek(1_000_000).expect("session is open");
        assert!(second.session > first.session);

        let stale = server.serve_uri(&format!("{}/{}", first.frame_url, first.frame));
        assert_eq!(stale.status(), tauri::http::StatusCode::GONE);

        server.stop();
    }
}
