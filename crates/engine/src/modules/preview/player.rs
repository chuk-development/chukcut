//! The native player's render thread: "the project at this time, this big"
//! in, BGRA frames ready for the UI out.
//!
//! The app shows these through GPUI, which draws with a wgpu device of its own.
//! Two ways across, chosen per player ([`FramePlayer::use_shared_frames`]):
//!
//! - **Shared** ([`FramePixels::Shared`]): the frame stays in GPU memory that
//!   is exported to GPUI's device (`render::shared_frame`); GPUI copies it into
//!   a texture GPU-side. No readback, no upload, nothing per pixel on a CPU.
//!   `docs/decisions/0027-preview-frames-shared-with-gpui.md`.
//! - **Readback** ([`FramePixels::Bgra`]): the frame is read back to system
//!   memory and GPUI uploads it again. The fallback wherever sharing is not
//!   possible — not Vulkan, no external memory, GPUI on another GPU.
//!
//! Everything else here serves both:
//!
//! - **The swizzle and the row packing happen on the GPU**, and the readback is
//!   asynchronous, through a ring of mapped buffers
//!   ([`BgraReadback`]). The thread never waits for the GPU while it has
//!   anything else to do.
//! - **Decode-ahead.** While frame N is composited and read back, a helper
//!   thread decodes frame N+1 of every visible clip, one thread per clip
//!   ([`MediaSourceProvider::prefetch`]). A frame costs the slowest of decode
//!   and readback instead of their sum.
//! - **Render-ahead during playback.** Frames are rendered for the instants the
//!   clock *will* reach and held in a short ring ([`READ_AHEAD`]); the UI takes
//!   the one that is due ([`FramePlayer::take`]). The audio clock stays the
//!   master: nothing here keeps time of its own.
//! - **Late frames are skipped, not queued.** A renderer that falls behind
//!   jumps forward to the frame that will be due when it finishes, using its
//!   own measured latency, so a heavy timeline drops frames evenly and stays in
//!   sync with the sound instead of drifting behind it.
//! - **Scrubbing is latest-wins.** While paused only the newest request is
//!   rendered, at once, without read-ahead; anything in flight for an older
//!   position is thrown away. A frame that finishes after a newer request
//!   arrived is still shown — showing nothing until the pointer stops would
//!   freeze the picture during a drag.
//!
//! The engine has no UI dependency, so a frame is plain bytes or a plain
//! description of exported memory; the app wraps either for GPUI.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use super::clock::{frame_at, frame_time};
use crate::modules::media::MediaSourceProvider;
use crate::modules::project::{Micros, Project, SAMPLE_SLACK};
use crate::modules::render::texture_pool::PooledTexture;
use crate::modules::render::{
    BgraReadback, Compositor, ReadbackStats, RenderContext, SharedFrame, SharedFrames,
    SourceProvider,
};

/// How many frames playback renders ahead of the clock. Four is 133 ms at
/// 30 fps: enough to ride out a slow frame (a GOP boundary, a title's first
/// raster), little enough that a 4K ring is 130 MB and an edit during playback
/// shows within a few frames.
pub const READ_AHEAD: usize = 4;

/// Readback slots. Three: one being collected, one on the GPU, one being
/// composited.
const READBACK_DEPTH: usize = 3;

/// The longest the thread sleeps with nothing to do, so a stop or a missed
/// wake-up is never more than this away.
const IDLE: Duration = Duration::from_millis(4);

/// What the UI wants on screen.
#[derive(Clone)]
pub struct PlayerRequest {
    pub project: Arc<Project>,
    /// The caller's edit counter. A new value invalidates every frame rendered
    /// for an older one, including those already in the ring.
    pub generation: u64,
    /// The timeline instant, with [`SAMPLE_SLACK`] already added — the same
    /// convention as the export.
    pub time: Micros,
    /// Output size in device pixels. Even numbers; the viewer's size, never
    /// the canvas's when the viewer is smaller.
    pub size: (u32, u32),
    /// Playback at normal speed, driven by the audio clock. Everything else —
    /// paused, scrubbing, shuttling — is `false` and renders exactly `time`.
    pub playing: bool,
}

/// A frame ready for the screen.
pub struct PlayerFrame {
    pub time: Micros,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub pixels: FramePixels,
}

/// Where a frame's pixels are.
pub enum FramePixels {
    /// Tightly packed BGRA8 in system memory — what GPUI's `RenderImage`
    /// stores.
    Bgra(Vec<u8>),
    /// BGRA8 in exported GPU memory. Holding it keeps the buffer from being
    /// reused; drop it once the picture is replaced.
    Shared(SharedFrame),
}

impl PlayerFrame {
    /// The bytes, on the readback path.
    pub fn bgra(&self) -> Option<&[u8]> {
        match &self.pixels {
            FramePixels::Bgra(bytes) => Some(bytes),
            FramePixels::Shared(_) => None,
        }
    }
}

/// How frames reach the UI. See [`FramePlayer::use_shared_frames`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sharing {
    /// Read back to system memory, as asked.
    Readback,
    /// In exported GPU memory.
    Shared,
    /// Sharing was asked for, and this device cannot export memory; frames
    /// are read back.
    Unavailable,
}

impl Sharing {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Shared,
            2 => Self::Unavailable,
            _ => Self::Readback,
        }
    }
}

/// Counters for "is playback keeping up", cumulative since the player started.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlayerStats {
    /// Frames composited and read back.
    pub rendered: u64,
    /// Frames handed to the UI.
    pub shown: u64,
    /// Rendered frames the clock passed before the UI took them.
    pub late: u64,
    /// Frame numbers never rendered because the renderer jumped ahead to stay
    /// in sync.
    pub skipped: u64,
    /// Frames thrown away by an edit, a seek or a resize.
    pub discarded: u64,
    /// Smoothed time from starting a frame to having its bytes, in ms.
    pub latency_ms: f64,
    /// Mean milliseconds per frame spent waiting on the GPU for a readback,
    /// and copying it out.
    pub readback_wait_ms: f64,
    pub readback_copy_ms: f64,
    /// Frames handed out in shared GPU memory rather than as bytes.
    pub shared: u64,
}

struct Arrived {
    request: PlayerRequest,
    at: Instant,
    seq: u64,
}

#[derive(Default)]
struct Shared {
    request: Mutex<Option<Arrived>>,
    wake: Condvar,
    seq: AtomicU64,
    ready: Mutex<VecDeque<PlayerFrame>>,
    stop: AtomicBool,
    failure: Mutex<Option<String>>,
    stats: Mutex<PlayerStats>,
    /// Decode from proxies where they exist. See [`FramePlayer::use_proxies`].
    proxies: AtomicBool,
    /// Share frames with the UI's device. See [`FramePlayer::use_shared_frames`].
    share: AtomicBool,
    /// What the render thread is actually doing, a [`Sharing`].
    sharing: AtomicU8,
}

impl Shared {
    fn notify(&self) {
        // Taking the lock orders the notify after any waiter's check, so a
        // wake-up between the check and the wait cannot be lost.
        let _guard = self.request.lock();
        self.wake.notify_one();
    }
}

/// The player. Dropping it stops the thread.
pub struct FramePlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl FramePlayer {
    /// A player on the process's render device. A machine without one gets a
    /// player whose [`Self::failure`] says so.
    pub fn new() -> Self {
        Self::start(crate::modules::gpu::render_context())
    }

    /// A player on `ctx`, for benchmarks and tests that already hold one.
    pub fn with_context(ctx: Arc<RenderContext>) -> Self {
        Self::start(Some(ctx))
    }

    fn start(ctx: Option<Arc<RenderContext>>) -> Self {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("chukcut-player".into())
                .spawn(move || match ctx {
                    Some(ctx) => Renderer::new(ctx, shared).run(),
                    None => *shared.failure.lock() = Some("No usable GPU adapter".into()),
                })
                .expect("spawn the player thread")
        };
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Say what should be on screen. Replaces the previous request.
    pub fn request(&self, mut request: PlayerRequest) {
        request.size = (request.size.0.max(2) & !1, request.size.1.max(2) & !1);
        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut slot = self.shared.request.lock();
        *slot = Some(Arrived {
            request,
            at: Instant::now(),
            seq,
        });
        self.shared.wake.notify_one();
    }

    /// The frame to show now that the clock reads `clock`, if a new one is due.
    ///
    /// Paused, that is simply the newest frame. Playing, it is the newest frame
    /// whose instant the clock has reached; frames it overtook are dropped, and
    /// frames still in the future stay in the ring.
    pub fn take(&self, clock: Micros) -> Option<PlayerFrame> {
        let (playing, generation, size, fps) = {
            let slot = self.shared.request.lock();
            let request = &slot.as_ref()?.request;
            (
                request.playing,
                request.generation,
                request.size,
                request.project.fps,
            )
        };
        let mut ready = self.shared.ready.lock();
        let before = ready.len();
        ready.retain(|f| f.generation == generation && (f.width, f.height) == size);
        let stale = (before - ready.len()) as u64;

        let mut late = 0;
        let chosen = if playing {
            let due = frame_at(clock, fps);
            let mut chosen = None;
            while ready
                .front()
                .is_some_and(|frame| frame_at(frame.time, fps) <= due)
            {
                if chosen.is_some() {
                    late += 1;
                }
                chosen = ready.pop_front();
            }
            chosen
        } else {
            let newest = ready.pop_back();
            late += ready.len() as u64;
            ready.clear();
            newest
        };
        drop(ready);

        let mut stats = self.shared.stats.lock();
        stats.discarded += stale;
        stats.late += late;
        if chosen.is_some() {
            stats.shown += 1;
        }
        drop(stats);
        if chosen.is_some() || late > 0 {
            // Room in the ring.
            self.shared.notify();
        }
        chosen
    }

    /// Preview from proxy media where the cache has it (the settings' proxy
    /// policy is not `Off`). Takes effect with the next request; a proxy that
    /// finishes while the player runs is picked up the same way.
    pub fn use_proxies(&self, on: bool) {
        self.shared.proxies.store(on, Ordering::Relaxed);
        self.shared.notify();
    }

    /// Hand frames out in exported GPU memory ([`FramePixels::Shared`]) instead
    /// of reading them back. Takes effect with the next request and throws
    /// away the frames rendered the other way. When the device cannot export
    /// memory the player keeps reading back and [`Self::sharing`] says
    /// [`Sharing::Unavailable`]; turning it off is how a consumer that cannot
    /// import (GPUI on another GPU) falls back.
    pub fn use_shared_frames(&self, on: bool) {
        if self.shared.share.swap(on, Ordering::Relaxed) == on {
            return;
        }
        // The current request again, under a new sequence number: the render
        // thread adopts it, sees the changed key and renders the frame anew,
        // so a paused preview is not left showing a frame nobody can draw.
        let seq = self.shared.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let mut slot = self.shared.request.lock();
        if let Some(arrived) = slot.as_mut() {
            arrived.seq = seq;
        }
        self.shared.wake.notify_one();
    }

    /// How the frames being rendered now reach the UI.
    pub fn sharing(&self) -> Sharing {
        Sharing::from_u8(self.shared.sharing.load(Ordering::Relaxed))
    }

    /// Why there is no picture, when the GPU could not be opened.
    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().clone()
    }

    pub fn stats(&self) -> PlayerStats {
        *self.shared.stats.lock()
    }
}

impl Default for FramePlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FramePlayer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.notify();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// --- the render thread ------------------------------------------------------------

/// What travels with a frame through the readback ring.
struct Tag {
    time: Micros,
    generation: u64,
    started: Instant,
}

/// What a frame was rendered for. A change of any of it throws the ring away.
#[derive(Clone, PartialEq)]
struct Key {
    generation: u64,
    size: (u32, u32),
    project: usize,
    /// Whether proxies are on, and how many the proxy queue has finished: a
    /// new proxy changes which file a clip decodes from.
    proxies: Option<u64>,
    /// Frames go out shared rather than as bytes.
    shared: bool,
}

impl Key {
    fn of(request: &PlayerRequest, proxies: bool, shared: bool) -> Self {
        Self {
            generation: request.generation,
            size: request.size,
            project: Arc::as_ptr(&request.project) as usize,
            proxies: proxies.then(|| crate::modules::proxy::ProxyQueue::shared().generation()),
            shared,
        }
    }
}

/// The two ways a composited frame leaves the render thread, behind one
/// submit-and-collect shape.
struct Output {
    readback: BgraReadback<Tag>,
    /// Made on the first request for sharing; `Err` once that failed, so it
    /// is not retried every frame.
    shared: Option<std::result::Result<SharedFrames<Tag>, ()>>,
    /// Which of the two the ring is using now.
    sharing: bool,
}

type Collected = (Tag, crate::modules::render::Result<(u32, u32, FramePixels)>);

impl Output {
    /// Switch to sharing or away from it. Answers whether frames are now
    /// shared. The caller has discarded whatever was in flight.
    fn set_sharing(&mut self, ctx: &Arc<RenderContext>, on: bool) -> bool {
        if on && self.shared.is_none() {
            self.shared = Some(SharedFrames::new(Arc::clone(ctx), READBACK_DEPTH).ok_or(()));
            if matches!(self.shared, Some(Err(()))) {
                tracing::info!("this device cannot share frames with the UI; reading them back");
            }
        }
        self.sharing = on && matches!(self.shared, Some(Ok(_)));
        self.sharing
    }

    fn shared(&mut self) -> Option<&mut SharedFrames<Tag>> {
        match (&mut self.shared, self.sharing) {
            (Some(Ok(shared)), true) => Some(shared),
            _ => None,
        }
    }

    fn in_flight(&self) -> usize {
        match (&self.shared, self.sharing) {
            (Some(Ok(shared)), true) => shared.in_flight(),
            _ => self.readback.in_flight(),
        }
    }

    fn has_room(&self) -> bool {
        match (&self.shared, self.sharing) {
            (Some(Ok(shared)), true) => shared.has_room(),
            _ => self.readback.has_room(),
        }
    }

    fn stats(&self) -> ReadbackStats {
        match (&self.shared, self.sharing) {
            (Some(Ok(shared)), true) => shared.stats(),
            _ => self.readback.stats(),
        }
    }

    fn submit(&mut self, target: &PooledTexture, tag: Tag) -> std::result::Result<(), Tag> {
        match self.shared() {
            Some(shared) => shared.submit(target, tag),
            None => self.readback.submit(target, tag),
        }
    }

    fn try_collect(&mut self) -> Option<Collected> {
        match self.shared() {
            Some(shared) => shared.try_collect().map(shared_pixels),
            None => self.readback.try_collect().map(bgra_pixels),
        }
    }

    fn collect_oldest(&mut self) -> Option<Collected> {
        match self.shared() {
            Some(shared) => shared.collect_oldest().map(shared_pixels),
            None => self.readback.collect_oldest().map(bgra_pixels),
        }
    }

    fn discard_all(&mut self) -> usize {
        let mut thrown = self.readback.discard_all().len();
        if let Some(Ok(shared)) = &mut self.shared {
            thrown += shared.discard_all().len();
        }
        thrown
    }
}

fn shared_pixels((tag, frame): (Tag, crate::modules::render::Result<SharedFrame>)) -> Collected {
    (
        tag,
        frame.map(|frame| (frame.width, frame.height, FramePixels::Shared(frame))),
    )
}

fn bgra_pixels(
    (tag, frame): (
        Tag,
        crate::modules::render::Result<crate::modules::render::BgraFrame>,
    ),
) -> Collected {
    (
        tag,
        frame.map(|frame| (frame.width, frame.height, FramePixels::Bgra(frame.data))),
    )
}

struct Renderer {
    ctx: Arc<RenderContext>,
    shared: Arc<Shared>,
    compositor: Compositor,
    output: Output,
    /// The provider and the material set it was built for. Rebuilt only when
    /// the files change, because it holds open decoders.
    sources: Option<(MaterialKey, Arc<MediaSourceProvider>)>,
    ahead: DecodeAhead,
    current: Option<(PlayerRequest, Instant, u64)>,
    key: Option<Key>,
    /// The next frame number to render during playback.
    cursor: Option<i64>,
    /// The paused instant already delivered, so an idle player stays idle.
    still: Option<(Micros, Key)>,
    /// Smoothed start-to-bytes latency of one frame, in microseconds.
    latency: f64,
}

impl Renderer {
    fn new(ctx: Arc<RenderContext>, shared: Arc<Shared>) -> Self {
        Self {
            compositor: Compositor::new(Arc::clone(&ctx)),
            output: Output {
                readback: BgraReadback::new(Arc::clone(&ctx), READBACK_DEPTH),
                shared: None,
                sharing: false,
            },
            ahead: DecodeAhead::new(Arc::clone(&ctx)),
            ctx,
            shared,
            sources: None,
            current: None,
            key: None,
            cursor: None,
            still: None,
            latency: 40_000.0,
        }
    }

    fn run(mut self) {
        loop {
            if self.shared.stop.load(Ordering::Acquire) {
                return;
            }
            while let Some((tag, frame)) = self.output.try_collect() {
                self.publish(tag, frame);
            }
            self.adopt_newest_request();
            let Some((request, arrived, _)) = self.current.clone() else {
                self.idle();
                continue;
            };
            if request.playing {
                self.play_step(&request, arrived);
            } else {
                self.still_step(&request);
            }
        }
    }

    /// Pick up a request newer than the one being served, and throw away
    /// whatever it invalidates.
    fn adopt_newest_request(&mut self) {
        let newest = {
            let slot = self.shared.request.lock();
            match slot.as_ref() {
                Some(arrived) if Some(arrived.seq) != self.current.as_ref().map(|c| c.2) => {
                    Some((arrived.request.clone(), arrived.at, arrived.seq))
                }
                _ => None,
            }
        };
        let Some((request, at, seq)) = newest else {
            return;
        };
        let key = self.key_of(&request);
        if self.key.as_ref() != Some(&key) {
            let files_changed = self
                .key
                .as_ref()
                .is_none_or(|old| old.proxies != key.proxies);
            self.discard_everything();
            self.cursor = None;
            self.prepare_sources(&request.project, files_changed, key.proxies.is_some());
            let sharing = self.output.set_sharing(&self.ctx, key.shared);
            self.shared.sharing.store(
                match (key.shared, sharing) {
                    (_, true) => 1,
                    (true, false) => 2,
                    (false, false) => 0,
                },
                Ordering::Relaxed,
            );
            self.key = Some(key);
        }
        self.current = Some((request, at, seq));
    }

    fn key_of(&self, request: &PlayerRequest) -> Key {
        // A device that cannot share keeps a key that says so, or every
        // request would look like a change and throw the ring away.
        let shared = self.shared.share.load(Ordering::Relaxed)
            && !matches!(self.output.shared, Some(Err(())));
        Key::of(request, self.shared.proxies.load(Ordering::Relaxed), shared)
    }

    /// Bring the provider up to date with `project`: rebuilt when the files
    /// changed (or which of them are proxied), titles resynced otherwise.
    fn prepare_sources(&mut self, project: &Project, files_changed: bool, proxies: bool) {
        self.ahead.wait_idle();
        let key = material_key(project);
        let rebuild = files_changed || self.sources.as_ref().is_none_or(|(known, _)| known != &key);
        if rebuild {
            let provider = MediaSourceProvider::from_project(project);
            let provider = if proxies {
                provider.with_preview_proxies()
            } else {
                provider
            };
            self.sources = Some((key, Arc::new(provider)));
            return;
        }
        if let Some((_, provider)) = self.sources.as_mut() {
            match Arc::get_mut(provider) {
                Some(provider) => provider.sync_texts(project),
                // The decode-ahead still holds it — it never should once idle,
                // but a fresh provider is always correct.
                None => {
                    let fresh = MediaSourceProvider::from_project(project);
                    *provider = Arc::new(if proxies {
                        fresh.with_preview_proxies()
                    } else {
                        fresh
                    });
                }
            }
        }
    }

    fn provider(&self) -> Arc<MediaSourceProvider> {
        Arc::clone(&self.sources.as_ref().expect("prepared with the request").1)
    }

    /// Paused or scrubbing: render exactly the requested instant, once.
    fn still_step(&mut self, request: &PlayerRequest) {
        self.cursor = None;
        // The key the request was adopted under, not one computed afresh: a
        // flag flipped since (sharing, proxies) is the next adoption's to
        // act on. Recomputing here rendered a frame the old way, marked it
        // done the new way, and the adoption then threw it away.
        let key = self.key.clone().unwrap_or_else(|| self.key_of(request));
        if self.still.as_ref() == Some(&(request.time, key.clone())) {
            self.idle();
            return;
        }
        let fps = request.project.fps;
        let wanted = frame_at(request.time, fps);
        // Playback just stopped: the frame it stopped on is probably in the
        // ring already, rendered ahead.
        let found = {
            let mut ready = self.shared.ready.lock();
            let hit = ready
                .iter()
                .position(|f| frame_at(f.time, fps) == wanted && f.generation == key.generation);
            match hit {
                Some(index) => {
                    let frame = ready.remove(index);
                    ready.clear();
                    ready.extend(frame);
                    true
                }
                None => false,
            }
        };
        if found {
            self.discard_in_flight();
            self.still = Some((request.time, key));
            self.shared.notify();
            return;
        }

        self.discard_everything();
        self.ahead.wait_idle();
        let provider = self.provider();
        provider.prefetch(&self.ctx, &request.project, request.time, request.size);
        self.render(request, request.time, &provider);
        if let Some((tag, frame)) = self.output.collect_oldest() {
            self.publish(tag, frame);
        }
        self.still = Some((request.time, key));
    }

    /// Playing: keep the ring [`READ_AHEAD`] frames ahead of the clock.
    fn play_step(&mut self, request: &PlayerRequest, arrived: Instant) {
        self.still = None;
        let fps = request.project.fps;
        let frame_us = 1e6 / fps.max(1.0);
        // The request is the clock at the UI's last tick; it has moved since.
        let since = arrived.elapsed().min(Duration::from_millis(250));
        let now = frame_at(request.time + since.as_micros() as Micros, fps);
        // A frame started now is ready this many frames from now.
        let lead = (self.latency / frame_us)
            .ceil()
            .clamp(1.0, 2.0 * READ_AHEAD as f64) as i64;

        let cursor = match self.cursor {
            // Behind the clock: jump to the frame that will be due when this
            // one is done, instead of rendering the past.
            Some(cursor) if cursor <= now => {
                let to = now + lead;
                self.shared.stats.lock().skipped += (to - cursor) as u64;
                to
            }
            // A seek backwards, or a loop.
            Some(cursor) if cursor > now + lead + READ_AHEAD as i64 + 2 => {
                self.discard_everything();
                now + lead
            }
            Some(cursor) => cursor,
            None => now,
        };
        self.cursor = Some(cursor);

        let queued = self.shared.ready.lock().len() + self.output.in_flight();
        if queued >= READ_AHEAD || cursor > now + lead + READ_AHEAD as i64 {
            // Far enough ahead. Collect what the GPU has, or wait for the UI to
            // take a frame.
            if self.output.in_flight() > 0 {
                if let Some((tag, frame)) = self.output.collect_oldest() {
                    self.publish(tag, frame);
                }
            } else {
                self.idle();
            }
            return;
        }

        let time = frame_time(cursor, fps) + SAMPLE_SLACK;
        self.ahead.wait_idle();
        let provider = self.provider();
        self.render(request, time, &provider);
        if provider.hands_out_decoder_surfaces() {
            // The next decode may recycle a surface the GPU is still reading.
            let _ = self.ctx.device().poll(wgpu::PollType::wait_indefinitely());
        }
        let next = frame_time(cursor + 1, fps) + SAMPLE_SLACK;
        self.ahead
            .start(provider, Arc::clone(&request.project), next, request.size);
        self.cursor = Some(cursor + 1);
    }

    /// Composite `time` and queue its readback. Collects the oldest frame
    /// first if the ring is full.
    fn render(&mut self, request: &PlayerRequest, time: Micros, provider: &MediaSourceProvider) {
        let started = Instant::now();
        let target =
            match self
                .compositor
                .render_to_texture(&request.project, time, request.size, provider)
            {
                Ok(target) => target,
                Err(error) => {
                    tracing::warn!(%error, time, "preview frame failed");
                    return;
                }
            };
        if !self.output.has_room() {
            if let Some((tag, frame)) = self.output.collect_oldest() {
                self.publish(tag, frame);
            }
        }
        let tag = Tag {
            time,
            generation: request.generation,
            started,
        };
        if self.output.submit(&target, tag).is_err() {
            tracing::warn!("the preview output refused a frame");
        }
        self.compositor.pool().release(target);
    }

    fn publish(
        &mut self,
        tag: Tag,
        frame: crate::modules::render::Result<(u32, u32, FramePixels)>,
    ) {
        let (width, height, pixels) = match frame {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "preview readback failed");
                return;
            }
        };
        let elapsed = tag.started.elapsed().as_micros() as f64;
        self.latency = self.latency * 0.8 + elapsed * 0.2;
        let current = self.key.as_ref().map(|k| k.generation);
        let stats = self.output.stats();
        {
            let mut shared = self.shared.stats.lock();
            shared.rendered += 1;
            shared.latency_ms = self.latency / 1000.0;
            let frames = stats.frames.max(1) as f64;
            shared.readback_wait_ms = stats.wait_ns as f64 / frames / 1e6;
            shared.readback_copy_ms = stats.copy_ns as f64 / frames / 1e6;
            if matches!(pixels, FramePixels::Shared(_)) {
                shared.shared += 1;
            }
            if current != Some(tag.generation) {
                shared.discarded += 1;
                return;
            }
        }
        let frame = PlayerFrame {
            time: tag.time,
            generation: tag.generation,
            width,
            height,
            pixels,
        };
        let mut ready = self.shared.ready.lock();
        let at = ready.partition_point(|f| f.time < frame.time);
        if ready.get(at).is_some_and(|f| f.time == frame.time) {
            ready[at] = frame;
        } else {
            ready.insert(at, frame);
        }
    }

    fn discard_in_flight(&mut self) {
        let thrown = self.output.discard_all() as u64;
        if thrown > 0 {
            self.shared.stats.lock().discarded += thrown;
        }
    }

    fn discard_everything(&mut self) {
        self.discard_in_flight();
        let thrown = {
            let mut ready = self.shared.ready.lock();
            let n = ready.len() as u64;
            ready.clear();
            n
        };
        self.shared.stats.lock().discarded += thrown;
    }

    /// Sleep until a new request, a frame taken, or [`IDLE`].
    fn idle(&self) {
        let mut slot = self.shared.request.lock();
        let unchanged = slot.as_ref().map(|a| a.seq) == self.current.as_ref().map(|c| c.2);
        if unchanged && !self.shared.stop.load(Ordering::Acquire) {
            self.shared.wake.wait_for(&mut slot, IDLE);
        }
    }
}

/// Which media a provider was built for. Text materials are in the key with
/// their content and style, because the provider snapshots them.
type MaterialKey = Vec<(String, String)>;

fn material_key(project: &Project) -> MaterialKey {
    let pool = &project.materials;
    let mut key: Vec<(String, String)> = pool
        .videos
        .iter()
        .map(|m| (m.id.clone(), m.path.clone()))
        .chain(pool.images.iter().map(|m| (m.id.clone(), m.path.clone())))
        .collect();
    key.sort();
    key
}

// --- decode-ahead -----------------------------------------------------------------------

struct AheadJob {
    provider: Arc<MediaSourceProvider>,
    project: Arc<Project>,
    time: Micros,
    size: (u32, u32),
}

#[derive(Default)]
struct AheadState {
    job: Option<AheadJob>,
    busy: bool,
    stop: bool,
}

/// One helper thread that runs [`MediaSourceProvider::prefetch`] for the next
/// frame while the render thread finishes this one.
struct DecodeAhead {
    state: Arc<(Mutex<AheadState>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl DecodeAhead {
    fn new(ctx: Arc<RenderContext>) -> Self {
        let state: Arc<(Mutex<AheadState>, Condvar)> = Arc::default();
        let thread = {
            let state = Arc::clone(&state);
            std::thread::Builder::new()
                .name("chukcut-decode-ahead".into())
                .spawn(move || loop {
                    let job = {
                        let mut guard = state.0.lock();
                        loop {
                            if guard.stop {
                                return;
                            }
                            if let Some(job) = guard.job.take() {
                                guard.busy = true;
                                break job;
                            }
                            state.1.wait(&mut guard);
                        }
                    };
                    job.provider
                        .prefetch(&ctx, &job.project, job.time, job.size);
                    // Drop the job's provider before saying so: the render
                    // thread may want it back exclusively.
                    drop(job);
                    state.0.lock().busy = false;
                    state.1.notify_all();
                })
                .expect("spawn the decode-ahead thread")
        };
        Self {
            state,
            thread: Some(thread),
        }
    }

    fn start(
        &self,
        provider: Arc<MediaSourceProvider>,
        project: Arc<Project>,
        time: Micros,
        size: (u32, u32),
    ) {
        let mut guard = self.state.0.lock();
        guard.job = Some(AheadJob {
            provider,
            project,
            time,
            size,
        });
        self.state.1.notify_all();
    }

    /// Wait until nothing is queued or running.
    fn wait_idle(&self) {
        let mut guard = self.state.0.lock();
        while guard.busy || guard.job.is_some() {
            self.state.1.wait(&mut guard);
        }
    }
}

impl Drop for DecodeAhead {
    fn drop(&mut self) {
        {
            let mut guard = self.state.0.lock();
            guard.stop = true;
            guard.job = None;
        }
        self.state.1.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::{
        CanvasConfig, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    };

    /// A project whose one clip's file does not exist: the provider answers
    /// with the missing-media field, so frames render without media.
    fn project() -> Arc<Project> {
        let mut project = Project::new(
            "player",
            CanvasConfig {
                width: 320,
                height: 180,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        );
        project.materials.videos.push(VideoMaterial {
            id: "gone".into(),
            path: "/nonexistent/clip.mp4".into(),
            width: 320,
            height: 180,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "clip".into(),
            material_id: "gone".into(),
            target_range: TimeRange::new(0, 10_000_000),
            source_range: TimeRange::new(0, 10_000_000),
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

    fn request(project: &Arc<Project>, time: Micros, playing: bool) -> PlayerRequest {
        PlayerRequest {
            project: Arc::clone(project),
            generation: 1,
            time: time + SAMPLE_SLACK,
            size: (160, 90),
            playing,
        }
    }

    fn wait_for(player: &FramePlayer, clock: Micros) -> Option<PlayerFrame> {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if let Some(frame) = player.take(clock) {
                return Some(frame);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn a_paused_request_renders_exactly_that_instant_as_bgra() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let player = FramePlayer::with_context(ctx);
        let project = project();
        player.request(request(&project, 1_000_000, false));
        let frame = wait_for(&player, 1_000_000).expect("a frame");
        assert_eq!(frame.time, 1_000_000 + SAMPLE_SLACK);
        assert_eq!((frame.width, frame.height), (160, 90));
        let bgra = frame.bgra().expect("bytes unless sharing was asked for");
        assert_eq!(bgra.len(), 160 * 90 * 4);
        // The missing-media field is RGBA (122, 26, 26): as BGRA the red is
        // the third byte.
        let centre = ((45 * 160 + 80) * 4) as usize;
        assert_eq!(&bgra[centre..centre + 4], &[26, 26, 122, 255], "BGRA order");
        assert_eq!(player.sharing(), Sharing::Readback);
    }

    /// Copy a shared frame back out on the engine's device, tightly packed.
    fn read_shared(ctx: &RenderContext, frame: &SharedFrame) -> Vec<u8> {
        let bytes = u64::from(frame.bytes_per_row) * u64::from(frame.height);
        let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("test staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = ctx.device().create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(frame.buffer.buffer(), 0, &staging, 0, bytes);
        ctx.queue().submit(Some(encoder.finish()));
        staging.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let view = staging.slice(..).get_mapped_range().expect("mapped");
        view.chunks(frame.bytes_per_row as usize)
            .flat_map(|row| row[..frame.width as usize * 4].to_vec())
            .collect()
    }

    /// The shared path: same picture, no bytes in system memory. Skipped on a
    /// device that cannot export memory, where the player says so and reads
    /// back instead.
    #[test]
    fn a_player_asked_to_share_hands_out_the_same_picture_in_gpu_memory() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let player = FramePlayer::with_context(Arc::clone(&ctx));
        player.use_shared_frames(true);
        let project = project();
        player.request(request(&project, 1_000_000, false));
        let frame = wait_for(&player, 1_000_000).expect("a frame");
        let FramePixels::Shared(shared) = &frame.pixels else {
            assert_eq!(player.sharing(), Sharing::Unavailable);
            assert_eq!(frame.bgra().map(<[u8]>::len), Some(160 * 90 * 4));
            eprintln!("skipping: this device cannot share frames; the fallback read back");
            return;
        };
        assert_eq!(player.sharing(), Sharing::Shared);
        assert_eq!((shared.width, shared.height), (160, 90));
        assert_eq!(shared.bytes_per_row, 768);
        let bgra = read_shared(&ctx, shared);
        let centre = ((45 * 160 + 80) * 4) as usize;
        assert_eq!(&bgra[centre..centre + 4], &[26, 26, 122, 255], "BGRA order");
        assert_eq!(player.stats().shared, 1);
    }

    /// Turning sharing off — what the app does when GPUI cannot import — goes
    /// back to bytes with the next request, and the frames rendered shared are
    /// not handed out after it.
    #[test]
    fn turning_sharing_off_falls_back_to_bytes() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let player = FramePlayer::with_context(ctx);
        player.use_shared_frames(true);
        let project = project();
        player.request(request(&project, 0, false));
        let _ = wait_for(&player, 0).expect("a frame");
        player.use_shared_frames(false);
        player.request(request(&project, 33_334, false));
        let frame = wait_for(&player, 33_334).expect("a frame after the switch");
        assert!(frame.bgra().is_some(), "bytes after sharing was turned off");
        assert_eq!(player.sharing(), Sharing::Readback);
    }

    #[test]
    fn playback_hands_out_frames_in_order_and_never_from_the_future() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let player = FramePlayer::with_context(ctx);
        let project = project();
        let started = Instant::now();
        let mut last = -1;
        let mut shown = 0;
        while started.elapsed() < Duration::from_millis(600) {
            let clock = started.elapsed().as_micros() as Micros;
            player.request(request(&project, clock, true));
            if let Some(frame) = player.take(clock) {
                let n = frame_at(frame.time, 30.0);
                assert!(n > last, "frame {n} after {last}");
                assert!(
                    n <= frame_at(clock, 30.0),
                    "frame {n} shown before its time"
                );
                last = n;
                shown += 1;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        assert!(shown >= 5, "only {shown} frames in 600 ms");
    }

    #[test]
    fn an_edit_throws_away_frames_rendered_for_the_old_document() {
        let Some(ctx) = crate::modules::render::test_context() else {
            return;
        };
        let player = FramePlayer::with_context(ctx);
        let project = project();
        player.request(request(&project, 0, false));
        let _ = wait_for(&player, 0).expect("a frame");
        let mut edited = request(&project, 0, false);
        edited.generation = 2;
        player.request(edited);
        let frame = wait_for(&player, 0).expect("a frame for the edit");
        assert_eq!(frame.generation, 2);
    }
}
