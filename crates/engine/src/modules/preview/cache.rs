//! The frame ring.
//!
//! A fixed number of encoded frames, addressed by frame number. Playback reads
//! ahead into it and the protocol handler reads out of it, which is the whole
//! reason it exists: rendering is jittery — a keyframe-heavy second costs twice
//! what the next one does — and playback is not allowed to be. The ring absorbs
//! the difference, the same way a terminal keeps scrollback instead of
//! re-deriving the screen.
//!
//! It is direct-mapped: frame `n` lives in slot `n % capacity`, so writing
//! frame `n` evicts frame `n - capacity` and nothing else. No LRU bookkeeping,
//! no allocation per insert, and eviction order is exactly playback order.
//!
//! Every entry is tagged with its session and the ring as a whole holds exactly
//! one session's frames. A lookup for any other session is [`Lookup::Stale`],
//! which the frame server turns into a 410 — never a frame from before the
//! seek, which would paint the old position over the new one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use crate::modules::project::document::Micros;

/// Two seconds behind the playhead plus the read-ahead window in front.
///
/// This used to be 90 with a comment about "riding out a slow patch", which
/// was a fiction: the renderer never runs more than `DEFAULT_READ_AHEAD` = 12
/// frames past the playhead, so 78 of the 90 slots could only ever hold
/// frames *behind* it. Frames behind the playhead are not useless — the
/// webview's requests trail the position announcements, and a short scrub
/// back lands on them — but two seconds of trail is ample for both, and the
/// profile (`docs/research/preview-performance.md`) called the rest dead
/// weight. Sized as trail + read-ahead so the two constants cannot drift
/// apart again: if the read-ahead grows, the ring grows with it.
pub const DEFAULT_CAPACITY: usize = 60 + super::clock::DEFAULT_READ_AHEAD;

/// One encoded frame, ready to hand to the webview verbatim.
#[derive(Debug, Clone)]
pub struct CachedFrame {
    pub session: u64,
    pub frame: i64,
    /// Timeline position this frame depicts.
    pub time: Micros,
    /// JPEG bytes. `Arc` because the protocol handler hands them out while the
    /// render thread is already working on the next frame.
    pub bytes: Arc<[u8]>,
}

/// The three answers a lookup can have, which are also three different HTTP
/// statuses. Keeping them distinct is what stops a stale request from being
/// answered with a fresh frame that happens to have the same number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Hit(Arc<[u8]>),
    /// Right session, frame not rendered yet (or already evicted).
    Miss,
    /// Wrong session. The frame this request wanted no longer exists anywhere.
    Stale,
}

impl Lookup {
    pub fn is_hit(&self) -> bool {
        matches!(self, Lookup::Hit(_))
    }
}

pub struct FrameCache {
    slots: Mutex<Slots>,
    /// Signalled on every insert so a waiting protocol handler wakes the
    /// instant its frame lands rather than polling for it.
    inserted: Condvar,
}

struct Slots {
    ring: Vec<Option<CachedFrame>>,
    session: u64,
    len: usize,
}

impl FrameCache {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            slots: Mutex::new(Slots {
                ring: vec![None; capacity],
                session: 0,
                len: 0,
            }),
            inserted: Condvar::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.lock().ring.len()
    }

    /// The session this ring currently holds frames for. 0 before the first
    /// session opens.
    pub fn session(&self) -> u64 {
        self.slots.lock().session
    }

    pub fn len(&self) -> usize {
        self.slots.lock().len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drop everything and adopt `session`.
    ///
    /// This is what a seek does. Frames rendered for the previous session are
    /// not merely unhelpful, they are wrong, so there is no partial
    /// invalidation to be clever about.
    pub fn reset(&self, session: u64) {
        let mut slots = self.slots.lock();
        for slot in slots.ring.iter_mut() {
            *slot = None;
        }
        slots.len = 0;
        slots.session = session;
        // Anyone blocked on a frame from the old session must be told now, not
        // when their timeout expires.
        self.inserted.notify_all();
    }

    /// Store a frame, unless its session has already been superseded.
    ///
    /// Returns whether it was kept. A render that finishes after a seek lands
    /// here and is dropped on the floor, which is exactly what should happen to
    /// it — the alternative is a frame for the wrong position sitting in the
    /// ring waiting to be served.
    pub fn insert(&self, frame: CachedFrame) -> bool {
        let mut slots = self.slots.lock();
        if frame.session != slots.session {
            return false;
        }
        let index = slot_index(frame.frame, slots.ring.len());
        if slots.ring[index].is_none() {
            slots.len += 1;
        }
        slots.ring[index] = Some(frame);
        drop(slots);
        self.inserted.notify_all();
        true
    }

    pub fn get(&self, session: u64, frame: i64) -> Lookup {
        let slots = self.slots.lock();
        Self::lookup(&slots, session, frame)
    }

    /// The frame of `session` closest to `frame`, within `tolerance` frames.
    ///
    /// This exists because a frame request must never fail: WebKitGTK responds
    /// to a stream of failed resource loads by killing its web process — an
    /// actual crash, observed — so answering "not ready" with an error status
    /// is not an option at video rates.
    ///
    /// The tolerance is the part that took a second attempt to get right. An
    /// unbounded search returns *whatever is in the ring*, so a request for
    /// frame 200 against a ring holding 170 answers with 170 and the picture
    /// jumps thirty frames backwards. A neighbouring frame is indistinguishable
    /// from a slightly late one; a frame a second old is a visible glitch, and
    /// worse than simply leaving the previous picture up.
    pub fn nearest(&self, session: u64, frame: i64, tolerance: i64) -> Option<(i64, Arc<[u8]>)> {
        let slots = self.slots.lock();
        if slots.session != session {
            return None;
        }
        slots
            .ring
            .iter()
            .flatten()
            .filter(|cached| cached.session == session)
            .filter(|cached| (cached.frame - frame).abs() <= tolerance)
            .min_by_key(|cached| (cached.frame - frame).abs())
            .map(|cached| (cached.frame, Arc::clone(&cached.bytes)))
    }

    /// [`Self::get`], but wait up to `timeout` for the frame to be rendered.
    ///
    /// The protocol handler uses this so that "the frame is 8 ms away" reads as
    /// a slightly slow image rather than a 404 and a blank viewer. The timeout
    /// is short by design: a request that cannot be answered promptly is
    /// answered with a miss, because the frontend can ask again and a hung
    /// protocol handler blocks the webview.
    pub fn wait(&self, session: u64, frame: i64, timeout: Duration) -> Lookup {
        let deadline = Instant::now() + timeout;
        let mut slots = self.slots.lock();
        loop {
            match Self::lookup(&slots, session, frame) {
                Lookup::Miss => {}
                found => return found,
            }
            let now = Instant::now();
            if now >= deadline {
                return Lookup::Miss;
            }
            self.inserted.wait_for(&mut slots, deadline - now);
        }
    }

    fn lookup(slots: &Slots, session: u64, frame: i64) -> Lookup {
        if session != slots.session {
            return Lookup::Stale;
        }
        let index = slot_index(frame, slots.ring.len());
        match &slots.ring[index] {
            Some(hit) if hit.session == session && hit.frame == frame => {
                Lookup::Hit(Arc::clone(&hit.bytes))
            }
            _ => Lookup::Miss,
        }
    }

    /// Frame numbers currently held, ascending. For diagnostics and tests.
    pub fn frames(&self) -> Vec<i64> {
        let slots = self.slots.lock();
        let mut frames: Vec<i64> = slots.ring.iter().flatten().map(|f| f.frame).collect();
        frames.sort_unstable();
        frames
    }
}

impl Default for FrameCache {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl std::fmt::Debug for FrameCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slots = self.slots.lock();
        f.debug_struct("FrameCache")
            .field("capacity", &slots.ring.len())
            .field("session", &slots.session)
            .field("len", &slots.len)
            .finish()
    }
}

/// `rem_euclid` rather than `%` so a negative frame number — which a clock
/// nudged before zero can produce — indexes a real slot instead of panicking.
fn slot_index(frame: i64, capacity: usize) -> usize {
    frame.rem_euclid(capacity as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(session: u64, n: i64) -> CachedFrame {
        CachedFrame {
            session,
            frame: n,
            time: n * 33_333,
            bytes: Arc::from(vec![n as u8; 4].into_boxed_slice()),
        }
    }

    fn cache(capacity: usize, session: u64) -> FrameCache {
        let cache = FrameCache::new(capacity);
        cache.reset(session);
        cache
    }

    #[test]
    fn a_stored_frame_comes_back() {
        let cache = cache(8, 1);
        assert!(cache.insert(frame(1, 3)));
        assert_eq!(
            cache.get(1, 3),
            Lookup::Hit(Arc::from(vec![3u8; 4].into_boxed_slice()))
        );
        assert_eq!(cache.get(1, 4), Lookup::Miss);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn writing_past_the_end_evicts_the_frame_one_lap_back() {
        let cache = cache(4, 1);
        for n in 0..4 {
            cache.insert(frame(1, n));
        }
        assert_eq!(cache.frames(), vec![0, 1, 2, 3]);

        // Frame 4 shares a slot with frame 0.
        cache.insert(frame(1, 4));
        assert_eq!(cache.get(1, 0), Lookup::Miss, "frame 0 was evicted");
        assert!(cache.get(1, 4).is_hit());
        assert_eq!(cache.frames(), vec![1, 2, 3, 4]);
        assert_eq!(cache.len(), 4, "the ring never grows past its capacity");
    }

    #[test]
    fn wraparound_does_not_confuse_frames_that_share_a_slot() {
        let cache = cache(4, 1);
        cache.insert(frame(1, 2));
        // 6 and 2 map to the same slot; asking for 6 must not get 2's bytes.
        assert_eq!(cache.get(1, 6), Lookup::Miss);
        assert!(cache.get(1, 2).is_hit());
    }

    #[test]
    fn negative_frame_numbers_do_not_panic() {
        let cache = cache(4, 1);
        cache.insert(frame(1, -1));
        assert!(cache.get(1, -1).is_hit());
        assert_eq!(cache.get(1, 3), Lookup::Miss);
    }

    #[test]
    fn invalidation_drops_everything_and_moves_to_the_new_session() {
        let cache = cache(8, 1);
        for n in 0..5 {
            cache.insert(frame(1, n));
        }
        cache.reset(2);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.session(), 2);
        // The old session's frames are gone, and gone loudly.
        assert_eq!(cache.get(1, 0), Lookup::Stale);
        assert_eq!(cache.get(2, 0), Lookup::Miss);
    }

    #[test]
    fn a_frame_from_a_superseded_session_is_refused_on_insert() {
        let cache = cache(8, 2);
        // A render that was in flight when the user seeked.
        assert!(!cache.insert(frame(1, 0)));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get(2, 0), Lookup::Miss, "and it did not sneak in");
    }

    #[test]
    fn a_lookup_for_a_future_session_is_stale_not_a_miss() {
        let cache = cache(8, 2);
        cache.insert(frame(2, 0));
        assert_eq!(cache.get(3, 0), Lookup::Stale);
    }

    #[test]
    fn waiting_returns_as_soon_as_the_frame_lands() {
        let cache = Arc::new(cache(8, 1));
        let writer = Arc::clone(&cache);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            writer.insert(frame(1, 7));
        });

        let started = Instant::now();
        let found = cache.wait(1, 7, Duration::from_millis(2_000));
        assert!(found.is_hit());
        assert!(
            started.elapsed() < Duration::from_millis(1_000),
            "woke on the insert, not on the timeout"
        );
        handle.join().unwrap();
    }

    #[test]
    fn waiting_gives_up_rather_than_hanging() {
        let cache = cache(8, 1);
        let started = Instant::now();
        assert_eq!(cache.wait(1, 7, Duration::from_millis(20)), Lookup::Miss);
        assert!(started.elapsed() >= Duration::from_millis(15));
    }

    #[test]
    fn waiting_on_a_stale_session_returns_immediately() {
        let cache = cache(8, 2);
        let started = Instant::now();
        assert_eq!(cache.wait(1, 0, Duration::from_secs(5)), Lookup::Stale);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn a_seek_wakes_everyone_waiting_on_the_old_session() {
        let cache = Arc::new(cache(8, 1));
        let seeker = Arc::clone(&cache);
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            seeker.reset(2);
        });

        assert_eq!(cache.wait(1, 0, Duration::from_secs(5)), Lookup::Stale);
        handle.join().unwrap();
    }
}
