//! Speech to text, for auto captions.
//!
//! Two transcribers, chosen in the captions panel:
//!
//! - **Cloud**: any account in the `cloud` registry that can transcribe — an
//!   OpenAI-compatible endpoint (OpenAI's `whisper-1`, Groq's
//!   `whisper-large-v3-turbo`, a self-hosted server). Long audio is cut into
//!   chunks at pauses to stay under upload limits and the answers are stitched
//!   back together on the timeline.
//! - **Local**: whisper.cpp through `whisper-rs`, offline, with the model
//!   downloaded on first use and checked against a pinned SHA-256. On an
//!   NVIDIA GPU it runs in the CUDA helper process `chukcut-whisper-cuda`
//!   ([`helper`], decision 0036); everywhere else, and whenever the helper
//!   cannot, on the CPU in this process (the `local-whisper` feature, which
//!   the app enables). Why whisper.cpp and not candle:
//!   `docs/decisions/0013-local-transcription-with-whisper-cpp.md`.
//!
//! Both hear the same thing — the timeline's own mix at 16 kHz mono, see
//! [`audio`] — and both produce a `captions::Transcript` in timeline time.

pub mod audio;
pub mod commands;
pub mod helper;
pub mod local;
pub mod models;
pub mod settings;

pub use models::LocalModel;
pub use settings::{Backend, SpeechSettings};

/// Languages offered in the panel: ISO 639-1 code and English name. Whisper
/// knows ~100; these are the ones short-form creators ask for, and the name is
/// how OpenAI reports a detected language.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("id", "Indonesian"),
    ("vi", "Vietnamese"),
    ("th", "Thai"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("no", "Norwegian"),
    ("fi", "Finnish"),
    ("cs", "Czech"),
    ("el", "Greek"),
    ("he", "Hebrew"),
    ("hu", "Hungarian"),
    ("ro", "Romanian"),
];

/// The English name of `code`, or the code itself.
pub fn language_name(code: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| *name)
        .unwrap_or(code)
}
