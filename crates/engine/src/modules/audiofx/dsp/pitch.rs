//! Pitch shift without a change of speed, through Signalsmith Stretch.
//!
//! The whole buffer goes through `exact`, which feeds the stretcher with the
//! buffer's own head as pre-roll and drops the latency at both ends, so the
//! output lines up with the input sample for sample. Decision 0020.

use signalsmith_stretch::Stretch;

#[derive(Debug, Clone, Copy)]
pub struct PitchParams {
    pub semitones: f32,
    /// Keep the voice's resonances (formants) where they are, so a shifted
    /// voice sounds like the same person singing higher rather than a
    /// smaller one. Off is the classic chipmunk.
    pub keep_formants: bool,
    /// Move the formants by this many semitones on top of whatever
    /// `keep_formants` does. The deep voice moves them down a little less than
    /// the pitch, which reads as "bigger", not "slowed down".
    pub formant_semitones: f32,
}

/// Shift `buffer` (interleaved, `channels` wide) in place.
pub fn shift(buffer: &mut [f32], channels: usize, rate: u32, p: PitchParams) {
    if p.semitones.abs() < 1e-3 && p.formant_semitones.abs() < 1e-3 {
        return;
    }
    let channels = channels.max(1);
    let frames = buffer.len() / channels;
    let mut stretch = Stretch::preset_default(channels as u32, rate);
    stretch.set_transpose_factor_semitones(p.semitones, None);
    if p.keep_formants || p.formant_semitones.abs() > 1e-3 {
        stretch.set_formant_factor_semitones(p.formant_semitones, p.keep_formants);
    }
    let mut output = vec![0.0f32; frames * channels];
    if stretch.exact(&*buffer, &mut output) {
        buffer.copy_from_slice(&output);
    }
    // `exact` refuses a buffer shorter than twice the latency (a few tenths
    // of a second). A clip that short is left unshifted rather than garbled.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::audiofx::dsp::test_signals::{dominant_frequency, sine};

    #[test]
    fn twelve_semitones_up_doubles_the_frequency_and_keeps_the_length() {
        let mut buffer = sine(220.0, 0.5, 48_000, 2);
        let length = buffer.len();
        shift(
            &mut buffer,
            2,
            48_000,
            PitchParams {
                semitones: 12.0,
                keep_formants: false,
                formant_semitones: 0.0,
            },
        );
        assert_eq!(buffer.len(), length);
        let f = dominant_frequency(&buffer[2 * 12_000..2 * 36_000], 2, 48_000);
        assert!((f - 440.0).abs() < 8.0, "{f} Hz");
    }

    #[test]
    fn five_semitones_down_lands_on_the_fourth_below() {
        let mut buffer = sine(440.0, 0.5, 48_000, 1);
        shift(
            &mut buffer,
            1,
            48_000,
            PitchParams {
                semitones: -5.0,
                keep_formants: true,
                formant_semitones: 0.0,
            },
        );
        let expected = 440.0 * 2f64.powf(-5.0 / 12.0);
        let f = dominant_frequency(&buffer[12_000..36_000], 1, 48_000);
        assert!((f - expected).abs() < 6.0, "{f} Hz, expected {expected}");
    }
}
