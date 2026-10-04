//! Music beds for the built-in templates, synthesised here.
//!
//! A template is cut to its music, and music from anywhere else comes with a
//! licence, a download and a credit line. These beds are ours: a few lines of
//! arithmetic each, deterministic, rendered to a WAV once and kept
//! (`assets::ensure_music`). They are not meant to compete with a real track
//! — the user swaps in their own — but a template should play with a beat
//! under its cuts the moment it is opened.

use serde::{Deserialize, Serialize};

pub const SAMPLE_RATE: u32 = 48_000;

/// Bump to re-render every bed.
const VERSION: u32 = 1;

/// The beds, by mood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Bed {
    /// 120 bpm: kick on the beat, hats off it, a plucked bass line.
    Pulse,
    /// 88 bpm: soft kick, rim, warm electric-piano chords.
    Chill,
    /// 70 bpm: a slow pad, a low boom every two bars, a rising swell.
    Cinematic,
    /// 128 bpm: four on the floor, claps, a bright arpeggio.
    Bright,
}

impl Bed {
    pub const ALL: [Bed; 4] = [Bed::Pulse, Bed::Chill, Bed::Cinematic, Bed::Bright];

    pub fn id(self) -> &'static str {
        match self {
            Bed::Pulse => "pulse",
            Bed::Chill => "chill",
            Bed::Cinematic => "cinematic",
            Bed::Bright => "bright",
        }
    }

    pub fn bpm(self) -> f64 {
        match self {
            Bed::Pulse => 120.0,
            Bed::Chill => 88.0,
            Bed::Cinematic => 70.0,
            Bed::Bright => 128.0,
        }
    }

    /// One beat, in microseconds: what a template's cut lengths are made of.
    pub fn beat(self) -> i64 {
        (60_000_000.0 / self.bpm()).round() as i64
    }

    fn from_id(id: &str) -> Option<Bed> {
        Bed::ALL.into_iter().find(|b| b.id() == id)
    }
}

/// `pulse-12s-v1.wav`.
pub fn file_name(bed: Bed, seconds: u32) -> String {
    format!("{}-{seconds}s-v{VERSION}.wav", bed.id())
}

/// The inverse of [`file_name`].
pub fn parse_file_name(name: &str) -> Option<(Bed, u32)> {
    let rest = name.strip_suffix(&format!("s-v{VERSION}.wav"))?;
    let (id, seconds) = rest.rsplit_once('-')?;
    Some((Bed::from_id(id)?, seconds.parse().ok()?))
}

/// A tiny deterministic noise source (xorshift32), so a bed renders to the
/// same bytes every time.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn midi(note: f64) -> f64 {
    440.0 * 2f64.powf((note - 69.0) / 12.0)
}

/// A decaying envelope `t` seconds after a hit.
fn decay(t: f64, rate: f64) -> f64 {
    if t < 0.0 {
        0.0
    } else {
        (-t * rate).exp()
    }
}

/// A soft saw: the first few harmonics only, so it never buzzes.
fn soft_saw(phase: f64) -> f64 {
    let mut sum = 0.0;
    for k in 1..=6 {
        sum += (phase * k as f64 * std::f64::consts::TAU).sin() / k as f64;
    }
    sum * 0.55
}

/// Interleaved stereo f32 samples, `seconds` long, faded in and out, peaking
/// at about −3 dBFS.
pub fn render(bed: Bed, seconds: u32) -> Vec<f32> {
    let rate = SAMPLE_RATE as f64;
    let frames = (seconds as f64 * rate) as usize;
    let beat = 60.0 / bed.bpm();
    let bar = beat * 4.0;
    // A–F–C–G, the four chords half of all pop is made of, as roots.
    let roots = [57.0, 53.0, 60.0, 55.0];
    let mut noise = Noise(0x9e37_79b9);
    let mut hat_state = 0.0f64;
    let mut pad_lp = [0.0f64; 2];
    let mut out = Vec::with_capacity(frames * 2);

    for i in 0..frames {
        let t = i as f64 / rate;
        let in_beat = t % beat;
        let beat_index = (t / beat) as i64;
        let chord = roots[((t / bar) as usize) % roots.len()];
        let white = noise.next() as f64;
        // A one-pole high pass over noise: the hats' hiss.
        let hiss = white - hat_state;
        hat_state = white * 0.35 + hat_state * 0.65;

        let (mut l, mut r);
        match bed {
            Bed::Pulse => {
                let kick_t = in_beat;
                let f = 45.0 + 80.0 * decay(kick_t, 30.0);
                let kick = (f * kick_t * std::f64::consts::TAU).sin() * decay(kick_t, 9.0);
                let off = (t + beat * 0.5) % beat;
                let hat = hiss * decay(off, 60.0) * 0.25;
                let eighth = beat * 0.5;
                let pluck_t = t % eighth;
                let note = chord - 24.0
                    + if (t / eighth) as i64 % 4 == 3 {
                        7.0
                    } else {
                        0.0
                    };
                let bass = soft_saw(midi(note) * t) * decay(pluck_t, 7.0) * 0.45;
                l = kick * 0.8 + hat + bass;
                r = kick * 0.8 + hat * 0.8 + bass;
            }
            Bed::Chill => {
                let kick_t = t % (beat * 2.0);
                let kick = (55.0 * kick_t * std::f64::consts::TAU).sin() * decay(kick_t, 10.0);
                let rim_t = (t + beat) % (beat * 2.0);
                let rim = hiss * decay(rim_t, 45.0) * 0.3;
                let chord_t = t % bar;
                let mut keys = 0.0;
                for (n, step) in [0.0, 4.0, 7.0, 11.0].iter().enumerate() {
                    let f = midi(chord + step);
                    keys += ((f * t * std::f64::consts::TAU).sin()
                        + 0.3 * (2.0 * f * t * std::f64::consts::TAU).sin())
                        * decay(chord_t - n as f64 * 0.03, 1.4);
                }
                keys *= 0.12;
                l = kick * 0.6 + rim + keys * 1.1;
                r = kick * 0.6 + rim * 0.7 + keys * 0.9;
            }
            Bed::Cinematic => {
                let two_bars = bar * 2.0;
                let boom_t = t % two_bars;
                let boom = (38.0 * boom_t * std::f64::consts::TAU).sin() * decay(boom_t, 1.8);
                let mut pad = [0.0f64; 2];
                for (side, detune) in [(0usize, 0.997), (1, 1.003)] {
                    let mut raw = 0.0;
                    for step in [0.0, 7.0, 12.0, 15.0] {
                        raw += soft_saw(midi(chord - 12.0 + step) * detune * t);
                    }
                    // A slow one-pole low pass keeps the pad dark.
                    pad_lp[side] += (raw - pad_lp[side]) * 0.02;
                    pad[side] = pad_lp[side];
                }
                let swell = 0.4 + 0.6 * ((t % two_bars) / two_bars);
                l = boom * 0.7 + pad[0] * 0.18 * swell;
                r = boom * 0.7 + pad[1] * 0.18 * swell;
            }
            Bed::Bright => {
                let kick_t = in_beat;
                let f = 50.0 + 90.0 * decay(kick_t, 35.0);
                let kick = (f * kick_t * std::f64::consts::TAU).sin() * decay(kick_t, 10.0);
                let clap = if beat_index % 2 == 1 {
                    white * decay(in_beat, 25.0) * 0.35
                } else {
                    0.0
                };
                let sixteenth = beat * 0.25;
                let step = (t / sixteenth) as usize % 4;
                let note = chord + 12.0 + [0.0, 4.0, 7.0, 12.0][step];
                let arp = ((midi(note) * t * std::f64::consts::TAU).sin()
                    + 0.25 * (midi(note) * 3.0 * t * std::f64::consts::TAU).sin())
                    * decay(t % sixteenth, 14.0)
                    * 0.2;
                l = kick * 0.75 + clap + arp * 1.2;
                r = kick * 0.75 + clap * 0.8 + arp * 0.8;
            }
        }

        // 20 ms in, 1.5 s out, so the last cut does not end on a click.
        let fade_in = (t / 0.02).min(1.0);
        let left_over = seconds as f64 - t;
        let fade_out = (left_over / 1.5).clamp(0.0, 1.0);
        let gain = fade_in * fade_out;
        l *= gain;
        r *= gain;
        out.push(l as f32);
        out.push(r as f32);
    }

    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let scale = 0.7 / peak;
        for s in &mut out {
            *s *= scale;
        }
    }
    out
}

/// 16-bit PCM WAV bytes of interleaved stereo samples.
pub fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let channels: u16 = 2;
    let bits: u16 = 16;
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    let block = channels * bits / 8;
    out.extend_from_slice(&(SAMPLE_RATE * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_round_trip() {
        for bed in Bed::ALL {
            assert_eq!(parse_file_name(&file_name(bed, 14)), Some((bed, 14)));
        }
        assert_eq!(parse_file_name("pulse-14s-v0.wav"), None);
    }

    #[test]
    fn every_bed_is_audible_bounded_and_deterministic() {
        for bed in Bed::ALL {
            let a = render(bed, 2);
            assert_eq!(a.len(), 2 * 2 * SAMPLE_RATE as usize);
            let peak = a.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((peak - 0.7).abs() < 1e-3, "{bed:?} peaks at {peak}");
            let rms = (a.iter().map(|s| s * s).sum::<f32>() / a.len() as f32).sqrt();
            assert!(rms > 0.02, "{bed:?} is nearly silent: {rms}");
            assert_eq!(a, render(bed, 2), "{bed:?} is not deterministic");
        }
    }

    #[test]
    fn the_wav_header_describes_the_data() {
        let bytes = wav_bytes(&[0.0, 0.5, -0.5, 1.0]);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            48_000
        );
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
        assert_eq!(bytes.len(), 44 + 8);
    }
}
