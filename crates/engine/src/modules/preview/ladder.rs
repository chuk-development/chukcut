//! What playback is allowed to give up when it cannot keep up, and when it
//! gives it back.
//!
//! The preview has one hard constraint and one soft one. The hard one is the
//! owner's, stated repeatedly: **a paused frame must look like the project**.
//! The soft one is that playback should reach the project's frame rate. When
//! they conflict, playback loses resolution — never the paused frame.
//!
//! So the ladder only ever applies to frames rendered *while playing*. A scrub,
//! a seek and a pause all render at the session's own size and quality, and
//! [`super::server::PreviewServer::pause`] discards a ring that was filled at a
//! reduced rung so the frame the user sits and looks at is re-rendered at the
//! top of the ladder.
//!
//! ## Why steps and not a continuous scale
//!
//! Changing the render size means a new wgpu render target and, on the hardware
//! JPEG path, a new encoder and surface pool — `avcodec_open2` is not
//! repeatable, so every distinct size is a rebuild (`vaapi.rs`). Three sizes
//! are three pools; a continuously varying one would rebuild the encoder on
//! most frames and cost more than it saved.
//!
//! ## Why it climbs back slowly and falls quickly
//!
//! A stutter is one bad second and a user notices it immediately; the softness
//! of a lower rung is only noticeable when they look for it. Falling after
//! three bad frames and climbing after three good seconds is that asymmetry
//! written down. Climbing at the same rate as falling produces an oscillation
//! between two rungs, which reads as the picture *breathing* and is worse than
//! either rung.

/// One rung: a fraction of the paused render size, and how much JPEG quality
/// goes with it.
///
/// The fraction is a rational rather than a float so the arithmetic is exact
/// and the sizes are reproducible in a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rung {
    pub numerator: u32,
    pub denominator: u32,
    /// Subtracted from the session's playback quality.
    pub quality_drop: u8,
}

impl Rung {
    /// Fraction of the full frame's pixels this rung renders.
    pub fn pixel_fraction(&self) -> f64 {
        let scale = self.numerator as f64 / self.denominator as f64;
        scale * scale
    }
}

/// The rungs, best first.
///
/// Three, and no lower one, because below half the panel the softness stops
/// being something you have to look for. A machine that cannot hold half
/// resolution has a problem the preview cannot paper over, and the honest
/// answer there is the dropped-frame count in the log, not a fourth rung.
pub const RUNGS: [Rung; 3] = [
    Rung {
        numerator: 1,
        denominator: 1,
        quality_drop: 0,
    },
    Rung {
        numerator: 3,
        denominator: 4,
        quality_drop: 8,
    },
    Rung {
        numerator: 1,
        denominator: 2,
        quality_drop: 18,
    },
];

/// A playback frame is never encoded worse than this, whatever the ladder says.
///
/// Below about 60 a dark shot shows blocking that an editor reads as damage to
/// their footage, which is the same mistake `SCRUB_JPEG_QUALITY` was raised to
/// undo.
pub const MIN_QUALITY: u8 = 60;

/// Frames over budget in a row before the ladder steps down.
///
/// Three, not one: a single slow frame is a keyframe, a GC pause or the window
/// manager, and dropping the resolution for it would make the picture flicker
/// between rungs on ordinary footage.
pub const STEP_DOWN_AFTER: u32 = 3;

/// Frames inside the budget in a row before it steps back up. Three seconds at
/// 30 fps. See the module header for why this is not symmetric.
pub const STEP_UP_AFTER: u32 = 90;

/// Where playback currently is on the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ladder {
    rung: usize,
    over_budget: u32,
    /// Finished frames left in the post-adopt grace window; see `begin_grace`.
    grace: u32,
    healthy: u32,
    /// Whether this run has been below the top rung at all. What
    /// [`PreviewServer::pause`] asks, because a ring filled entirely at rung 0
    /// needs no re-render.
    ///
    /// [`PreviewServer::pause`]: super::server::PreviewServer::pause
    degraded_this_run: bool,
}

impl Default for Ladder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ladder {
    pub const fn new() -> Self {
        Self {
            rung: 0,
            over_budget: 0,
            grace: 0,
            healthy: 0,
            degraded_this_run: false,
        }
    }

    pub fn rung(&self) -> usize {
        self.rung
    }

    /// Whether anything has been rendered below the top rung since the last
    /// [`Self::reset`].
    pub fn degraded(&self) -> bool {
        self.degraded_this_run
    }

    /// The size a playback frame renders at, given the session's full size.
    ///
    /// Even, for the reason in `session::even`: an odd size takes the JPEG
    /// encode off the GPU and costs five times what the rung saved.
    pub fn size(&self, full: (u32, u32)) -> (u32, u32) {
        let rung = RUNGS[self.rung];
        if rung.numerator == rung.denominator {
            return full;
        }
        let scale = |value: u32| {
            let scaled = (value as u64 * rung.numerator as u64 / rung.denominator as u64) as u32;
            (scaled & !1).max(2)
        };
        (scale(full.0), scale(full.1))
    }

    /// The JPEG quality a playback frame is encoded at.
    pub fn quality(&self, full: u8) -> u8 {
        full.saturating_sub(RUNGS[self.rung].quality_drop)
            .max(MIN_QUALITY.min(full))
    }

    /// One playback frame finished. Returns the new rung if it moved.
    #[must_use]
    pub fn frame(&mut self, over_budget: bool) -> Option<usize> {
        self.grace = self.grace.saturating_sub(1);
        if over_budget {
            self.healthy = 0;
            self.over_budget += 1;
            if self.over_budget >= STEP_DOWN_AFTER {
                return self.step_down();
            }
            return None;
        }
        self.over_budget = 0;
        self.healthy += 1;
        if self.healthy >= STEP_UP_AFTER {
            return self.step_up();
        }
        None
    }

    /// Frames the renderer abandoned outright. Returns the new rung if it moved.
    ///
    /// A drop is louder than an over-budget frame — the picture visibly did not
    /// move — so it steps down immediately rather than after a streak.
    ///
    /// Unless the drop was **ours**. Superseding the session — a resize, a seek
    /// — resets the ring by construction, and the pacer then reports missing
    /// frames until the read-ahead refills. Those drops are the supersede's
    /// physics, not the machine's load, and for a while they were counted:
    /// entering fullscreen during playback stepped the ladder down within a
    /// frame and the picture stayed soft until a pause reset it — with
    /// `over_budget=0` in every telemetry line, the machine never having
    /// struggled at all. That is what [`Self::begin_grace`] exists to absorb.
    #[must_use]
    pub fn dropped(&mut self, frames: i64) -> Option<usize> {
        if frames <= 0 || self.grace > 0 {
            return None;
        }
        self.healthy = 0;
        self.over_budget = STEP_DOWN_AFTER;
        self.step_down()
    }

    /// Ignore dropped frames for the next `frames` finished frames.
    ///
    /// Called when a session is adopted mid-playback. Only drops are graced:
    /// an over-budget frame during the refill is a real measurement of a real
    /// render and still counts.
    pub fn begin_grace(&mut self, frames: u32) {
        self.grace = frames;
    }

    /// Back to the top. Returns whether anything had been given up, which is
    /// what tells a pause whether the ring is worth keeping.
    pub fn reset(&mut self) -> bool {
        let degraded = self.degraded_this_run;
        *self = Self::new();
        degraded
    }

    fn step_down(&mut self) -> Option<usize> {
        self.over_budget = 0;
        self.healthy = 0;
        if self.rung + 1 >= RUNGS.len() {
            return None;
        }
        self.rung += 1;
        self.degraded_this_run = true;
        Some(self.rung)
    }

    fn step_up(&mut self) -> Option<usize> {
        self.healthy = 0;
        self.over_budget = 0;
        if self.rung == 0 {
            return None;
        }
        self.rung -= 1;
        Some(self.rung)
    }
}

/// The label a log line uses for a rung, so the ladder is readable in the file
/// without knowing the constants.
pub fn rung_label(rung: usize) -> &'static str {
    match rung {
        0 => "full",
        1 => "three-quarter",
        _ => "half",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fullscreen bug, distilled: a supersede's own drops must not move
    /// the ladder. `over_budget=0` in every telemetry line while the picture
    /// stayed soft is how it was caught.
    #[test]
    fn drops_during_the_adopt_grace_are_construction_not_load() {
        let mut ladder = Ladder::new();
        ladder.begin_grace(30);
        assert_eq!(ladder.dropped(3), None, "the refill's drops are ours");
        assert_eq!(ladder.rung(), 0);

        // The grace is spent by finished frames, and then a drop is a drop.
        for _ in 0..30 {
            let _ = ladder.frame(false);
        }
        assert!(ladder.dropped(1).is_some(), "grace over, real drops count");
    }

    #[test]
    fn over_budget_frames_still_count_during_the_grace() {
        // The grace covers only the drops the adoption caused. A frame that
        // genuinely costs more than the budget is a real measurement of the
        // new size, and three of them must still step down.
        let mut ladder = Ladder::new();
        ladder.begin_grace(30);
        assert_eq!(ladder.frame(true), None);
        assert_eq!(ladder.frame(true), None);
        assert_eq!(
            ladder.frame(true),
            Some(1),
            "real cost steps the ladder, graced or not"
        );
    }

    #[test]
    fn the_top_rung_changes_nothing() {
        let ladder = Ladder::new();
        assert_eq!(ladder.rung(), 0);
        assert_eq!(ladder.size((1920, 1080)), (1920, 1080));
        assert_eq!(ladder.quality(88), 88);
        assert!(!ladder.degraded());
    }

    #[test]
    fn three_frames_over_budget_step_down_and_one_does_not() {
        let mut ladder = Ladder::new();
        assert_eq!(ladder.frame(true), None, "one slow frame is a keyframe");
        assert_eq!(ladder.frame(true), None);
        assert_eq!(ladder.frame(true), Some(1), "three in a row is a trend");
        assert_eq!(ladder.size((1920, 1080)), (1440, 810));
        assert_eq!(ladder.quality(88), 80);
        assert!(ladder.degraded());
    }

    #[test]
    fn a_good_frame_breaks_the_streak() {
        let mut ladder = Ladder::new();
        assert_eq!(ladder.frame(true), None);
        assert_eq!(ladder.frame(true), None);
        assert_eq!(ladder.frame(false), None);
        assert_eq!(ladder.frame(true), None, "the streak restarted");
        assert_eq!(ladder.rung(), 0);
    }

    #[test]
    fn a_dropped_frame_steps_down_at_once() {
        let mut ladder = Ladder::new();
        assert_eq!(ladder.dropped(1), Some(1));
        assert_eq!(ladder.dropped(4), Some(2));
        assert_eq!(ladder.size((1920, 1080)), (960, 540));
        assert_eq!(ladder.quality(88), 70);
        assert_eq!(
            ladder.dropped(1),
            None,
            "there is no fourth rung to fall to"
        );
        assert_eq!(ladder.rung(), RUNGS.len() - 1);
    }

    #[test]
    fn it_climbs_back_only_after_a_long_healthy_run() {
        let mut ladder = Ladder::new();
        assert_eq!(ladder.dropped(1), Some(1));
        for _ in 0..STEP_UP_AFTER - 1 {
            assert_eq!(ladder.frame(false), None, "not yet");
        }
        assert_eq!(ladder.frame(false), Some(0));
        assert_eq!(ladder.size((1920, 1080)), (1920, 1080));
        // A run that had to degrade is still remembered, so the pause after it
        // re-renders even though the ladder is back at the top.
        assert!(ladder.degraded());
    }

    #[test]
    fn one_bad_frame_during_the_climb_restarts_the_count() {
        let mut ladder = Ladder::new();
        assert_eq!(ladder.dropped(1), Some(1));
        for _ in 0..STEP_UP_AFTER - 1 {
            let _ = ladder.frame(false);
        }
        assert_eq!(ladder.frame(true), None);
        assert_eq!(ladder.frame(false), None, "and the climb starts again");
        assert_eq!(ladder.rung(), 1);
    }

    #[test]
    fn every_rung_is_even_sized_at_every_plausible_preview_size() {
        // An odd size is not a rounding detail: NV12 cannot represent it, so
        // the whole encode falls back to libjpeg-turbo at five times the cost.
        // Rung 0 hands the session's size back untouched — an odd *canvas* is
        // the session's problem and not the ladder's — so every rung below it
        // has to be even without making one up.
        for size in [(1920, 1080), (1080, 1920), (700, 394), (1234, 696), (2, 2)] {
            for rung in 0..RUNGS.len() {
                let mut ladder = Ladder::new();
                for _ in 0..rung {
                    let _ = ladder.dropped(1);
                }
                let (w, h) = ladder.size(size);
                assert_eq!(w % 2, 0, "{size:?} rung {rung} -> {w}x{h}");
                assert_eq!(h % 2, 0, "{size:?} rung {rung} -> {w}x{h}");
                assert!(w >= 2 && h >= 2, "{size:?} rung {rung} -> {w}x{h}");
                assert!(w <= size.0 && h <= size.1, "a rung never renders bigger");
            }
        }
    }

    #[test]
    fn quality_never_falls_through_the_floor() {
        let mut ladder = Ladder::new();
        let _ = ladder.dropped(1);
        let _ = ladder.dropped(1);
        assert_eq!(ladder.quality(70), MIN_QUALITY, "clamped, not 52");
        // A session that already asks for less than the floor keeps its own
        // number rather than being raised to it.
        assert_eq!(ladder.quality(55), 55);
    }

    #[test]
    fn the_pixels_each_rung_saves_are_what_the_names_say() {
        assert_eq!(RUNGS[0].pixel_fraction(), 1.0);
        assert!((RUNGS[1].pixel_fraction() - 0.5625).abs() < 1e-9);
        assert_eq!(RUNGS[2].pixel_fraction(), 0.25);
    }

    #[test]
    fn a_reset_reports_whether_anything_was_given_up() {
        let mut ladder = Ladder::new();
        assert!(!ladder.reset(), "a run at the top rung has nothing to redo");
        let _ = ladder.dropped(1);
        assert!(ladder.reset());
        assert_eq!(ladder.rung(), 0);
        assert!(!ladder.degraded());
    }
}
