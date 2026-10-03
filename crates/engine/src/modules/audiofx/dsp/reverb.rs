//! Freeverb: Jezar at Dreampoint's public-domain Schroeder–Moorer reverb.
//!
//! Eight damped feedback combs in parallel, four allpasses in series, per
//! channel; the right channel's delays are 23 samples longer, which is what
//! makes the tail wide. The tunings are Jezar's, given at 44.1 kHz and scaled
//! to the render's rate so a room sounds the same size in a 48 kHz preview and
//! a 44.1 kHz export.

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const SCALE_DAMP: f32 = 0.4;
/// Jezar's wet scale times his default wet level (3 × 1/3): a mix of 1 is a
/// tail about as loud as the dry signal.
const WET_SCALE: f32 = 1.0;

struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
}

impl Comb {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buffer[self.index];
        self.store = output * (1.0 - damp) + self.store * damp;
        self.buffer[self.index] = input + self.store * feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }
}

struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

impl Allpass {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length.max(1)],
            index: 0,
        }
    }

    #[inline]
    fn tick(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.index];
        let output = delayed - input;
        self.buffer[self.index] = input + delayed * 0.5;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }
}

struct Side {
    combs: Vec<Comb>,
    allpasses: Vec<Allpass>,
}

impl Side {
    fn new(rate: u32, spread: usize) -> Self {
        let scale = |n: usize| ((n + spread) as f64 * rate as f64 / 44_100.0).round() as usize;
        Self {
            combs: COMBS.iter().map(|&n| Comb::new(scale(n))).collect(),
            allpasses: ALLPASSES.iter().map(|&n| Allpass::new(scale(n))).collect(),
        }
    }

    #[inline]
    fn tick(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let mut out = 0.0;
        for comb in &mut self.combs {
            out += comb.tick(input, feedback, damp);
        }
        for allpass in &mut self.allpasses {
            out = allpass.tick(out);
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ReverbParams {
    pub room: f32,
    pub damping: f32,
    pub width: f32,
    pub mix: f32,
}

pub struct Reverb {
    left: Side,
    right: Side,
    p: ReverbParams,
    channels: usize,
}

impl Reverb {
    pub fn new(p: ReverbParams, rate: u32, channels: usize) -> Self {
        Self {
            left: Side::new(rate, 0),
            right: Side::new(rate, STEREO_SPREAD),
            p,
            channels: channels.max(1),
        }
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        let feedback = self.p.room.clamp(0.0, 1.0) * SCALE_ROOM + OFFSET_ROOM;
        let damp = self.p.damping.clamp(0.0, 1.0) * SCALE_DAMP;
        let width = self.p.width.clamp(0.0, 1.0);
        let mix = self.p.mix.clamp(0.0, 1.0);
        let wet = mix * WET_SCALE;
        let wet1 = wet * (width / 2.0 + 0.5);
        let wet2 = wet * ((1.0 - width) / 2.0);
        let dry = 1.0 - mix;
        let channels = self.channels;
        for frame in buffer.chunks_exact_mut(channels) {
            let l = frame[0];
            let r = if channels > 1 { frame[1] } else { l };
            let input = (l + r) * FIXED_GAIN;
            let out_l = self.left.tick(input, feedback, damp);
            let out_r = self.right.tick(input, feedback, damp);
            frame[0] = out_l * wet1 + out_r * wet2 + l * dry;
            if channels > 1 {
                frame[1] = out_r * wet1 + out_l * wet2 + r * dry;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn impulse(frames: usize) -> Vec<f32> {
        let mut v = vec![0.0; frames * 2];
        v[0] = 1.0;
        v[1] = 1.0;
        v
    }

    fn energy(v: &[f32]) -> f64 {
        v.iter().map(|s| (*s as f64).powi(2)).sum()
    }

    #[test]
    fn an_impulse_grows_a_decaying_tail_and_a_bigger_room_rings_longer() {
        let tail = |room: f32| {
            let mut buffer = impulse(96_000);
            Reverb::new(
                ReverbParams {
                    room,
                    damping: 0.5,
                    width: 1.0,
                    mix: 1.0,
                },
                48_000,
                2,
            )
            .process(&mut buffer);
            buffer
        };
        let small = tail(0.2);
        let large = tail(0.9);
        // Something arrives after the first comb's delay, and it decays.
        let early = energy(&large[2 * 4_800..2 * 24_000]);
        let late = energy(&large[2 * 72_000..]);
        assert!(early > 0.0 && late < early, "{early} then {late}");
        // A larger room keeps more energy in its last second.
        assert!(energy(&large[2 * 48_000..]) > 4.0 * energy(&small[2 * 48_000..]));
    }

    #[test]
    fn a_mix_of_zero_is_the_dry_signal() {
        let input: Vec<f32> = (0..9_600).map(|i| ((i as f32) * 0.01).sin()).collect();
        let mut output = input.clone();
        Reverb::new(
            ReverbParams {
                room: 0.8,
                damping: 0.2,
                width: 1.0,
                mix: 0.0,
            },
            48_000,
            2,
        )
        .process(&mut output);
        assert_eq!(input, output);
    }

    #[test]
    fn the_room_is_the_same_size_in_seconds_at_any_rate() {
        let at = |rate: u32| Side::new(rate, 0).combs[0].buffer.len() as f64 / rate as f64;
        assert!((at(44_100) - at(48_000)).abs() < 1e-4);
    }
}
