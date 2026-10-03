//! An echo: a feedback delay line per channel with a low-pass in the loop, so
//! every repeat is darker than the one before, like a tape echo or a room.

use super::biquad::{Biquad, Coefficients};

#[derive(Debug, Clone, Copy)]
pub struct DelayParams {
    pub time_ms: f32,
    pub feedback: f32,
    pub mix: f32,
    pub tone_hz: f32,
}

pub struct Delay {
    lines: Vec<Vec<f32>>,
    index: usize,
    tone: Biquad,
    p: DelayParams,
}

impl Delay {
    pub fn new(p: DelayParams, rate: u32, channels: usize) -> Self {
        let length = ((p.time_ms.max(1.0) as f64 / 1_000.0) * rate as f64).round() as usize;
        Self {
            lines: vec![vec![0.0; length.max(1)]; channels.max(1)],
            index: 0,
            tone: Biquad::new(
                Coefficients::low_pass(rate as f64, p.tone_hz as f64, 0.707),
                channels.max(1),
            ),
            p,
        }
    }

    pub fn process(&mut self, buffer: &mut [f32]) {
        let channels = self.lines.len();
        let feedback = self.p.feedback.clamp(0.0, 0.95);
        let mix = self.p.mix.clamp(0.0, 1.0);
        let length = self.lines[0].len();
        for frame in buffer.chunks_exact_mut(channels) {
            for (channel, sample) in frame.iter_mut().enumerate() {
                let delayed = self.lines[channel][self.index];
                let darker = self.tone.tick(channel, delayed as f64) as f32;
                self.lines[channel][self.index] = *sample + darker * feedback;
                // The dry signal stays at full level and the echoes are added:
                // an echo that ducked the voice it repeats would sound wrong.
                *sample += delayed * mix;
            }
            self.index = (self.index + 1) % length;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_impulse_repeats_at_the_delay_time_and_decays_by_the_feedback() {
        let rate = 48_000;
        let mut buffer = vec![0.0f32; rate as usize];
        buffer[0] = 1.0;
        Delay::new(
            DelayParams {
                time_ms: 100.0,
                feedback: 0.5,
                mix: 1.0,
                tone_hz: 20_000.0,
            },
            rate,
            1,
        )
        .process(&mut buffer);
        // The tone filter smears each repeat over a few samples, so a repeat
        // is measured as the sum around its instant: the filter's DC gain is 1.
        let around = |at: usize| buffer[at - 2..at + 60].iter().sum::<f32>();
        let first = around(4_800);
        let second = around(9_600);
        assert!((first - 1.0).abs() < 0.05, "first echo {first}");
        assert!((second - 0.5).abs() < 0.05, "second echo {second}");
        // Nothing in between.
        assert!(buffer[100..4_700].iter().all(|s| s.abs() < 1e-3));
    }
}
