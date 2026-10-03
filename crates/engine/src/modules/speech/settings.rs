//! What the captions panel remembers between sessions.
//!
//! Preferences only — which transcriber, which account, which language and
//! mode. The account's URL and key are not here: they are a `cloud` account,
//! stored once in `accounts.toml` and `secrets.toml` and shared with every
//! other feature that talks to that provider. This file names the account by
//! id and nothing more, so it holds no secret.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::models::LocalModel;
use crate::modules::captions::CaptionMode;

/// Which transcriber to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// A cloud account that can transcribe (`cloud::Capability::Transcribe`).
    #[default]
    Cloud,
    /// whisper.cpp on this machine.
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SpeechSettings {
    pub backend: Backend,
    /// The `cloud` account id used when `backend` is `Cloud`.
    pub account: Option<String>,
    /// Overrides the account's default model when set.
    pub model: Option<String>,
    pub local_model: LocalModel,
    /// ISO 639-1 code; `None` lets the transcriber detect it.
    pub language: Option<String>,
    pub mode: CaptionMode,
    pub auto_emoji: bool,
    /// Delete existing captions before adding new ones.
    pub replace: bool,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            backend: Backend::default(),
            account: None,
            model: None,
            local_model: LocalModel::Base,
            language: None,
            mode: CaptionMode::default(),
            auto_emoji: false,
            replace: true,
        }
    }
}

impl SpeechSettings {
    pub fn path() -> PathBuf {
        crate::modules::workspace::paths::config_root().join("captions.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        let Ok(raw) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_else(|error| {
            tracing::warn!(%error, "the captions settings are unreadable; using defaults");
            Self::default()
        })
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::modules::cloud::secrets::write_private(path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_a_broken_file_is_defaults() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch/speech");
        let path = dir.join("captions.json");
        let settings = SpeechSettings {
            backend: Backend::Local,
            account: Some("acct-1".into()),
            language: Some("de".into()),
            mode: CaptionMode::words(2),
            ..Default::default()
        };
        settings.save_to(&path).unwrap();
        assert_eq!(SpeechSettings::load_from(&path), settings);

        std::fs::write(&path, "{ nope").unwrap();
        assert_eq!(SpeechSettings::load_from(&path), SpeechSettings::default());
    }
}
