//! Driving `PreviewServer` end to end: real media, real GPU, real JPEG bytes.
//!
//! The clock reads from a source this file owns. That is the whole design of
//! these tests: playback driven by wall time can only be checked by sleeping
//! and hoping, and a suite that sleeps produces failures that mean "the machine
//! was busy". With an injected source, where the renderer has got to is an
//! exact function of where the playhead was put, and every wait below blocks on
//! a condition — the ring's condvar or the event channel's — rather than on a
//! timer.
//!
//! What is checked here that the unit tests cannot check:
//!
//! - the bytes served over `chukcut-frame://` are a JPEG of the right size that
//!   decodes to the frame the playhead is on, which covers clock, compositor,
//!   decoder, encoder and ring in one assertion;
//! - position events arrive in order and describe frames that exist;
//! - a superseded session is refused rather than answered with a stale picture.

mod support;

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use chukcut_lib::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_lib::modules::preview::clock::{frame_time, ManualSource};
use chukcut_lib::modules::preview::{
    encoder::is_jpeg, PreviewEvent, PreviewOptions, PreviewServer, DEFAULT_CAPACITY,
    DEFAULT_READ_AHEAD,
};
use chukcut_lib::modules::project::document::{CanvasConfig, Micros, Project, Track, TrackKind};
use chukcut_lib::modules::render::SourceProvider;

use support::{material_for, read_counter_rgba, segment};

/// A hung renderer must fail the suite, not stall it. Nothing waits this long
/// in practice — every wait returns the moment its condition holds.
const DEADLINE: Duration = Duration::from_secs(30);

/// A 320x240 timeline showing four seconds of the counter clip.
///
/// The canvas is small enough that the preview renders it at native resolution
/// (the proxy table only starts capping above a 720-pixel long edge), so the
/// counter stripes survive into the served JPEG and the frame the viewer would
/// be looking at can be read back exactly.
fn counter_project() -> Option<Arc<Project>> {
    let media = support::media().ok()?;
    let mut project = Project::new(
        "preview integration",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment("counter", 0, 4_000_000));
    project.tracks.push(track);
    Some(Arc::new(project))
}

// ---------------------------------------------------------------------------
// Listening to the event channel
// ---------------------------------------------------------------------------

/// One position update, flattened out of the channel's JSON.
#[derive(Debug, Clone, PartialEq)]
struct Position {
    session: u64,
    frame: i64,
    time: Micros,
    playing: bool,
}

/// Everything the server has said, and a way to block until it says more.
#[derive(Default)]
struct Events {
    seen: Mutex<Vec<serde_json::Value>>,
    arrived: Condvar,
}

impl Events {
    fn channel(self: &Arc<Self>) -> tauri::ipc::Channel<PreviewEvent> {
        let events = Arc::clone(self);
        tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                    events.seen.lock().expect("event lock").push(value);
                    events.arrived.notify_all();
                }
            }
            Ok(())
        })
    }

    fn positions(&self) -> Vec<Position> {
        self.seen
            .lock()
            .expect("event lock")
            .iter()
            .filter(|event| event["type"] == "position")
            .map(|event| Position {
                session: event["session"].as_u64().unwrap_or(0),
                frame: event["frame"].as_i64().unwrap_or(-1),
                time: event["time"].as_i64().unwrap_or(-1),
                playing: event["playing"].as_bool().unwrap_or(false),
            })
            .collect()
    }

    fn kinds(&self) -> Vec<String> {
        self.seen
            .lock()
            .expect("event lock")
            .iter()
            .filter_map(|event| event["type"].as_str().map(str::to_string))
            .collect()
    }

    /// Block until some event satisfies `wanted`, or fail saying what arrived.
    #[track_caller]
    fn await_event(&self, what: &str, wanted: impl Fn(&serde_json::Value) -> bool) {
        let deadline = std::time::Instant::now() + DEADLINE;
        let mut seen = self.seen.lock().expect("event lock");
        loop {
            if seen.iter().any(&wanted) {
                return;
            }
            let now = std::time::Instant::now();
            assert!(
                now < deadline,
                "waited {DEADLINE:?} for {what}; the server only said {:?}",
                seen.iter()
                    .map(|e| e["type"].as_str().unwrap_or("?"))
                    .collect::<Vec<_>>()
            );
            let (guard, _) = self
                .arrived
                .wait_timeout(seen, deadline - now)
                .expect("event lock");
            seen = guard;
        }
    }
}

// ---------------------------------------------------------------------------
// A running server
// ---------------------------------------------------------------------------

/// Only one preview server at a time in this process.
///
/// This used to be load-bearing for a second reason: `PreviewServer` opened its
/// own `RenderContext` on its render thread, so N servers meant N GPU devices —
/// which segfaults inside the driver on this machine at eight of them. It no
/// longer opens anything; `chukcut_lib::modules::gpu` owns the one device and
/// every server shares it. The serialisation stays because the app only ever
/// has one server, so testing them one at a time is testing what actually runs.
/// The lock is deliberately poison-tolerant: one failing test must not turn the
/// rest into a cascade of confusing secondary failures.
static ONE_SERVER: Mutex<()> = Mutex::new(());

struct Running {
    server: Arc<PreviewServer>,
    time: Arc<ManualSource>,
    events: Arc<Events>,
    session: u64,
    frame_url: String,
    size: (u32, u32),
    _exclusive: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.server.stop();
    }
}

/// Start a session on `project` with the playhead at `at`.
fn start(project: Arc<Project>, at: Micros) -> Option<Running> {
    support::gpu()?;
    let exclusive = ONE_SERVER.lock().unwrap_or_else(|e| e.into_inner());

    let time = Arc::new(ManualSource::new());
    let server = PreviewServer::with_time_source(DEFAULT_CAPACITY, time.clone());
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    server.set_source_provider(sources);

    let events = Arc::new(Events::default());
    let info = server.start(project, PreviewOptions::default(), at, events.channel());

    Some(Running {
        server,
        time,
        events,
        session: info.session,
        frame_url: info.frame_url,
        size: (info.width, info.height),
        _exclusive: exclusive,
    })
}

macro_rules! running {
    ($project:expr, $at:expr) => {{
        let _ = require_media!();
        let _ = require_gpu!();
        match start($project, $at) {
            Some(running) => running,
            None => return,
        }
    }};
}

impl Running {
    fn url(&self, frame: i64) -> String {
        format!("{}/{frame}", self.frame_url)
    }

    /// Block until `frame` is in the ring.
    ///
    /// Encoding runs off the render thread, so frames finish out of order and
    /// waiting on one says nothing about its neighbours — see [`Self::await_frames`].
    #[track_caller]
    fn await_frame(&self, frame: i64) {
        let found = self.server.cache().wait(self.session, frame, DEADLINE);
        assert!(
            found.is_hit(),
            "frame {frame} never arrived; the ring holds {:?}",
            self.server.cache().frames()
        );
    }

    /// Block until every frame in `frames` is in the ring.
    #[track_caller]
    fn await_frames(&self, frames: std::ops::Range<i64>) {
        for frame in frames {
            self.await_frame(frame);
        }
    }

    /// Fetch a frame over the protocol handler and check it is a real image.
    #[track_caller]
    fn fetch(&self, frame: i64) -> Vec<u8> {
        self.await_frame(frame);
        let response = self.server.serve_uri(&self.url(frame));
        assert_eq!(
            response.status(),
            tauri::http::StatusCode::OK,
            "frame {frame} was not served"
        );
        assert_eq!(
            response
                .headers()
                .get(tauri::http::header::CONTENT_TYPE)
                .map(|v| v.to_str().unwrap_or("")),
            Some("image/jpeg")
        );
        let bytes = response.body().clone();
        assert!(is_jpeg(&bytes), "the body is not a JPEG");
        bytes
    }
}

/// Decode a served JPEG and read the counter it carries.
///
/// FFmpeg demuxes a bare JPEG as a one-frame video, so the same decoder that
/// reads the project's media reads the preview's output — which means this
/// checks the whole path from playhead to pixels rather than checking that some
/// bytes came back.
fn counter_of(bytes: &[u8], label: &str) -> (u64, u32, u32) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("preview");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    let path = dir.join(format!("{label}.jpg"));
    std::fs::write(&path, bytes).expect("write the served frame");

    let mut decoder = VideoDecoder::open(&path).expect("a served frame is a decodable image");
    let frame = decoder.seek_and_decode(0).expect("decode the served frame");
    let counter = read_counter_rgba(&frame.data, frame.width, frame.height)
        .unwrap_or_else(|| panic!("the served frame for {label} carries no readable counter"));
    let _ = std::fs::remove_file(&path);
    (counter, frame.width, frame.height)
}

// ---------------------------------------------------------------------------
// Scrubbing
// ---------------------------------------------------------------------------

#[test]
fn a_new_session_serves_the_frame_under_the_playhead_and_it_is_the_right_one() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    // One second in, which at 30 fps is frame 30 of the timeline and — since
    // the clip starts at zero and runs at 1x — frame 30 of the source.
    let running = running!(project, 1_000_000);

    assert_eq!(
        running.size,
        (320, 240),
        "a canvas below the proxy threshold previews at native resolution"
    );

    let bytes = running.fetch(30);
    let (counter, width, height) = counter_of(&bytes, "scrub");
    assert_eq!((width, height), (320, 240), "the JPEG is the proxy size");
    assert_eq!(counter, 30, "the preview is showing the wrong frame");

    // And the frontend was told which frame to display, without polling.
    running
        .events
        .await_event("a position update", |e| e["type"] == "position");
    let latest = running.events.positions();
    assert!(
        latest.iter().any(|p| p.session == running.session && p.frame == 30),
        "no position update named frame 30: {latest:?}"
    );
}

#[test]
fn moving_the_playhead_serves_the_frame_it_moved_to() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 0);
    assert_eq!(counter_of(&running.fetch(0), "seek_0").0, 0);

    // Each seek supersedes the session, so the url changes with it — which is
    // exactly the mechanism being relied on here.
    let mut session = running.session;
    for (at, want) in [(2_000_000i64, 60u64), (500_000, 15), (3_500_000, 105)] {
        let info = running.server.seek(at).expect("session is open");
        assert!(info.session > session, "a seek must supersede the session");
        session = info.session;

        let found = running.server.cache().wait(session, info.frame, DEADLINE);
        assert!(found.is_hit(), "frame {} never arrived", info.frame);
        let response = running.server.serve_uri(&format!("{}/{}", info.frame_url, info.frame));
        assert_eq!(response.status(), tauri::http::StatusCode::OK);
        assert_eq!(
            counter_of(response.body(), &format!("seek_{at}")).0,
            want,
            "seeking to {at} µs showed the wrong frame"
        );
    }
}

#[test]
fn a_frame_from_a_superseded_session_is_refused_rather_than_shown() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 0);

    // Render something into the first session's ring, so there is a real frame
    // to be wrongly served.
    let stale_bytes = running.fetch(0);
    assert!(!stale_bytes.is_empty());
    let stale_url = running.url(0);
    let stale_session = running.session;

    let second = running.server.seek(2_000_000).expect("session is open");
    assert!(second.session > stale_session);

    let response = running.server.serve_uri(&stale_url);
    assert_eq!(
        response.status(),
        tauri::http::StatusCode::GONE,
        "a frame from before the seek must be refused"
    );
    assert_ne!(
        response.body(),
        &stale_bytes,
        "and certainly not answered with the picture of the old position"
    );

    // The refusal is about the session, not about the frame number: the same
    // number under the *live* session is served normally.
    running.server.cache().wait(second.session, second.frame, DEADLINE);
    assert_eq!(
        running
            .server
            .serve_uri(&format!("{}/{}", second.frame_url, second.frame))
            .status(),
        tauri::http::StatusCode::OK
    );

    // A render that was in flight when the seek happened is dropped on the
    // floor rather than landing in the new session's ring.
    assert!(!running.server.cache().insert(
        chukcut_lib::modules::preview::CachedFrame {
            session: stale_session,
            frame: 0,
            time: 0,
            bytes: Arc::from(vec![0xFF, 0xD8].into_boxed_slice()),
        }
    ));
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

#[test]
fn playback_renders_ahead_of_the_playhead_and_stops_at_the_read_ahead_limit() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 0);
    running.server.play().expect("session is open");

    // The clock is at frame 0 and will not move until this test moves it, so
    // the renderer must produce exactly the read-ahead window and then idle.
    let lead = DEFAULT_READ_AHEAD as i64;
    running.await_frames(0..lead);
    assert_eq!(
        running.server.cache().frames(),
        (0..lead).collect::<Vec<i64>>(),
        "the ring is not exactly the read-ahead window"
    );
    assert_eq!(running.server.clock().position(), 0);

    // Every frame in the ring is a real, correct picture — not merely present.
    for frame in [0i64, 5, lead - 1] {
        assert_eq!(
            counter_of(&running.fetch(frame), &format!("ahead_{frame}")).0,
            frame as u64,
            "the ring holds the wrong picture for frame {frame}"
        );
    }
}

#[test]
fn position_updates_arrive_in_order_as_the_playhead_advances() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 0);
    running.server.play().expect("session is open");
    let session = running.session;

    // Step the clock one frame at a time, waiting for the frontend to be told
    // about each. Waiting for each step is what makes the sequence exact: the
    // pacer reports the frame under the playhead, so a jump would legitimately
    // skip numbers and there would be nothing to assert.
    for frame in 1..=8i64 {
        running.time.set(frame_time(frame, 30.0));
        running.events.await_event(&format!("frame {frame}"), |event| {
            event["type"] == "position"
                && event["frame"].as_i64() == Some(frame)
                && event["playing"].as_bool() == Some(true)
        });
    }

    let positions: Vec<Position> = running
        .events
        .positions()
        .into_iter()
        .filter(|p| p.session == session)
        .collect();

    assert!(
        positions.windows(2).all(|w| w[1].frame >= w[0].frame),
        "position updates went backwards: {positions:?}"
    );
    let playing: Vec<i64> = positions
        .iter()
        .filter(|p| p.playing)
        .map(|p| p.frame)
        .collect();
    for frame in 1..=8i64 {
        assert!(
            playing.contains(&frame),
            "frame {frame} was never announced: {playing:?}"
        );
    }
    // Each update carries the instant as well as the number, and the two have
    // to agree or the ruler and the picture disagree.
    for position in &positions {
        assert_eq!(
            position.time,
            frame_time(position.frame, 30.0),
            "position update {position:?} has a time that is not its frame"
        );
    }
}

#[test]
fn the_ring_extends_when_the_playhead_moves_within_the_read_ahead_window() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 0);
    running.server.play().expect("session is open");

    let lead = DEFAULT_READ_AHEAD as i64;
    running.await_frames(0..lead);

    // Six frames on, which is inside the window the renderer has already
    // covered. It should extend the ring by six, not restart it: the frames
    // the playhead has not reached yet are still wanted.
    running.time.set(frame_time(6, 30.0));
    running.await_frames(lead..6 + lead);
    assert_eq!(
        running.server.cache().frames(),
        (0..6 + lead).collect::<Vec<i64>>(),
        "the ring did not simply extend"
    );

    // Pausing freezes it: a paused clock does not advance, so nothing new is
    // due and nothing new is rendered.
    running.server.pause().expect("session is open");
    let frozen = running.server.cache().frames();
    running.time.advance(10_000_000);
    assert_eq!(running.server.cache().frames(), frozen);
    assert!(!running.server.clock().is_playing());
}

#[test]
fn a_renderer_left_behind_by_the_playhead_abandons_the_frames_it_missed() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 0);
    running.server.play().expect("session is open");

    let lead = DEFAULT_READ_AHEAD as i64;
    running.await_frames(0..lead);

    // Jump the playhead past everything the renderer has produced, which is
    // what a stalled GPU looks like from the clock's side. The rule is that
    // time is never stretched to let the renderer catch up: frames 12..20 are
    // already late, so they are dropped unrendered and work resumes at 20.
    // Rendering them anyway would put the picture permanently behind the sound.
    running.time.set(frame_time(20, 30.0));
    running.await_frames(20..20 + lead);

    let frames = running.server.cache().frames();
    let mut want: Vec<i64> = (0..lead).collect();
    want.extend(20..20 + lead);
    assert_eq!(
        frames, want,
        "the renderer did not abandon the frames it was late for"
    );

    // What it did render after the jump is still the right picture.
    assert_eq!(counter_of(&running.fetch(20), "late_20").0, 20);
}

#[test]
fn playback_stops_at_the_end_of_the_project_and_says_so() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    // The clip is four seconds; start just before the end so the run is short.
    let running = running!(project, 3_900_000);
    running.server.play().expect("session is open");

    running.time.advance(200_000);
    running
        .events
        .await_event("the end of the project", |event| event["type"] == "ended");

    assert!(!running.server.clock().is_playing(), "playback did not stop");
    assert_eq!(
        running.server.clock().position(),
        4_000_000,
        "the playhead should rest exactly on the end"
    );
    assert!(
        running.events.kinds().iter().filter(|k| *k == "ended").count() >= 1,
        "the frontend was never told"
    );

    // Playing again from the end replays rather than producing one frame and
    // stopping, which is what every player does.
    running.server.play().expect("session is open");
    assert_eq!(running.server.clock().position(), 0);
    assert!(running.server.clock().is_playing());
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[test]
fn the_status_snapshot_describes_the_session_the_frontend_is_looking_at() {
    let Some(project) = counter_project() else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let running = running!(project, 1_000_000);
    running.fetch(30);

    let status = running.server.status();
    assert_eq!(status.session, Some(running.session));
    assert_eq!((status.width, status.height), (320, 240));
    assert_eq!(status.fps, 30.0);
    assert_eq!(status.duration, 4_000_000);
    assert_eq!(status.position, 1_000_000);
    assert_eq!(status.frame, 30);
    assert!(!status.playing);
    assert!(status.cached >= 1);
    assert_eq!(status.capacity, DEFAULT_CAPACITY);
    assert_eq!(
        status.frame_url.as_deref(),
        Some(running.frame_url.as_str()),
        "the url the status reports has to be the one the frames answer on"
    );

    // Stopping closes the session and throws the ring away: its frames describe
    // a document that is no longer on screen.
    running.server.stop();
    let after = running.server.status();
    assert_eq!(after.session, None);
    assert_eq!(after.cached, 0);
    assert!(after.frame_url.is_none());
}
