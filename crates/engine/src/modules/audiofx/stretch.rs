//! Playing a stretch of source audio through a clip's time map: at a constant
//! speed or along a speed curve, with the pitch kept or moved.
//!
//! The map says, for every output sample frame `k` (counted from the clip's
//! start on the timeline), which source frame is heard there. Two ways to
//! follow it:
//!
//! - [`Mode::Resample`] reads the source at that position with linear
//!   interpolation: a tape played faster. The pitch moves with the speed.
//!   CapCut's "Change audio pitch" switch, and what the mixers did before
//!   this module existed.
//! - [`Mode::PreservePitch`] feeds Signalsmith Stretch, a phase vocoder, with
//!   as much source as each block of output spans. Every `process` call may
//!   take a different number of input frames per output frame, which is what
//!   lets the stretch follow a curve and not only a constant rate.
//!
//! ## Alignment
//!
//! The stretcher has an input latency `Li` and an output latency `Lo`
//! (2880 frames each at 48 kHz with the default preset). Measured against a
//! click, output stream index `m` shows the source the stretcher had been fed
//! up to `Lo` output frames earlier, less `Li`. So the driver feeds up to
//! `map(m) + Li` by stream index `m`, pre-rolls the stretcher with real source
//! from before the clip's first frame (a clip is usually trimmed, so there is
//! audio there), runs `Lo` frames past the end, and drops the first `Lo`
//! frames. `a_click_lands_where_the_map_puts_it` holds it to within a
//! millisecond from 0.5x to 2x; at 4x a click arrives about 9 ms early.

use signalsmith_stretch::Stretch;

use crate::modules::project::speed::{advance, SpeedPoint};

/// Output frames per `process` call. Small enough that a curve's speed is
/// followed closely (5 ms at 48 kHz), large enough that the per-call cost is
/// noise.
const BLOCK: usize = 256;

/// How far past either end of the clip the map is defined, in output frames:
/// more than the stretcher's output latency at any rate we render at.
const MAP_PAD: i64 = 16_384;

/// Which source frame plays at each output frame.
#[derive(Debug, Clone)]
pub enum SourceMap {
    /// `start + k * speed`.
    Linear { start: f64, speed: f64 },
    /// Positions sampled every `step` output frames from output frame
    /// `first`, linear in between and extrapolated with the edge slope.
    Table {
        first: i64,
        step: usize,
        positions: Vec<f64>,
    },
}

impl SourceMap {
    /// A constant speed from source frame `start`.
    pub fn linear(start: f64, speed: f64) -> Self {
        SourceMap::Linear { start, speed }
    }

    /// A speed curve: `points` in source microseconds, the clip's source
    /// starting at `source_start` (microseconds), `out_frames` long at
    /// `rate`.
    pub fn curve(points: &[SpeedPoint], source_start: i64, out_frames: usize, rate: u32) -> Self {
        let step = 64usize;
        let first = -MAP_PAD;
        let last = out_frames as i64 + MAP_PAD;
        let count = ((last - first) as usize).div_ceil(step) + 1;
        let per_frame = 1_000_000.0 / rate as f64;
        let positions = (0..count)
            .map(|i| {
                let k = first + (i * step) as i64;
                let us = advance(points, source_start as f64, k as f64 * per_frame);
                us / per_frame
            })
            .collect();
        SourceMap::Table {
            first,
            step,
            positions,
        }
    }

    /// The source frame (fractional, absolute) heard at output frame `k`.
    pub fn at(&self, k: f64) -> f64 {
        match self {
            SourceMap::Linear { start, speed } => start + k * speed,
            SourceMap::Table {
                first,
                step,
                positions,
            } => {
                let x = (k - *first as f64) / *step as f64;
                let n = positions.len();
                if n == 1 {
                    return positions[0];
                }
                let i = (x.floor().max(0.0) as usize).min(n - 2);
                let t = x - i as f64;
                positions[i] + (positions[i + 1] - positions[i]) * t
            }
        }
    }

    /// The speed at output frame `k`, for the stretcher's pre-roll.
    fn speed_at(&self, k: f64) -> f64 {
        (self.at(k + 1.0) - self.at(k)).max(1e-3)
    }
}

/// How to follow the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    PreservePitch,
    Resample,
}

/// Interleaved source samples covering absolute source frames
/// `[first, first + len)`; silence outside.
pub struct SourceBuffer<'a> {
    pub samples: &'a [f32],
    pub first: i64,
    pub channels: usize,
}

impl SourceBuffer<'_> {
    fn frames(&self) -> i64 {
        (self.samples.len() / self.channels.max(1)) as i64
    }

    #[inline]
    fn sample(&self, frame: i64, channel: usize) -> f32 {
        let at = frame - self.first;
        if at < 0 || at >= self.frames() {
            0.0
        } else {
            self.samples[at as usize * self.channels + channel]
        }
    }

    /// Frames `[a, b)`, zero-padded where they leave the buffer.
    fn slice(&self, a: i64, b: i64) -> Vec<f32> {
        let mut out = Vec::with_capacity(((b - a).max(0) as usize) * self.channels);
        for frame in a..b {
            for channel in 0..self.channels {
                out.push(self.sample(frame, channel));
            }
        }
        out
    }
}

/// The source frames `[lo, hi)` a render of `out_frames` through `map` reads,
/// margins included. Callers decode exactly this.
pub fn source_span(map: &SourceMap, out_frames: usize, mode: Mode, rate: u32) -> (i64, i64) {
    let ends = [
        map.at(-(MAP_PAD as f64)),
        map.at(0.0),
        map.at(out_frames as f64),
        map.at(out_frames as f64 + MAP_PAD as f64),
    ];
    let (lo, hi) = match mode {
        Mode::Resample => (ends[1], ends[2]),
        Mode::PreservePitch => (ends[0], ends[3]),
    };
    // A second of slack at 48 kHz on each side is more than any latency or
    // pre-roll the stretcher asks for.
    let slack = rate as f64 / 2.0;
    ((lo - slack).floor() as i64, (hi + slack).ceil() as i64 + 2)
}

/// Render `out_frames` of output through `map`.
pub fn stretch(
    source: &SourceBuffer<'_>,
    map: &SourceMap,
    out_frames: usize,
    mode: Mode,
    rate: u32,
) -> Vec<f32> {
    let channels = source.channels.max(1);
    match mode {
        Mode::Resample => {
            let mut out = Vec::with_capacity(out_frames * channels);
            for k in 0..out_frames {
                let position = map.at(k as f64);
                let lower = position.floor();
                let fraction = (position - lower) as f32;
                let lower = lower as i64;
                for channel in 0..channels {
                    let a = source.sample(lower, channel);
                    let b = source.sample(lower + 1, channel);
                    out.push(a + (b - a) * fraction);
                }
            }
            out
        }
        Mode::PreservePitch => preserve_pitch(source, map, out_frames, rate),
    }
}

fn preserve_pitch(
    source: &SourceBuffer<'_>,
    map: &SourceMap,
    out_frames: usize,
    rate: u32,
) -> Vec<f32> {
    let channels = source.channels.max(1);
    let mut stretch = Stretch::preset_default(channels as u32, rate);
    let li = stretch.input_latency() as f64;
    let lo = stretch.output_latency() as i64;
    // Measured, not read off the documentation: output stream index `m`
    // shows the source the stretcher had been fed up to `Lo` output frames
    // earlier, less `Li`. Feeding up to `map(m) + Li` at stream index `m`
    // therefore puts `map(k)` at stream index `k + Lo`, and the first `Lo`
    // frames are dropped. `a_click_lands_where_the_map_puts_it` holds it.
    let input_at = |m: i64| (map.at(m as f64) + li).round() as i64;

    let first = input_at(0);
    // `seek` keeps the last block-plus-interval of what it is given; twice the
    // input latency is at least that for every preset.
    let preroll = (2.0 * li).ceil() as i64 + 1;
    stretch.seek(source.slice(first - preroll, first), map.speed_at(0.0));

    let total = out_frames + lo.max(0) as usize;
    let mut out = vec![0.0f32; total * channels];
    let mut fed = first;
    let mut m = 0usize;
    while m < total {
        let m1 = (m + BLOCK).min(total);
        let until = input_at(m1 as i64).max(fed);
        let input = source.slice(fed, until);
        stretch.process(&input, &mut out[m * channels..m1 * channels]);
        fed = until;
        m = m1;
    }
    out.drain(..(lo.max(0) as usize * channels));
    out.truncate(out_frames * channels);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{dominant_frequency, rms, sine};

    const RATE: u32 = 48_000;

    fn render(
        samples: &[f32],
        channels: usize,
        map: &SourceMap,
        out: usize,
        mode: Mode,
    ) -> Vec<f32> {
        let source = SourceBuffer {
            samples,
            first: 0,
            channels,
        };
        stretch(&source, map, out, mode, RATE)
    }

    #[test]
    fn double_speed_halves_the_time_and_keeps_the_pitch() {
        let input = sine(440.0, 0.5, 4 * 48_000, 2);
        let out = render(
            &input,
            2,
            &SourceMap::linear(0.0, 2.0),
            2 * 48_000,
            Mode::PreservePitch,
        );
        assert_eq!(out.len(), 2 * 48_000 * 2);
        let f = dominant_frequency(&out[2 * 24_000..2 * 72_000], 2, RATE);
        assert!((f - 440.0).abs() < 3.0, "pitch moved to {f} Hz");
        // And it is not attenuated into nothing.
        let level = rms(&out[2 * 24_000..2 * 72_000]) / rms(&input);
        assert!((0.7..1.3).contains(&level), "level {level}");
    }

    #[test]
    fn resampling_moves_the_pitch_with_the_speed() {
        let input = sine(440.0, 0.5, 4 * 48_000, 1);
        let out = render(
            &input,
            1,
            &SourceMap::linear(0.0, 2.0),
            48_000,
            Mode::Resample,
        );
        let f = dominant_frequency(&out[12_000..36_000], 1, RATE);
        assert!((f - 880.0).abs() < 3.0, "{f} Hz");
    }

    #[test]
    fn half_speed_keeps_the_pitch_too() {
        let input = sine(330.0, 0.5, 2 * 48_000, 1);
        let out = render(
            &input,
            1,
            &SourceMap::linear(0.0, 0.5),
            3 * 48_000,
            Mode::PreservePitch,
        );
        let f = dominant_frequency(&out[48_000..120_000], 1, RATE);
        assert!((f - 330.0).abs() < 3.0, "{f} Hz");
    }

    /// Where the click lands decides whether the picture and the sound stay in
    /// sync, so it is measured, not assumed.
    #[test]
    fn a_click_lands_where_the_map_puts_it() {
        for (speed, mode) in [
            (2.0, Mode::PreservePitch),
            (0.5, Mode::PreservePitch),
            (1.5, Mode::Resample),
        ] {
            // A short 2 kHz burst one second into the source.
            let mut input = vec![0.0f32; 4 * 48_000];
            for i in 0..480 {
                input[48_000 + i] =
                    0.8 * (std::f32::consts::TAU * 2_000.0 * i as f32 / 48_000.0).sin();
            }
            let map = SourceMap::linear(0.0, speed);
            let out = render(&input, 1, &map, (3.5 * 48_000.0 / speed) as usize, mode);
            let expected = 48_000.0 / speed;
            // The centre of mass of the burst's energy.
            let (mut sum, mut weight) = (0.0f64, 0.0f64);
            for (i, s) in out.iter().enumerate() {
                let e = (*s as f64).powi(2);
                sum += i as f64 * e;
                weight += e;
            }
            let centre = sum / weight - 240.0 / speed;
            assert!(
                (centre - expected).abs() < 0.004 * 48_000.0,
                "{mode:?} at {speed}x: click at {centre}, expected {expected}"
            );
        }
    }

    #[test]
    fn a_curve_table_agrees_with_the_closed_form_map() {
        let points = [
            SpeedPoint {
                source: 0,
                speed: 1.0,
            },
            SpeedPoint {
                source: 2_000_000,
                speed: 4.0,
            },
            SpeedPoint {
                source: 4_000_000,
                speed: 0.5,
            },
        ];
        let map = SourceMap::curve(&points, 500_000, 96_000, RATE);
        for k in [0usize, 1_000, 48_000, 95_999] {
            let us = advance(&points, 500_000.0, k as f64 * 1e6 / RATE as f64);
            let exact = us * RATE as f64 / 1e6;
            assert!((map.at(k as f64) - exact).abs() < 0.05, "frame {k}");
        }
    }

    #[test]
    fn a_curve_keeps_the_pitch_while_the_speed_sweeps() {
        let points = [
            SpeedPoint {
                source: 0,
                speed: 0.5,
            },
            SpeedPoint {
                source: 3_000_000,
                speed: 3.0,
            },
        ];
        let input = sine(500.0, 0.5, 8 * 48_000, 1);
        let map = SourceMap::curve(&points, 0, 3 * 48_000, RATE);
        let out = render(&input, 1, &map, 3 * 48_000, Mode::PreservePitch);
        for window in [24_000..48_000, 96_000..120_000] {
            let f = dominant_frequency(&out[window.clone()], 1, RATE);
            assert!((f - 500.0).abs() < 6.0, "{f} Hz in {window:?}");
        }
    }
}

#[cfg(test)]
mod throughput {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::sine;

    /// How much faster than real time a render runs. Not a gate: the number
    /// quoted in decision 0021. `cargo test -- --ignored throughput --nocapture`.
    #[test]
    #[ignore]
    fn throughput() {
        let input = sine(440.0, 0.5, 120 * 48_000, 2);
        let source = SourceBuffer {
            samples: &input,
            first: 0,
            channels: 2,
        };
        for speed in [0.5, 2.0] {
            let out_frames = (60.0 * 48_000.0) as usize;
            let started = std::time::Instant::now();
            let _ = stretch(
                &source,
                &SourceMap::linear(0.0, speed),
                out_frames,
                Mode::PreservePitch,
                48_000,
            );
            let seconds = started.elapsed().as_secs_f64();
            eprintln!(
                "60 s of output at {speed}x: {seconds:.2} s, {:.0}x real time",
                60.0 / seconds
            );
        }
        let mut buffer = input[..60 * 48_000 * 2].to_vec();
        for kind in [
            "eq5",
            "compressor",
            "reverb",
            "delay",
            "pitch",
            "voice_robot",
        ] {
            let mut effect = crate::modules::audiofx::AudioEffect::new(kind);
            if kind == "pitch" {
                effect.params.insert("semitones".into(), 3.0);
            }
            if kind == "eq5" {
                // At its defaults every band is flat and skipped.
                effect.params.insert("b3_gain".into(), 6.0);
            }
            let started = std::time::Instant::now();
            crate::modules::audiofx::dsp::apply(&mut buffer, 2, 48_000, &effect);
            let seconds = started.elapsed().as_secs_f64();
            eprintln!(
                "60 s through {kind}: {seconds:.2} s, {:.0}x real time",
                60.0 / seconds
            );
        }
    }
}
