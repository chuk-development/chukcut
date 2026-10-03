//! Cloud providers the user brings their own key for.
//!
//! The registry is deliberately small: one provider kind (OpenAI-compatible)
//! and one capability (transcription) so far, in the shape
//! `docs/research/integrations.md` §7 lays out, so TTS, stock search and fal
//! can be added as more kinds and capabilities without changing what is here.
//!
//! - [`registry`]: provider kinds (code) and accounts (the user's data, in
//!   `accounts.toml`).
//! - [`secrets`]: the keys, in `secrets.toml`, owner-only, never in a project
//!   file or a log.
//! - [`http`]: the one client; neutral `User-Agent`, no user identity.
//! - [`providers`]: one adapter per kind, implementing capability traits.
//! - [`commands`]: the shell-facing API.
//!
//! The editor asks for a *capability* ("who can transcribe?"), never for a
//! vendor, so the captions panel lists every account that can do
//! [`Capability::Transcribe`] and does not know OpenAI from Groq.

pub mod commands;
pub mod http;
pub mod providers;
pub mod registry;
pub mod secrets;

use serde::{Deserialize, Serialize};

use crate::modules::captions::Transcript;
use crate::modules::project::Micros;

pub use registry::{Account, CloudStore, ProviderKind};

/// What an account can be asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Audio in, words with times out.
    Transcribe,
}

/// One piece of audio to transcribe.
#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    /// The encoded file, as uploaded.
    pub audio: Vec<u8>,
    pub file_name: String,
    pub content_type: String,
    /// ISO 639-1; `None` lets the provider detect it.
    pub language: Option<String>,
    /// `None` uses the account's default model.
    pub model: Option<String>,
    /// How long the audio is. A provider that answers with bare text gets one
    /// caption over the whole of it.
    pub duration: Micros,
}

/// Speech to timed words. Times in the answer are relative to the start of
/// the audio sent.
pub trait Transcribe: Send + Sync {
    fn transcribe(&self, request: &TranscribeRequest) -> Result<Transcript, String>;
}

/// The answer to "does this account work?".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestReport {
    pub ok: bool,
    /// One line for the UI: "Connected, 12 models" or the provider's error.
    pub message: String,
    /// Model ids, when the provider lists them.
    pub models: Vec<String>,
}

/// The transcriber behind `account`, with its key.
pub fn transcriber(store: &CloudStore, account_id: &str) -> Result<Box<dyn Transcribe>, String> {
    let account = store.account(account_id).ok_or_else(|| {
        "that account no longer exists; pick another in the captions panel".to_string()
    })?;
    if !account.can(Capability::Transcribe) {
        return Err(format!("{} cannot transcribe", account.name));
    }
    let key = store.key(&account.id);
    match account.kind {
        ProviderKind::OpenaiCompatible => {
            Ok(Box::new(providers::openai_compat::OpenAiCompatible {
                account,
                key,
            }))
        }
    }
}
