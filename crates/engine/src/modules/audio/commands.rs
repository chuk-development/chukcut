//! The `command` surface for audio.
//!
//! Deliberately small. Play, pause and seek are *preview* commands — the
//! playhead is one thing and it belongs to one module — and the frontend
//! should never be able to move the sound and the picture independently. What
//! is left is the mixer's own state: how loud it is, and whether there is a
//! device at all.

use std::sync::Arc;

use super::engine::{AudioEngine, AudioStatus};

/// Where the audio path stands: which device, at what rate, how much latency
/// the playhead is being corrected by, and why there is no sound when there is
/// none.
///
/// The frontend polls this when the user opens the audio settings, not per
/// frame; playback position arrives on the preview channel.
pub fn audio_status(audio: &Arc<AudioEngine>) -> Result<AudioStatus, String> {
    Ok(audio.status())
}

/// Master output volume, `0.0` to `4.0`.
///
/// Applied before the limiter, so lowering it genuinely removes clipping
/// rather than quietening an already-clipped mix.
pub fn audio_set_volume(audio: &Arc<AudioEngine>, volume: f32) -> Result<AudioStatus, String> {
    audio.set_volume(volume);
    Ok(audio.status())
}
