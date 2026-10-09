//! Offline transcription with whisper.cpp: on the NVIDIA GPU through the CUDA
//! helper when one is usable, else on the CPU in this process.
//!
//! The CPU path is compiled only with the `local-whisper` feature, because
//! building whisper.cpp costs a CMake run. The GPU path needs no feature
//! here: it is a separate binary, `chukcut-whisper-cuda` (decision 0036),
//! which `build.rs` builds beside the editor on machines with `nvcc`.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use parking_lot::Mutex;

use super::helper::{self, Attempt};
use crate::modules::captions::{TimedWord, Transcript};
use chukcut_whisper::protocol::Request;
pub use chukcut_whisper::Device;

/// Whether this build carries whisper.cpp for the CPU.
pub const AVAILABLE: bool = cfg!(feature = "local-whisper");

/// Whether this editor can transcribe offline at all: it carries whisper.cpp,
/// or the CUDA helper is installed beside it and an NVIDIA driver is loaded.
pub fn available() -> bool {
    static HELPER: OnceLock<bool> = OnceLock::new();
    AVAILABLE || *HELPER.get_or_init(|| helper::worth_trying().is_some())
}

/// The device the last transcription or probe found.
static DEVICE: Mutex<Option<Device>> = Mutex::new(None);

/// Where offline transcription runs on this machine: the GPU when the CUDA
/// helper starts and finds one, else the CPU; `None` when this editor cannot
/// transcribe offline. The first call starts the helper once to ask it
/// (a fraction of a second, longer on a cold driver); later calls answer
/// from memory, updated by every transcription. Blocking: call it off the UI
/// thread.
pub fn device() -> Option<Device> {
    if let Some(device) = DEVICE.lock().clone() {
        return Some(device);
    }
    let device = match helper::probe() {
        Ok(device) if device.gpu => device,
        Ok(_) if !AVAILABLE => return None,
        Ok(_) => Device::cpu(),
        Err(why) => {
            if helper::worth_trying().is_some() {
                tracing::info!("offline transcription runs on the CPU: the CUDA helper {why}");
            }
            if !AVAILABLE {
                return None;
            }
            Device::cpu()
        }
    };
    *DEVICE.lock() = Some(device.clone());
    Some(device)
}

/// What [`device`] knows without starting anything.
pub fn device_known() -> Option<Device> {
    DEVICE.lock().clone()
}

/// Transcribe `samples` (16 kHz mono) with the model at `model`: on the GPU
/// through the CUDA helper when it works, else on the CPU here. A helper that
/// is missing, cannot start or fails is logged and the CPU takes over, so the
/// only difference a caller sees is the device `progress` names (0..1 with
/// it).
pub fn transcribe(
    model: &Path,
    samples: &[f32],
    language: Option<&str>,
    progress: &(dyn Fn(&Device, f32) + Sync),
    cancel: &AtomicBool,
) -> Result<Transcript, String> {
    if let Some(binary) = helper::worth_trying() {
        let request = Request {
            model: model.to_path_buf(),
            language: language.map(str::to_string),
            samples: samples.len(),
        };
        let started = std::time::Instant::now();
        match helper::transcribe(&binary, request, samples, progress, cancel) {
            Attempt::Done(transcription, device) => {
                tracing::info!(
                    "transcribed {:.1} s of audio on {device} in {:.1} s",
                    samples.len() as f64 / f64::from(super::audio::RATE),
                    started.elapsed().as_secs_f64()
                );
                *DEVICE.lock() = Some(device);
                return Ok(into_transcript(transcription));
            }
            Attempt::Cancelled => return Err("cancelled".to_string()),
            Attempt::Unavailable(why) => {
                tracing::warn!("transcribing on the CPU: the CUDA helper {why}");
            }
        }
    }
    let result = on_cpu(model, samples, language, progress, cancel);
    if result.is_ok() {
        *DEVICE.lock() = Some(Device::cpu());
    }
    result
}

#[cfg(not(feature = "local-whisper"))]
fn on_cpu(
    _model: &Path,
    _samples: &[f32],
    _language: Option<&str>,
    _progress: &(dyn Fn(&Device, f32) + Sync),
    _cancel: &AtomicBool,
) -> Result<Transcript, String> {
    Err("this build has no offline transcription; build with the `local-whisper` feature, or use a cloud account".to_string())
}

#[cfg(feature = "local-whisper")]
fn on_cpu(
    model: &Path,
    samples: &[f32],
    language: Option<&str>,
    progress: &(dyn Fn(&Device, f32) + Sync),
    cancel: &AtomicBool,
) -> Result<Transcript, String> {
    let cpu = Device::cpu();
    chukcut_whisper::run::transcribe(
        model,
        samples,
        language,
        false,
        &|fraction| progress(&cpu, fraction),
        cancel,
    )
    .map(into_transcript)
}

fn into_transcript(transcription: chukcut_whisper::Transcription) -> Transcript {
    Transcript {
        language: transcription.language,
        words: transcription
            .words
            .into_iter()
            .map(|w| TimedWord {
                text: w.text,
                start: w.start,
                end: w.end,
            })
            .collect(),
        segments: Vec::new(),
    }
}
