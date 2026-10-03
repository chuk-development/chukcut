//! Offline transcription with whisper.cpp.
//!
//! Compiled only with the `local-whisper` feature, because building
//! whisper.cpp costs a CMake run. Without it [`transcribe`] says how to get it.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::modules::captions::Transcript;

/// Whether this build carries whisper.cpp.
pub const AVAILABLE: bool = cfg!(feature = "local-whisper");

#[cfg(not(feature = "local-whisper"))]
pub fn transcribe(
    _model: &Path,
    _samples: &[f32],
    _language: Option<&str>,
    _progress: &(dyn Fn(f32) + Sync),
    _cancel: &AtomicBool,
) -> Result<Transcript, String> {
    Err("this build has no offline transcription; build with the `local-whisper` feature, or use a cloud account".to_string())
}
