//! Commands for cloud accounts and what they can do.
//!
//! Keys go in and never come out: there is no command that returns one, only
//! [`AccountView::key_hint`], the last four characters.
//!
//! Every command that talks to a provider blocks; the shells call them off
//! the UI thread. None of them edits the document except
//! [`cloud_translate_captions`], which goes through the history like any
//! caption edit: a generated file reaches the timeline the way any file does,
//! through `project_import_media` and an ordinary `EditCommand`, and the
//! import copies its licence record into the project.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use super::credits::{self, LicenceSummary};
use super::jobs::JobEvent;
use super::provenance::{self, Commercial, Licence, Origin, OriginKind};
use super::providers::elevenlabs::ElevenLabs;
use super::providers::fal::{self, Estimate, Fal, FalAction, MediaFacts};
use super::providers::openai_compat::OpenAiCompatible;
use super::providers::{deepl::Deepl, freesound::Freesound, pexels::Pexels, pixabay::Pixabay};
use super::registry::{self, Account, CloudStore, ProviderDescriptor, ProviderKind};
use super::stock::{self, StockHit, StockPage, StockQuery, StockSearch};
use super::{secrets, AudioGenRequest, AudioOut, Capability, TestReport, TtsRequest, Voice};
use crate::modules::captions::edit::{self as caption_edit, PlaceOptions};
use crate::modules::captions::{CaptionStyle, Cue, TimedWord};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::modules::workspace::paths;
use crate::state::AppState;

/// An account as the settings UI shows it.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    #[serde(flatten)]
    pub account: Account,
    pub has_key: bool,
    /// `••••abcd`, or empty when there is no key.
    pub key_hint: String,
}

/// Every provider kind chukcut knows.
pub fn cloud_providers() -> &'static [ProviderDescriptor] {
    registry::PROVIDERS
}

/// The configured accounts, optionally only those that can do `capability`.
pub fn cloud_accounts(store: &CloudStore, capability: Option<Capability>) -> Vec<AccountView> {
    let secrets = store.secrets();
    store
        .accounts()
        .into_iter()
        .filter(|a| capability.is_none_or(|c| a.can(c)))
        .map(|account| {
            let key = secrets.key(&account.id);
            AccountView {
                has_key: key.is_some(),
                key_hint: key.as_deref().map(secrets::masked).unwrap_or_default(),
                account,
            }
        })
        .collect()
}

/// Add or update an account. `key: None` keeps the stored key, `Some("")`
/// deletes it. Returns the account id.
pub fn cloud_account_set(
    store: &CloudStore,
    account: Account,
    key: Option<&str>,
) -> Result<String, String> {
    store.save_account(&account, key)?;
    Ok(account.id)
}

/// A new OpenAI-compatible account from one of the presets, or a blank one.
pub fn cloud_account_new(name: &str, base_url: &str, model: &str) -> Account {
    Account::new(ProviderKind::OpenaiCompatible, name, base_url, model)
}

/// A new account of any kind, with its vendor's URL and default model.
pub fn cloud_account_new_of(kind: ProviderKind) -> Account {
    Account::of_kind(kind)
}

pub fn cloud_account_remove(store: &CloudStore, account_id: &str) -> Result<(), String> {
    store.remove_account(account_id)
}

/// One cheap call that proves the URL and key work. Blocking; run it off the
/// UI thread.
pub fn cloud_account_test(store: &CloudStore, account_id: &str) -> TestReport {
    let Some(account) = store.account(account_id) else {
        return TestReport::failed("that account no longer exists");
    };
    let key = store.key(&account.id);
    if key.is_none() && registry::descriptor(account.kind).needs_key {
        return TestReport::failed("there is no API key yet");
    }
    let k = key.clone().unwrap_or_default();
    match account.kind {
        ProviderKind::OpenaiCompatible => OpenAiCompatible { account, key }.test(),
        ProviderKind::Elevenlabs => ElevenLabs::new(account, k).test(),
        ProviderKind::Fal => Fal::new(account, k).test(),
        ProviderKind::Pexels => Pexels::new(account, k).test(),
        ProviderKind::Pixabay => Pixabay::new(account, k).test(),
        ProviderKind::Freesound => Freesound::new(account, k).test(),
        ProviderKind::Deepl => Deepl::new(account, k).test(),
    }
}

// --- generated audio ------------------------------------------------------------

/// A file a provider made, on disk with its `asset.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Generated {
    pub path: PathBuf,
    pub origin: Origin,
    /// Word timing from the provider, relative to the file's start. Empty
    /// when it gave none.
    pub words: Vec<TimedWord>,
}

/// The voices `account` offers.
pub fn cloud_voices(store: &CloudStore, account_id: &str) -> Result<Vec<Voice>, String> {
    super::speaker(store, account_id)?.voices()
}

/// What an ElevenLabs plan allows, from the tier its subscription reports.
fn elevenlabs_licence(tier: Option<&str>) -> Licence {
    let (commercial, note) = match tier {
        Some("free") => (
            Commercial::No,
            "ElevenLabs free plan: no commercial licence".to_string(),
        ),
        Some(tier) => (
            Commercial::Yes,
            format!("ElevenLabs {tier} plan: commercial licence"),
        ),
        None => (
            Commercial::Unknown,
            "The ElevenLabs plan could not be read; the free plan allows no commercial use"
                .to_string(),
        ),
    };
    Licence {
        id: "LicenseRef-ElevenLabs".into(),
        name: format!("ElevenLabs {} plan", tier.unwrap_or("unknown")),
        url: "https://elevenlabs.io/terms-of-use".into(),
        commercial,
        note,
        ..Licence::default()
    }
}

fn openai_licence(is_openai: bool) -> Licence {
    if is_openai {
        Licence {
            id: "LicenseRef-OpenAI".into(),
            name: "OpenAI terms".into(),
            url: "https://openai.com/policies/usage-policies/".into(),
            commercial: Commercial::Yes,
            note: "OpenAI asks you to disclose that the voice is AI-generated".into(),
            ..Licence::default()
        }
    } else {
        Licence {
            id: "LicenseRef-OpenAI-compatible".into(),
            name: "The server's model licence".into(),
            commercial: Commercial::Unknown,
            note: "Check the licence of the model your server runs".into(),
            ..Licence::default()
        }
    }
}

/// Write `out` to a new directory under `root` with its record.
fn save_generated(
    root: &Path,
    provider: &str,
    stem: &str,
    out: AudioOut,
    mut origin: Origin,
) -> Result<Generated, String> {
    origin.model = out.model.clone();
    origin.request_id = out.request_id.clone();
    origin.account_tier = out.tier.clone();
    let dir = provenance::asset_dir(root, provider);
    let file = format!("{}.{}", provenance::slug(stem), out.extension);
    let path = provenance::write_asset(&dir, &file, &out.bytes, &origin)?;
    let origin = provenance::read_sidecar(&path).unwrap_or(origin);
    Ok(Generated {
        path,
        origin,
        words: out.words,
    })
}

/// Where generated files go: user data, never the cache.
pub fn generated_root() -> PathBuf {
    paths::data_root().join("generated")
}

/// Speak `request` with `account`; the file goes under `root`
/// ([`generated_root`] in the app).
pub fn cloud_tts(
    store: &CloudStore,
    account_id: &str,
    request: &TtsRequest,
    root: &Path,
) -> Result<Generated, String> {
    let account = store
        .account(account_id)
        .ok_or("that account no longer exists")?;
    let out = super::speaker(store, account_id)?.speak(request)?;
    let (provider, licence, endpoint) = match account.kind {
        ProviderKind::Elevenlabs => (
            "elevenlabs",
            elevenlabs_licence(out.tier.as_deref()),
            "/v1/text-to-speech",
        ),
        _ => {
            let is_openai = account.base_url.contains("api.openai.com");
            (
                if is_openai {
                    "openai"
                } else {
                    "openai_compatible"
                },
                openai_licence(is_openai),
                "/audio/speech",
            )
        }
    };
    let words: String = request
        .text
        .split_whitespace()
        .take(6)
        .collect::<Vec<_>>()
        .join(" ");
    let origin = Origin {
        title: format!("Voice: {words}"),
        endpoint: endpoint.into(),
        prompt: request.text.clone(),
        voice: request.voice.clone(),
        licence,
        ..Origin::new(OriginKind::Generated, provider)
    };
    save_generated(root, provider, &format!("voice-{words}"), out, origin)
}

/// A sound effect (`Capability::SoundEffects`) or a piece of music
/// (`Capability::Music`) from a prompt.
pub fn cloud_generate_audio(
    store: &CloudStore,
    account_id: &str,
    capability: Capability,
    request: &AudioGenRequest,
    root: &Path,
) -> Result<Generated, String> {
    let generator = super::audio_generator(store, account_id, capability)?;
    let (out, endpoint, noun) = match capability {
        Capability::Music => (generator.music(request)?, "/v1/music", "music"),
        _ => (
            generator.sound_effect(request)?,
            "/v1/sound-generation",
            "sfx",
        ),
    };
    let mut licence = elevenlabs_licence(out.tier.as_deref());
    if capability == Capability::Music {
        licence
            .note
            .push_str(". Music: commercial use from the Starter plan up");
    }
    let origin = Origin {
        title: request.prompt.trim().to_string(),
        endpoint: endpoint.into(),
        prompt: request.prompt.clone(),
        licence,
        ..Origin::new(OriginKind::Generated, "elevenlabs")
    };
    save_generated(
        root,
        "elevenlabs",
        &format!("{noun}-{}", request.prompt),
        out,
        origin,
    )
}

// --- stock ------------------------------------------------------------------------

fn stock_searcher(store: &CloudStore, account_id: &str) -> Result<Box<dyn StockSearch>, String> {
    let account = store
        .account(account_id)
        .ok_or("that account no longer exists")?;
    if !account.can(Capability::StockSearch) {
        return Err(format!("{} has no stock library", account.name));
    }
    let key = store.key(&account.id).ok_or_else(|| {
        format!(
            "{} has no API key; add one in Settings, Accounts",
            account.name
        )
    })?;
    Ok(match account.kind {
        ProviderKind::Pexels => Box::new(Pexels::new(account, key)),
        ProviderKind::Pixabay => Box::new(Pixabay::new(account, key)),
        ProviderKind::Freesound => Box::new(Freesound::new(account, key)),
        _ => return Err(format!("{} has no stock library", account.name)),
    })
}

/// The kinds of stock `account` has.
pub fn cloud_stock_kinds(
    store: &CloudStore,
    account_id: &str,
) -> Result<&'static [stock::StockKind], String> {
    Ok(stock_searcher(store, account_id)?.kinds())
}

/// Search `account`'s library, through the one-day cache.
pub fn cloud_stock_search(
    store: &CloudStore,
    account_id: &str,
    query: &StockQuery,
) -> Result<StockPage, String> {
    if query.text.trim().is_empty() {
        return Ok(StockPage::default());
    }
    let account = store
        .account(account_id)
        .ok_or("that account no longer exists")?;
    let searcher = stock_searcher(store, account_id)?;
    let cache = paths::cache_root().join("library").join("search");
    stock::cached_search(account.kind.id(), searcher.as_ref(), query, Some(&cache))
}

/// Download a hit into the library cache with its `asset.json`; the path is
/// then imported like any file.
pub fn cloud_stock_download(
    hit: &StockHit,
    progress: &dyn Fn(u64),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    stock::download(&paths::cache_root().join("library"), hit, progress, cancel)
}

// --- fal ----------------------------------------------------------------------------

/// The actions fal can run on a clip.
pub fn cloud_fal_actions() -> &'static [FalAction] {
    fal::actions()
}

fn fal_for(store: &CloudStore, account_id: &str) -> Result<Fal, String> {
    let (account, key) = super::resolve(store, account_id, Capability::Process, "process clips")?;
    Ok(Fal::new(account, key.unwrap_or_default()))
}

/// The price of running `action_id` on a clip like `media`, before anything
/// is spent.
pub fn cloud_fal_estimate(
    store: &CloudStore,
    account_id: &str,
    action_id: &str,
    media: &MediaFacts,
) -> Result<Estimate, String> {
    let action = fal::action(action_id).ok_or("that action does not exist")?;
    fal_for(store, account_id)?.estimate(action, media)
}

/// Run `action_id` on the file at `source`. The result goes in a new
/// directory under `root` ([`generated_root`] in the app) with its record.
#[allow(clippy::too_many_arguments)]
pub fn cloud_fal_run(
    store: &CloudStore,
    account_id: &str,
    action_id: &str,
    source: &Path,
    estimate: Option<&Estimate>,
    root: &Path,
    events: &dyn Fn(JobEvent),
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let action = fal::action(action_id).ok_or("that action does not exist")?;
    let dir = provenance::asset_dir(root, "fal");
    fal_for(store, account_id)?.run(action, source, &dir, estimate, events, cancel)
}

// --- caption translation ---------------------------------------------------------------

/// The reply to a translation: the edit, and the new lane.
#[derive(Serialize)]
pub struct Translated {
    #[serde(flatten)]
    pub edit: EditResponse,
    pub track_id: String,
    pub lines: usize,
}

/// Translate the first caption lane into `target` (ISO 639-1) on a lane of
/// its own, one undo step. The new lane sits a little above the original so
/// both can be shown. `chat_model` is the model an OpenAI-compatible account
/// translates with.
pub fn cloud_translate_captions(
    state: &Arc<AppState>,
    store: &CloudStore,
    account_id: &str,
    target: &str,
    source: Option<&str>,
    chat_model: &str,
) -> Result<Translated, String> {
    let target = target.trim().to_ascii_lowercase();
    if target.is_empty() {
        return Err("choose a language to translate into".to_string());
    }
    // Read what to translate, then let go of the lock: the request can take
    // seconds and the project must stay editable meanwhile.
    let (cues, style) = state.with_project(|project| {
        let lane = caption_edit::caption_lane(project).map(|t| t.id.clone());
        let clips: Vec<_> = caption_edit::clips(project)
            .into_iter()
            .filter(|c| Some(&c.track_id) == lane.as_ref())
            .collect();
        let style = clips
            .first()
            .and_then(|c| {
                let (_, segment) = project.segment(&c.segment_id)?;
                let material = project.materials.text(&c.material_id)?;
                Some(CaptionStyle::of(material, segment))
            })
            .unwrap_or_else(|| CaptionStyle::default_for(&project.canvas));
        let cues: Vec<Cue> = clips
            .into_iter()
            .map(|c| Cue::new(c.start, c.end, c.text))
            .collect();
        (cues, style)
    })?;
    if cues.is_empty() {
        return Err("there are no captions to translate; add captions first".to_string());
    }
    let lines: Vec<String> = cues.iter().map(|c| c.text.clone()).collect();
    let translated =
        super::translator(store, account_id, chat_model)?.translate(&lines, &target, source)?;
    translated_lane(state, cues, translated, style, &target)
}

fn translated_lane(
    state: &Arc<AppState>,
    cues: Vec<Cue>,
    translated: Vec<String>,
    mut style: CaptionStyle,
    target: &str,
) -> Result<Translated, String> {
    let cues: Vec<Cue> = cues
        .into_iter()
        .zip(translated)
        .map(|(cue, text)| Cue::new(cue.start, cue.end, text))
        .collect();
    style.position[1] = (style.position[1] + 0.16).min(0.95);
    // A translation carries no word timing; karaoke on it would light words
    // at made-up times.
    style.highlight = None;
    let (track_id, count) = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let mut placed = caption_edit::place(project, &cues, &style, PlaceOptions::default())?;
        // The cues collide with the originals, so `place` always opens a new
        // lane; name it after the language.
        if let EditCommand::Composite { label, commands } = &mut placed.command {
            *label = format!("Translate captions ({})", target.to_uppercase());
            for command in commands.iter_mut() {
                if let EditCommand::AddTrack { track, .. } = command {
                    track.name = format!(
                        "Captions ({})",
                        crate::modules::speech::language_name(target)
                    );
                }
            }
        }
        let added: Vec<String> = placed.materials.iter().map(|m| m.id.clone()).collect();
        project.materials.texts.extend(placed.materials);
        let result = state.history.write().apply(project, placed.command);
        if let Err(error) = result {
            project.materials.texts.retain(|m| !added.contains(&m.id));
            return Err(error);
        }
        (placed.track_id, added.len())
    };
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);
    let history = state.history.read();
    Ok(Translated {
        edit: EditResponse {
            project,
            can_undo: history.can_undo(),
            can_redo: history.can_redo(),
            undo_label: history.undo_label(),
            redo_label: history.redo_label(),
        },
        track_id,
        lines: count,
    })
}

// --- licences ------------------------------------------------------------------------

/// What the licences on the timeline ask of an export.
pub fn cloud_licence_summary(state: &Arc<AppState>) -> Result<LicenceSummary, String> {
    state.with_project(credits::summary)
}

/// After an export: `<name>.credits.txt` beside `video` when anything needs
/// credit.
pub fn cloud_write_credits(state: &Arc<AppState>, video: &Path) -> Result<Option<PathBuf>, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    credits::write_credits(&project, video)
}

/// The credits as text, for "Copy credits".
pub fn cloud_credits_text(state: &Arc<AppState>) -> Result<String, String> {
    state.with_project(|project| credits::credits_text(&credits::summary(project), &project.name))
}

#[cfg(test)]
mod tests {
    use super::super::http::test_server;
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/cloud")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn account(store: &CloudStore, kind: ProviderKind, url: &str, key: &str) -> String {
        let mut account = Account::of_kind(kind);
        account.base_url = url.to_string();
        store.save_account(&account, Some(key)).unwrap();
        account.id
    }

    #[test]
    fn every_kind_is_tested_by_its_own_call() {
        let store = CloudStore::at(scratch("commands-test"));
        for (kind, path) in [
            (ProviderKind::Elevenlabs, "/v1/user/subscription"),
            (ProviderKind::Fal, "/v1/models/pricing"),
            (ProviderKind::Pexels, "/v1/search"),
            (ProviderKind::Pixabay, "/api/"),
            (ProviderKind::Freesound, "/search/text/"),
            (ProviderKind::Deepl, "/v2/usage"),
        ] {
            let server = test_server::serve(vec![(
                200,
                "application/json",
                br#"{"tier":"free","prices":[{"unit_price":1}],"count":3}"#.to_vec(),
            )]);
            let id = account(&store, kind, &server.url, "key-1234");
            let report = cloud_account_test(&store, &id);
            assert!(report.ok, "{kind:?}: {}", report.message);
            let line = server.requests.lock().unwrap()[0].request_line.clone();
            assert!(line.contains(path), "{kind:?}: {line}");
        }
        let views = cloud_accounts(&store, Some(Capability::StockSearch));
        assert_eq!(views.len(), 3);
        assert!(views.iter().all(|v| v.key_hint == "••••1234"));
    }

    #[test]
    fn an_account_without_a_key_is_not_called() {
        let store = CloudStore::at(scratch("commands-nokey"));
        let account = Account::of_kind(ProviderKind::Pexels);
        store.save_account(&account, None).unwrap();
        let report = cloud_account_test(&store, &account.id);
        assert!(!report.ok);
        assert!(report.message.contains("no API key"));
    }

    #[test]
    fn free_tier_speech_is_marked_non_commercial() {
        let dir = scratch("commands-tts");
        let store = CloudStore::at(dir.join("config"));
        let server = test_server::serve(vec![
            (200, "application/json", br#"{"tier":"free"}"#.to_vec()),
            (200, "audio/mpeg", b"mp3".to_vec()),
        ]);
        let id = account(&store, ProviderKind::Elevenlabs, &server.url, "xi");
        let mut request = TtsRequest::new("Welcome to the channel", "voice-1");
        request.with_timing = false;
        let generated = cloud_tts(&store, &id, &request, &dir.join("generated")).unwrap();
        assert_eq!(std::fs::read(&generated.path).unwrap(), b"mp3");
        assert!(generated
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("voice-welcome-to-the-channel"));
        let origin = provenance::read_sidecar(&generated.path).unwrap();
        assert_eq!(origin.account_tier.as_deref(), Some("free"));
        assert_eq!(origin.licence.commercial, Commercial::No);
        assert_eq!(origin.voice, "voice-1");
        assert!(!serde_json::to_string(&origin).unwrap().contains("\"xi\""));
    }

    #[test]
    fn music_and_effects_land_as_generated_files() {
        let dir = scratch("commands-audio");
        let store = CloudStore::at(dir.join("config"));
        let server = test_server::serve(vec![
            (200, "application/json", br#"{"tier":"creator"}"#.to_vec()),
            (200, "audio/mpeg", b"music".to_vec()),
        ]);
        let id = account(&store, ProviderKind::Elevenlabs, &server.url, "xi");
        let generated = cloud_generate_audio(
            &store,
            &id,
            Capability::Music,
            &AudioGenRequest {
                prompt: "Upbeat lofi".into(),
                duration_seconds: Some(10.0),
                looping: false,
                instrumental: true,
                prompt_influence: None,
            },
            &dir.join("generated"),
        )
        .unwrap();
        assert_eq!(generated.origin.licence.commercial, Commercial::Yes);
        assert_eq!(generated.origin.model, "music_v1");
        assert_eq!(generated.origin.title, "Upbeat lofi");
    }

    #[test]
    fn a_pexels_account_cannot_speak() {
        let store = CloudStore::at(scratch("commands-wrong"));
        let id = account(&store, ProviderKind::Pexels, "https://api.pexels.com", "k");
        let error = cloud_tts(
            &store,
            &id,
            &TtsRequest::new("hi", "v"),
            Path::new("/nonexistent"),
        )
        .unwrap_err();
        assert!(error.contains("cannot speak"), "{error}");
    }
}
