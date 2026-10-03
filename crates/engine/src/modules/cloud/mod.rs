//! Cloud providers the user brings their own key for.
//!
//! The shape is `docs/research/integrations.md` §7: provider *kinds* are
//! code, *accounts* are the user's data, and the editor asks for a
//! *capability*. Every call is blocking and runs off the UI thread.
//!
//! - [`registry`]: provider kinds (code) and accounts (the user's data, in
//!   `accounts.toml`).
//! - [`secrets`]: the keys, in `secrets.toml`, owner-only, never in a project
//!   file or a log.
//! - [`http`]: the one client; neutral `User-Agent`, no user identity.
//! - [`providers`]: one adapter per kind, implementing capability traits.
//! - [`provenance`]: where a generated or downloaded file came from and what
//!   its licence allows; kept beside the file and in the project.
//! - [`credits`]: the licence summary and credits file for an export.
//! - [`jobs`]: the submit, poll, download loop for queued providers (fal).
//! - [`stock`]: stock search results and their download into the cache.
//! - [`commands`]: the shell-facing API.
//!
//! The editor asks for a *capability* ("who can transcribe?"), never for a
//! vendor, so the captions panel lists every account that can do
//! [`Capability::Transcribe`] and does not know OpenAI from Groq.

pub mod audition;
pub mod commands;
pub mod credits;
pub mod http;
pub mod jobs;
pub mod place;
pub mod provenance;
pub mod providers;
pub mod registry;
pub mod secrets;
pub mod stock;

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
    /// Text in, speech out, sometimes with word timing.
    Tts,
    /// A list of voices with preview clips.
    VoiceList,
    /// A prompt in, a sound effect out.
    SoundEffects,
    /// A prompt in, a piece of music out.
    Music,
    /// Search a stock library and download from it.
    StockSearch,
    /// Media in, media out: remove background, upscale, interpolate.
    Process,
    /// Caption lines in one language in, the same lines in another out.
    Translate,
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
    /// The plan, when the provider says (ElevenLabs: `free`, `creator`, …).
    #[serde(default)]
    pub tier: Option<String>,
}

impl TestReport {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            ..Self::default()
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            ..Self::default()
        }
    }
}

// --- text to speech -----------------------------------------------------------

/// A voice an account offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// "premade", "cloned", "professional", … or empty.
    #[serde(default)]
    pub category: String,
    /// "female, young, american, narration": the labels, joined.
    #[serde(default)]
    pub description: String,
    /// A short sample to audition, when the provider has one.
    #[serde(default)]
    pub preview_url: Option<String>,
}

/// What to say, and how.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    pub voice: String,
    /// Empty uses the account's default model.
    #[serde(default)]
    pub model: String,
    /// ElevenLabs: 0..1, lower is more expressive.
    #[serde(default = "default_stability")]
    pub stability: f32,
    /// ElevenLabs: 0..1, how close to the original voice.
    #[serde(default = "default_similarity")]
    pub similarity: f32,
    /// 1.0 is normal.
    #[serde(default = "default_speed")]
    pub speed: f32,
    /// OpenAI `gpt-4o-mini-tts`: tone, accent, emotion in plain words.
    #[serde(default)]
    pub instructions: String,
    /// Ask for word timing when the provider can give it.
    #[serde(default = "yes")]
    pub with_timing: bool,
}

fn default_stability() -> f32 {
    0.5
}
fn default_similarity() -> f32 {
    0.75
}
fn default_speed() -> f32 {
    1.0
}
fn yes() -> bool {
    true
}

impl TtsRequest {
    pub fn new(text: impl Into<String>, voice: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            voice: voice.into(),
            model: String::new(),
            stability: default_stability(),
            similarity: default_similarity(),
            speed: default_speed(),
            instructions: String::new(),
            with_timing: true,
        }
    }
}

/// Audio a provider made.
#[derive(Debug, Clone, Default)]
pub struct AudioOut {
    pub bytes: Vec<u8>,
    /// `mp3`, `wav`, …
    pub extension: String,
    /// Word timing relative to the start of the audio; empty when the
    /// provider gave none.
    pub words: Vec<crate::modules::captions::TimedWord>,
    /// The provider's id for the request, for the provenance record.
    pub request_id: String,
    /// The model that made it.
    pub model: String,
    /// The account's plan at the time, when the provider says.
    pub tier: Option<String>,
}

/// Text to speech.
pub trait Tts: Send + Sync {
    fn voices(&self) -> Result<Vec<Voice>, String>;
    fn speak(&self, request: &TtsRequest) -> Result<AudioOut, String>;
}

/// A prompt for a sound effect or a piece of music.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioGenRequest {
    pub prompt: String,
    /// `None` lets the provider choose.
    pub duration_seconds: Option<f32>,
    /// Sound effects: a seamless loop.
    #[serde(default)]
    pub looping: bool,
    /// Music: no vocals.
    #[serde(default)]
    pub instrumental: bool,
    /// Sound effects: 0..1, how literally to follow the prompt.
    #[serde(default)]
    pub prompt_influence: Option<f32>,
}

/// Sound effects and music from a prompt.
pub trait AudioGen: Send + Sync {
    fn sound_effect(&self, request: &AudioGenRequest) -> Result<AudioOut, String>;
    fn music(&self, request: &AudioGenRequest) -> Result<AudioOut, String>;
}

/// Caption lines into another language, one out for one in, in order.
pub trait Translate: Send + Sync {
    fn translate(
        &self,
        lines: &[String],
        target: &str,
        source: Option<&str>,
    ) -> Result<Vec<String>, String>;
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
        _ => Err(format!("{} cannot transcribe", account.name)),
    }
}

/// The account and its key, refusing one that cannot do `capability`.
fn resolve(
    store: &CloudStore,
    account_id: &str,
    capability: Capability,
    verb: &str,
) -> Result<(Account, Option<String>), String> {
    let account = store
        .account(account_id)
        .ok_or_else(|| "that account no longer exists; pick another one".to_string())?;
    if !account.can(capability) {
        return Err(format!("{} cannot {verb}", account.name));
    }
    let key = store.key(&account.id);
    if key.is_none() && registry::descriptor(account.kind).needs_key {
        return Err(format!(
            "{} has no API key; add one in Settings, Accounts",
            account.name
        ));
    }
    Ok((account, key))
}

/// The speaker behind `account`.
pub fn speaker(store: &CloudStore, account_id: &str) -> Result<Box<dyn Tts>, String> {
    let (account, key) = resolve(store, account_id, Capability::Tts, "speak")?;
    match account.kind {
        ProviderKind::OpenaiCompatible => {
            Ok(Box::new(providers::openai_compat::OpenAiCompatible {
                account,
                key,
            }))
        }
        ProviderKind::Elevenlabs => Ok(Box::new(providers::elevenlabs::ElevenLabs::new(
            account,
            key.unwrap_or_default(),
        ))),
        _ => Err(format!("{} cannot speak", account.name)),
    }
}

/// The sound-effect and music generator behind `account`.
pub fn audio_generator(
    store: &CloudStore,
    account_id: &str,
    capability: Capability,
) -> Result<Box<dyn AudioGen>, String> {
    let (account, key) = resolve(store, account_id, capability, "generate audio")?;
    match account.kind {
        ProviderKind::Elevenlabs => Ok(Box::new(providers::elevenlabs::ElevenLabs::new(
            account,
            key.unwrap_or_default(),
        ))),
        _ => Err(format!("{} cannot generate audio", account.name)),
    }
}

/// The translator behind `account`. `chat_model` is the model an
/// OpenAI-compatible account translates with; DeepL ignores it.
pub fn translator(
    store: &CloudStore,
    account_id: &str,
    chat_model: &str,
) -> Result<Box<dyn Translate>, String> {
    let (account, key) = resolve(store, account_id, Capability::Translate, "translate")?;
    match account.kind {
        ProviderKind::Deepl => Ok(Box::new(providers::deepl::Deepl::new(
            account,
            key.unwrap_or_default(),
        ))),
        ProviderKind::OpenaiCompatible => Ok(Box::new(providers::openai_compat::ChatTranslator {
            provider: providers::openai_compat::OpenAiCompatible { account, key },
            model: chat_model.to_string(),
        })),
        _ => Err(format!("{} cannot translate", account.name)),
    }
}
