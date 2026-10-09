//! Offline transcription with whisper.cpp, for chukcut.
//!
//! Two ways to run it, one code path:
//!
//! - **In the editor's process, on the CPU** (feature `whisper`): the engine
//!   calls [`run::transcribe`] directly.
//! - **In the CUDA helper** (feature `cuda`, binary `chukcut-whisper-cuda`):
//!   the same [`run::transcribe`] on whisper.cpp's CUDA backend, in a process
//!   of its own. The helper links cudart and cuBLAS, so it cannot even start
//!   on a machine without them; that is why it is a separate binary and not a
//!   feature of the editor. The engine starts it, speaks [`protocol`] with it,
//!   and falls back to the CPU in its own process when the helper is missing,
//!   cannot start, finds no GPU or fails.
//!
//! Without features this crate is the protocol and the result types, with
//! serde as its only dependency, so the engine can talk to the helper without
//! building whisper.cpp at all.
//!
//! Design: `docs/decisions/0036-whisper-on-cuda-in-a-helper-process.md`.

use serde::{Deserialize, Serialize};

pub mod protocol;
#[cfg(feature = "whisper")]
pub mod run;

/// One word with its time in the audio, in microseconds from its start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: i64,
    pub end: i64,
}

/// What whisper heard.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Transcription {
    /// The language whisper used, as its code ("en"), when it said.
    pub language: Option<String>,
    pub words: Vec<Word>,
}

/// Where whisper.cpp runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// True for a GPU backend (CUDA), false for the CPU.
    pub gpu: bool,
    /// The backend: "CUDA" or "CPU".
    pub backend: String,
    /// The device's own name ("NVIDIA GeForce RTX 3060"); empty for the CPU.
    pub name: String,
}

impl Device {
    pub fn cpu() -> Self {
        Self {
            gpu: false,
            backend: "CPU".to_string(),
            name: String::new(),
        }
    }
}

impl std::fmt::Display for Device {
    /// "the GPU (NVIDIA GeForce RTX 3060, CUDA)" or "the CPU": the end of a
    /// sentence such as "Transcribing on …".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.gpu {
            return f.write_str("the CPU");
        }
        if self.name.is_empty() {
            write!(f, "the GPU ({})", self.backend)
        } else {
            write!(f, "the GPU ({}, {})", self.name, self.backend)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_reads_as_the_end_of_a_sentence() {
        assert_eq!(Device::cpu().to_string(), "the CPU");
        let gpu = Device {
            gpu: true,
            backend: "CUDA".into(),
            name: "NVIDIA GeForce RTX 3060".into(),
        };
        assert_eq!(gpu.to_string(), "the GPU (NVIDIA GeForce RTX 3060, CUDA)");
    }
}
