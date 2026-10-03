//! Transport beyond play and pause: J/K/L shuttle, an in/out range that can
//! loop, frame-exact stepping, and a moment of sound under the playhead while
//! it is dragged.
//!
//! Normal-speed playback stays the clock's and the audio engine's business,
//! exactly as before. Every other speed is *driven*: the clock is paused and
//! the playhead is moved on each tick by the wall time since the shuttle
//! started, times the rate. The audio engine has no rate control, so a driven
//! playhead is silent — the same as in most editors at 4×, and the honest
//! answer for reverse, which would otherwise need a backwards decoder.

use std::time::{Duration, Instant};

use chukcut_engine::modules::preview::clock::frame_time;
use gpui::KeyBinding;

use super::*;

actions!(
    chukcut,
    [
        ShuttleForward,
        ShuttleBack,
        ShuttleStop,
        MarkIn,
        MarkOut,
        ClearInOut,
        ToggleLoop,
        StepBack10,
        StepForward10
    ]
);

/// How long a scrub keeps sounding after the last move. Long enough to
/// recognise a word, short enough that a drag does not smear into playback.
const SCRUB_SOUND: Duration = Duration::from_millis(90);

/// The fastest shuttle speed, either way.
const MAX_RATE: i32 = 8;

pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    // Plain keys stay out of text fields, like the editor's own.
    const TYPING_OFF: Option<&str> = Some("!Input");
    vec![
        KeyBinding::new("l", ShuttleForward, TYPING_OFF),
        KeyBinding::new("j", ShuttleBack, TYPING_OFF),
        KeyBinding::new("k", ShuttleStop, TYPING_OFF),
        KeyBinding::new("i", MarkIn, TYPING_OFF),
        KeyBinding::new("o", MarkOut, TYPING_OFF),
        KeyBinding::new("alt-x", ClearInOut, TYPING_OFF),
        KeyBinding::new("ctrl-l", ToggleLoop, None),
        KeyBinding::new("shift-left", StepBack10, TYPING_OFF),
        KeyBinding::new("shift-right", StepForward10, TYPING_OFF),
    ]
}

#[derive(Debug, Default)]
pub(crate) struct PlaybackState {
    /// Shuttle speed in multiples of normal: 0 is stopped, 1 is ordinary
    /// playback on the clock, anything else is driven.
    rate: i32,
    /// Where and when the driven playhead started, the anchor it is derived
    /// from — never accumulated, so it cannot drift.
    anchor: Option<(Instant, Micros)>,
    pub(crate) mark_in: Option<Micros>,
    pub(crate) mark_out: Option<Micros>,
    pub(crate) looping: bool,
    /// When a scrub's sound should stop.
    scrub_until: Option<Instant>,
}

impl PlaybackState {
    pub(crate) fn driven(&self) -> bool {
        self.rate != 0 && self.rate != 1
    }
}

/// The next shuttle speed after pressing L (`forward`) or J.
fn next_rate(rate: i32, forward: bool) -> i32 {
    match (forward, rate) {
        (true, r) if r <= 0 => 1,
        (true, r) => (r * 2).min(MAX_RATE),
        (false, r) if r >= 0 => -1,
        (false, r) => (r * 2).max(-MAX_RATE),
    }
}

/// The range playback loops over: in to out, or the open side to the end of
/// the timeline. `None` when the marks are inverted or empty.
fn loop_range(
    mark_in: Option<Micros>,
    mark_out: Option<Micros>,
    duration: Micros,
) -> Option<(Micros, Micros)> {
    let start = mark_in.unwrap_or(0).max(0);
    let end = mark_out.unwrap_or(duration).min(duration);
    (end > start).then_some((start, end))
}

impl Editor {
    /// The in/out range for anything that wants to draw or use it — the
    /// timeline ruler, the export dialog.
    pub(crate) fn play_range(&self) -> (Option<Micros>, Option<Micros>) {
        (self.shell.playback.mark_in, self.shell.playback.mark_out)
    }

    /// Called at the top of every tick.
    pub(crate) fn tick_playback(&mut self) {
        let now = Instant::now();
        let playback = &mut self.shell.playback;

        if let Some(until) = playback.scrub_until {
            if now >= until {
                playback.scrub_until = None;
                if !self.clock.is_playing() {
                    self.audio.pause();
                }
            }
        }

        let duration = self.project.duration();
        let range = playback
            .looping
            .then(|| loop_range(playback.mark_in, playback.mark_out, duration))
            .flatten();

        if playback.driven() {
            let Some((started, from)) = playback.anchor else {
                return;
            };
            let elapsed = now.duration_since(started).as_micros() as i64;
            let mut to = from + elapsed * playback.rate as i64;
            let (low, high) = range.unwrap_or((0, duration));
            if to < low || to > high {
                if range.is_some() {
                    // Wrap, and re-anchor so the next lap is measured afresh.
                    to = if playback.rate > 0 { low } else { high };
                    playback.anchor = Some((now, to));
                } else {
                    to = to.clamp(low, high);
                    playback.rate = 0;
                    playback.anchor = None;
                }
            }
            self.clock.seek(to);
            return;
        }

        if self.clock.is_playing() {
            if let Some((start, end)) = range {
                if self.clock.position() >= end {
                    self.clock.seek(start);
                    self.audio.seek(start);
                }
            }
        }
    }

    /// Ordinary playback is starting: honour the loop range.
    pub(crate) fn before_play(&mut self) {
        let playback = &mut self.shell.playback;
        playback.rate = 1;
        playback.anchor = None;
        playback.scrub_until = None;
        if !playback.looping {
            return;
        }
        if let Some((start, end)) =
            loop_range(playback.mark_in, playback.mark_out, self.project.duration())
        {
            let at = self.clock.position();
            if at < start || at >= end {
                self.clock.seek(start);
            }
        }
    }

    /// Playback stopped, whichever way it was going.
    pub(crate) fn after_pause(&mut self) {
        self.shell.playback.rate = 0;
        self.shell.playback.anchor = None;
    }

    /// A seek while paused sounds the audio under the playhead briefly.
    pub(crate) fn scrub_sound(&mut self, at: Micros) {
        if self.clock.is_playing()
            || self.shell.playback.driven()
            || !self.shell.settings.audio_scrubbing
        {
            return;
        }
        self.audio.play(at);
        self.shell.playback.scrub_until = Some(Instant::now() + SCRUB_SOUND);
    }

    fn shuttle(&mut self, forward: bool, cx: &mut Context<Self>) {
        let rate = next_rate(self.shell.playback.rate, forward);
        let at = self.clock.position();
        if rate == 1 {
            self.play();
        } else {
            // Driven: the clock stands still and the tick moves it.
            self.pause();
            self.audio.pause();
            self.shell.playback.rate = rate;
            self.shell.playback.anchor = Some((Instant::now(), at));
            self.status = Some(format!("Shuttle {rate}\u{d7}").into());
        }
        cx.notify();
    }

    /// Move the playhead by whole frames, from the frame it is on. Never by
    /// adding a rounded frame duration: at 30 fps that loses a microsecond a
    /// frame and lands on the wrong frame after a few hundred steps.
    pub(crate) fn step_frames(&mut self, frames: i64) {
        let fps = self.project.fps;
        let frame = frame_at(self.clock.position(), fps);
        self.pause();
        self.seek(frame_time((frame + frames).max(0), fps));
    }

    fn set_mark(&mut self, out: bool, cx: &mut Context<Self>) {
        let at = self.clock.position();
        let playback = &mut self.shell.playback;
        if out {
            playback.mark_out = Some(at);
            if playback.mark_in.is_some_and(|start| start >= at) {
                playback.mark_in = None;
            }
        } else {
            playback.mark_in = Some(at);
            if playback.mark_out.is_some_and(|end| end <= at) {
                playback.mark_out = None;
            }
        }
        self.status = Some(self.range_label().into());
        cx.notify();
    }

    fn range_label(&self) -> String {
        let fps = self.project.fps;
        let (mark_in, mark_out) = self.play_range();
        let side = |mark: Option<Micros>| match mark {
            Some(time) => timecode(time, fps),
            None => "\u{2013}".into(),
        };
        format!(
            "In {} \u{b7} Out {}{}",
            side(mark_in),
            side(mark_out),
            if self.shell.playback.looping {
                " \u{b7} loop on"
            } else {
                ""
            }
        )
    }

    /// The transport actions, on the editor's root element.
    pub(crate) fn playback_actions(&self, root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        root.on_action(cx.listener(|this, _: &ShuttleForward, _, cx| this.shuttle(true, cx)))
            .on_action(cx.listener(|this, _: &ShuttleBack, _, cx| this.shuttle(false, cx)))
            .on_action(cx.listener(|this, _: &ShuttleStop, _, cx| {
                this.pause();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &MarkIn, _, cx| this.set_mark(false, cx)))
            .on_action(cx.listener(|this, _: &MarkOut, _, cx| this.set_mark(true, cx)))
            .on_action(cx.listener(|this, _: &ClearInOut, _, cx| {
                this.shell.playback.mark_in = None;
                this.shell.playback.mark_out = None;
                this.status = Some("In and out cleared".into());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleLoop, _, cx| {
                this.shell.playback.looping = !this.shell.playback.looping;
                this.status = Some(this.range_label().into());
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &StepBack10, _, cx| {
                this.step_frames(-10);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &StepForward10, _, cx| {
                this.step_frames(10);
                cx.notify();
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_and_j_double_up_to_the_limit_and_turn_round_through_one() {
        let mut rate = 0;
        let mut seen = Vec::new();
        for _ in 0..5 {
            rate = next_rate(rate, true);
            seen.push(rate);
        }
        assert_eq!(seen, [1, 2, 4, 8, 8]);
        assert_eq!(next_rate(8, false), -1, "J while going forward reverses");
        assert_eq!(next_rate(-1, false), -2);
        assert_eq!(next_rate(-8, false), -8);
        assert_eq!(next_rate(-4, true), 1, "L while reversing plays");
    }

    #[test]
    fn the_loop_range_falls_back_to_the_timeline_and_refuses_an_empty_one() {
        assert_eq!(loop_range(None, None, 10), Some((0, 10)));
        assert_eq!(loop_range(Some(2), None, 10), Some((2, 10)));
        assert_eq!(loop_range(None, Some(4), 10), Some((0, 4)));
        assert_eq!(loop_range(Some(5), Some(5), 10), None);
        assert_eq!(loop_range(Some(6), Some(20), 10), Some((6, 10)));
    }

    #[test]
    fn stepping_by_frames_lands_on_frame_starts_without_drift() {
        // The old step added (1e6 / fps) as an integer; after 300 steps at
        // 30 fps that is 100 µs short of frame 300.
        let fps = 30.0;
        let mut time = 0;
        for _ in 0..300 {
            time = frame_time(frame_at(time, fps) + 1, fps);
        }
        assert_eq!(frame_at(time, fps), 300);
        assert_eq!(time, 10_000_000);
    }
}
