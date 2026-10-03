//! Provider kinds (compiled in) and the user's accounts (data).
//!
//! A *kind* is code: what a provider can do and which fields it needs. An
//! *account* is the user's: one configured instance of a kind, with a name, a
//! base URL and a default model. The same kind can be configured several
//! times — "OpenAI", "Groq" and "whisper.cpp server" are three
//! OpenAI-compatible accounts. Accounts live in `accounts.toml`, keys in
//! `secrets.toml` next to it, joined by the account id.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::secrets::{self, Secrets};
use super::Capability;

/// The kinds of provider chukcut can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Anything that speaks OpenAI's REST format: OpenAI, Groq, a local
    /// whisper.cpp or faster-whisper server.
    OpenaiCompatible,
    /// Voices, sound effects and music, with one `xi-api-key`.
    Elevenlabs,
    /// One key for hundreds of models behind one queue API; chukcut uses it
    /// to process clips (remove background, upscale, interpolate).
    Fal,
    /// Stock photos and videos.
    Pexels,
    /// Stock photos and videos.
    Pixabay,
    /// Sound effects under Creative Commons licences.
    Freesound,
    /// Caption translation.
    Deepl,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 7] = [
        ProviderKind::OpenaiCompatible,
        ProviderKind::Elevenlabs,
        ProviderKind::Fal,
        ProviderKind::Pexels,
        ProviderKind::Pixabay,
        ProviderKind::Freesound,
        ProviderKind::Deepl,
    ];

    /// The id used in provenance records and file names: `"elevenlabs"`.
    pub fn id(self) -> &'static str {
        match self {
            ProviderKind::OpenaiCompatible => "openai_compatible",
            ProviderKind::Elevenlabs => "elevenlabs",
            ProviderKind::Fal => "fal",
            ProviderKind::Pexels => "pexels",
            ProviderKind::Pixabay => "pixabay",
            ProviderKind::Freesound => "freesound",
            ProviderKind::Deepl => "deepl",
        }
    }
}

/// What the UI needs to offer a kind.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderDescriptor {
    pub kind: ProviderKind,
    pub name: &'static str,
    pub capabilities: &'static [Capability],
    /// Ready-made accounts: `(name, base URL, default model)`. The first one
    /// is what "Add" fills in.
    pub presets: &'static [(&'static str, &'static str, &'static str)],
    /// Where the user gets a key.
    pub key_page: &'static str,
    /// The vendor's terms for what may be done with the results.
    pub terms: &'static str,
    /// Whether the account form shows the base URL. Fixed for every vendor
    /// except the OpenAI-compatible kind; still stored, so tests and
    /// self-hosted mirrors can point an account elsewhere.
    pub custom_url: bool,
    /// Whether the form shows a model field.
    pub has_model: bool,
    /// Whether a key is required. A local OpenAI-compatible server is not.
    pub needs_key: bool,
    /// One or two sentences for the account form: what the key gives, and
    /// what the vendor's terms ask of the user.
    pub help: &'static str,
}

pub const PROVIDERS: &[ProviderDescriptor] = &[
    ProviderDescriptor {
        kind: ProviderKind::OpenaiCompatible,
        name: "OpenAI-compatible",
        capabilities: &[Capability::Transcribe, Capability::Tts, Capability::Translate],
        presets: &[
            ("OpenAI", "https://api.openai.com/v1", "whisper-1"),
            (
                "Groq",
                "https://api.groq.com/openai/v1",
                "whisper-large-v3-turbo",
            ),
        ],
        key_page: "https://platform.openai.com/api-keys",
        terms: "https://openai.com/policies/usage-policies/",
        custom_url: true,
        has_model: true,
        needs_key: false,
        help: "OpenAI, Groq or a local server such as Kokoro-FastAPI. Transcription, speech and caption translation. OpenAI asks you to disclose that a voice is AI-generated.",
    },
    ProviderDescriptor {
        kind: ProviderKind::Elevenlabs,
        name: "ElevenLabs",
        capabilities: &[
            Capability::Tts,
            Capability::VoiceList,
            Capability::SoundEffects,
            Capability::Music,
        ],
        presets: &[("ElevenLabs", "https://api.elevenlabs.io", "eleven_multilingual_v2")],
        key_page: "https://elevenlabs.io/app/settings/api-keys",
        terms: "https://elevenlabs.io/terms-of-use",
        custom_url: false,
        has_model: true,
        needs_key: true,
        help: "Voices with word timing, sound effects and music. The free plan does not allow commercial use; chukcut records the plan with every file and warns at export. A key limited to text to speech, sound effects and music, with a credit cap, is enough.",
    },
    ProviderDescriptor {
        kind: ProviderKind::Fal,
        name: "fal.ai",
        capabilities: &[Capability::Process],
        presets: &[("fal.ai", "https://queue.fal.run", "")],
        key_page: "https://fal.ai/dashboard/keys",
        terms: "https://fal.ai/terms",
        custom_url: false,
        has_model: false,
        needs_key: true,
        help: "Remove a video's background, upscale it, or smooth a slow motion. The clip is uploaded to fal.ai; the price is shown before anything runs. Each model has its own licence.",
    },
    ProviderDescriptor {
        kind: ProviderKind::Pexels,
        name: "Pexels",
        capabilities: &[Capability::StockSearch],
        presets: &[("Pexels", "https://api.pexels.com", "")],
        key_page: "https://www.pexels.com/api/new/",
        terms: "https://www.pexels.com/license/",
        custom_url: false,
        has_model: false,
        needs_key: true,
        help: "Free stock photos and videos. No credit required, but chukcut lists the photographer in the credits file. 200 searches an hour.",
    },
    ProviderDescriptor {
        kind: ProviderKind::Pixabay,
        name: "Pixabay",
        capabilities: &[Capability::StockSearch],
        presets: &[("Pixabay", "https://pixabay.com", "")],
        key_page: "https://pixabay.com/api/docs/",
        terms: "https://pixabay.com/service/license-summary/",
        custom_url: false,
        has_model: false,
        needs_key: true,
        help: "Free stock photos and videos under the Pixabay Content License. Results are cached for a day, as Pixabay asks.",
    },
    ProviderDescriptor {
        kind: ProviderKind::Freesound,
        name: "Freesound",
        capabilities: &[Capability::StockSearch],
        presets: &[("Freesound", "https://freesound.org/apiv2", "")],
        key_page: "https://freesound.org/apiv2/apply",
        terms: "https://freesound.org/help/tos_api/",
        custom_url: false,
        has_model: false,
        needs_key: true,
        help: "Sound effects under CC0 and CC BY. Non-commercial sounds are hidden unless you ask for them. Uses the high-quality previews, so the API token is enough.",
    },
    ProviderDescriptor {
        kind: ProviderKind::Deepl,
        name: "DeepL",
        capabilities: &[Capability::Translate],
        presets: &[("DeepL", "https://api.deepl.com", "")],
        key_page: "https://www.deepl.com/your-account/keys",
        terms: "https://www.deepl.com/pro-license",
        custom_url: false,
        has_model: false,
        needs_key: true,
        help: "Caption translation that keeps every line where it was. Free keys (ending in :fx) work too.",
    },
];

pub fn descriptor(kind: ProviderKind) -> &'static ProviderDescriptor {
    PROVIDERS
        .iter()
        .find(|d| d.kind == kind)
        .expect("every kind has a descriptor")
}

/// One configured provider. No secret in here, ever.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub kind: ProviderKind,
    /// What the user calls it.
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub default_model: String,
}

impl Account {
    /// A new account of `kind` with its first preset filled in.
    pub fn of_kind(kind: ProviderKind) -> Self {
        let descriptor = descriptor(kind);
        let (name, url, model) =
            descriptor
                .presets
                .first()
                .copied()
                .unwrap_or((descriptor.name, "", ""));
        Self::new(kind, name, url, model)
    }

    /// The URL the account talks to.
    pub fn url(&self, path: &str) -> String {
        super::http::join(&self.base_url, path)
    }
}

impl Account {
    pub fn new(kind: ProviderKind, name: &str, base_url: &str, default_model: &str) -> Self {
        Self {
            id: format!("acct-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            kind,
            name: name.to_string(),
            base_url: base_url.trim().to_string(),
            default_model: default_model.trim().to_string(),
        }
    }

    pub fn can(&self, capability: Capability) -> bool {
        descriptor(self.kind).capabilities.contains(&capability)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct AccountsFile {
    #[serde(default, rename = "account")]
    accounts: Vec<Account>,
}

/// The accounts and their keys, at one config directory.
#[derive(Debug, Clone)]
pub struct CloudStore {
    dir: PathBuf,
}

impl CloudStore {
    /// `~/.config/chukcut`.
    pub fn user() -> Self {
        Self::at(crate::modules::workspace::paths::config_root())
    }

    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn accounts_path(&self) -> PathBuf {
        self.dir.join("accounts.toml")
    }

    pub fn secrets_path(&self) -> PathBuf {
        self.dir.join("secrets.toml")
    }

    pub fn accounts(&self) -> Vec<Account> {
        load_accounts(&self.accounts_path())
    }

    pub fn account(&self, id: &str) -> Option<Account> {
        self.accounts().into_iter().find(|a| a.id == id)
    }

    pub fn secrets(&self) -> Secrets {
        Secrets::load(&self.secrets_path())
    }

    pub fn key(&self, id: &str) -> Option<String> {
        self.secrets().key(id)
    }

    /// Add or replace `account`. `key: None` leaves a stored key alone;
    /// `Some("")` deletes it.
    pub fn save_account(&self, account: &Account, key: Option<&str>) -> Result<(), String> {
        if account.base_url.trim().is_empty() {
            return Err("the account needs a base URL".to_string());
        }
        if !(account.base_url.starts_with("http://") || account.base_url.starts_with("https://")) {
            return Err("the base URL must start with http:// or https://".to_string());
        }
        let mut accounts = self.accounts();
        match accounts.iter_mut().find(|a| a.id == account.id) {
            Some(slot) => *slot = account.clone(),
            None => accounts.push(account.clone()),
        }
        save_accounts(&self.accounts_path(), &accounts)?;
        if let Some(key) = key {
            let mut secrets = self.secrets();
            secrets.set(&account.id, Some(key.to_string()));
            secrets.save(&self.secrets_path())?;
        }
        Ok(())
    }

    /// Delete an account and its key.
    pub fn remove_account(&self, id: &str) -> Result<(), String> {
        let mut accounts = self.accounts();
        accounts.retain(|a| a.id != id);
        save_accounts(&self.accounts_path(), &accounts)?;
        let mut secrets = self.secrets();
        if secrets.has(id) {
            secrets.set(id, None);
            secrets.save(&self.secrets_path())?;
        }
        Ok(())
    }

    /// The accounts that can do `capability`.
    pub fn with_capability(&self, capability: Capability) -> Vec<Account> {
        self.accounts()
            .into_iter()
            .filter(|a| a.can(capability))
            .collect()
    }
}

fn load_accounts(path: &Path) -> Vec<Account> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match toml::from_str::<AccountsFile>(&raw) {
        Ok(file) => file.accounts,
        Err(error) => {
            tracing::warn!(file = %path.display(), %error, "the accounts file does not parse; ignoring it");
            Vec::new()
        }
    }
}

fn save_accounts(path: &Path, accounts: &[Account]) -> Result<(), String> {
    let text = toml::to_string(&AccountsFile {
        accounts: accounts.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    // No secret in here, but it names which services the user pays for; the
    // same owner-only write costs nothing.
    secrets::write_private(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> CloudStore {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/cloud")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        CloudStore::at(dir)
    }

    #[test]
    fn an_account_and_its_key_are_stored_apart() {
        let store = store("registry-apart");
        let account = Account::new(
            ProviderKind::OpenaiCompatible,
            "Groq",
            "https://api.groq.com/openai/v1",
            "whisper-large-v3-turbo",
        );
        store.save_account(&account, Some("gsk-secret")).unwrap();

        assert_eq!(store.accounts(), vec![account.clone()]);
        assert_eq!(store.key(&account.id).as_deref(), Some("gsk-secret"));
        let accounts_text = std::fs::read_to_string(store.accounts_path()).unwrap();
        assert!(!accounts_text.contains("gsk-secret"), "{accounts_text}");
        assert!(accounts_text.contains("openai_compatible"));
        assert_eq!(
            store.with_capability(Capability::Transcribe).len(),
            1,
            "an OpenAI-compatible account can transcribe"
        );

        // Saving again without a key keeps the key.
        let renamed = Account {
            name: "Groq fast".into(),
            ..account.clone()
        };
        store.save_account(&renamed, None).unwrap();
        assert_eq!(store.key(&account.id).as_deref(), Some("gsk-secret"));
        assert_eq!(store.accounts()[0].name, "Groq fast");

        store.remove_account(&account.id).unwrap();
        assert!(store.accounts().is_empty());
        assert!(store.key(&account.id).is_none());
    }

    #[test]
    fn a_base_url_must_be_a_url() {
        let store = store("registry-url");
        let account = Account::new(ProviderKind::OpenaiCompatible, "x", "api.example.com", "m");
        assert!(store.save_account(&account, None).is_err());
    }
}
