//! Cloud features: caption translation, text to speech, stock media.
//!
//! These use the accounts the app's Settings, Accounts page stores
//! (`~/.config/chukcut/accounts.toml`, keys in `secrets.toml` or
//! `CHUKCUT_KEY_<ACCOUNT>`). The CLI cannot add an account. Without one that
//! can do the job, the operation is refused with exit code 1.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::cloud::commands as cloud_commands;
use chukcut_engine::modules::cloud::place;
use chukcut_engine::modules::cloud::stock::{StockHit, StockKind, StockQuery};
use chukcut_engine::modules::cloud::{Capability, CloudStore, TtsRequest};
use chukcut_engine::modules::project::commands as project_commands;
use chukcut_engine::modules::timeline::commands as timeline_commands;
use chukcut_engine::modules::timeline::ops::EditCommand;
use clap::Args;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{enum_named, summary, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::session::Session;
use crate::values::{seconds, Time};

/// Nothing in a one-shot invocation cancels; the engine's download takes a
/// flag all the same.
static NEVER: AtomicBool = AtomicBool::new(false);

fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::Translate => "translate",
        Capability::Tts => "speak (text to speech)",
        Capability::StockSearch => "search stock media",
        _ => "do that",
    }
}

/// The account to use: the one named, or the first that can do
/// `capability`.
fn account(store: &CloudStore, named: Option<&str>, capability: Capability) -> CliResult<String> {
    if let Some(id) = named {
        // The engine checks it, with its own message, when it is used.
        return Ok(id.trim().to_string());
    }
    cloud_commands::cloud_accounts(store, Some(capability))
        .into_iter()
        .next()
        .map(|a| a.account.id)
        .ok_or_else(|| {
            CliError::refused(format!(
                "no cloud account can {}; add one in the app (Settings, Accounts). The CLI cannot add accounts",
                capability_name(capability)
            ))
        })
}

/// Import `path` into the project and, with `at`, put it on the timeline
/// there. Returns the material and the clip, when one was placed.
fn import_and_place(
    session: &Session,
    path: &Path,
    at: Option<Time>,
) -> CliResult<(Value, Option<Value>)> {
    let material = pollster::block_on(project_commands::project_import_media(
        &session.state,
        path.to_string_lossy().into_owned(),
    ))
    .map_err(|e| CliError::refused(format!("{}: {e}", path.display())))?;
    let mut entry = serde_json::to_value(&material).unwrap_or(Value::Null);
    entry["duration"] = json!(seconds(material.duration));
    let Some(at) = at else {
        return Ok((entry, None));
    };
    let at = at.resolve(session.fps());
    let command = session.with(|p| place::at_playhead(p, &material.id, at))?;
    let segment_id = match &command {
        EditCommand::InsertSegment { segment, .. } => Some(segment.id.clone()),
        EditCommand::Composite { commands, .. } => commands.iter().find_map(|c| match c {
            EditCommand::InsertSegment { segment, .. } => Some(segment.id.clone()),
            _ => None,
        }),
        _ => None,
    };
    timeline_commands::timeline_apply(&session.state, command)?;
    let clip = segment_id.map(|id| session.with(|p| summary::clip_by_id(p, &id)));
    Ok((entry, clip))
}

/// What `catalog accounts` lists: the cloud accounts the app has set up,
/// with what each can do. Keys are never shown, only their last four
/// characters.
pub fn accounts_catalog() -> Value {
    let store = CloudStore::user();
    let caps = [
        ("transcribe", Capability::Transcribe),
        ("tts", Capability::Tts),
        ("translate", Capability::Translate),
        ("stock", Capability::StockSearch),
        ("sound_effects", Capability::SoundEffects),
        ("music", Capability::Music),
        ("process", Capability::Process),
    ];
    json!(cloud_commands::cloud_accounts(&store, None)
        .into_iter()
        .map(|view| {
            let can: Vec<&str> = caps
                .iter()
                .filter(|(_, c)| view.account.can(*c))
                .map(|(n, _)| *n)
                .collect();
            json!({
                "id": view.account.id,
                "name": view.account.name,
                "kind": view.account.kind,
                "base_url": view.account.base_url,
                "has_key": view.has_key,
                "key_hint": view.key_hint,
                "can": can,
                "text": can.join(", "),
            })
        })
        .collect::<Vec<_>>())
}

/// Translate the first caption lane into another language, onto a new
/// caption lane just above it. One undo step. Needs a DeepL or an
/// OpenAI-compatible account.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TranslateCaptionsArgs {
    /// The language to translate into, as an ISO 639-1 code (de, fr, es).
    #[arg(long)]
    pub to: String,
    /// The captions' language; detected when left out.
    #[arg(long)]
    pub from: Option<String>,
    /// The cloud account id; the first one that can translate when left out.
    #[arg(long)]
    pub account: Option<String>,
    /// The chat model for an OpenAI-compatible account (e.g. gpt-4o-mini).
    #[arg(long)]
    pub model: Option<String>,
}

impl Operation for TranslateCaptionsArgs {
    const NAME: &'static str = "translate_captions";
    fn run(self, session: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        if self.to.trim().is_empty() {
            return Err(CliError::usage("--to needs a language code"));
        }
        let store = CloudStore::user();
        let account = account(&store, self.account.as_deref(), Capability::Translate)?;
        let done = cloud_commands::cloud_translate_captions(
            &session.state,
            &store,
            &account,
            &self.to,
            self.from.as_deref(),
            self.model.as_deref().unwrap_or(""),
        )?;
        Ok(Outcome::changed(
            format!(
                "translated {} caption(s) into {}",
                done.lines,
                self.to.trim()
            ),
            json!({"track_id": done.track_id, "lines": done.lines}),
        ))
    }
}

/// Speak a text with a cloud voice (ElevenLabs or OpenAI-compatible) and
/// import the sound into the project. With `at`, it also goes on an audio
/// lane at that time. The file goes to ~/.local/share/chukcut/generated
/// with a record of where it came from.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct TtsArgs {
    /// What to say.
    pub text: String,
    /// The voice id or name, as the provider calls it (alloy, nova, ...).
    #[arg(long)]
    pub voice: String,
    /// The cloud account id; the first one that can speak when left out.
    #[arg(long)]
    pub account: Option<String>,
    /// The speech model; the account's default when left out.
    #[arg(long)]
    pub model: Option<String>,
    /// 1 is normal.
    #[arg(long)]
    pub speed: Option<f32>,
    /// OpenAI voices: tone, accent or emotion in plain words.
    #[arg(long)]
    pub instructions: Option<String>,
    /// Put the voice on the timeline at this time.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for TtsArgs {
    const NAME: &'static str = "tts";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        if self.text.trim().is_empty() {
            return Err(CliError::usage("there is no text to speak"));
        }
        let store = CloudStore::user();
        let account = account(&store, self.account.as_deref(), Capability::Tts)?;
        let mut request = TtsRequest::new(self.text.clone(), self.voice.clone());
        if let Some(m) = &self.model {
            request.model = m.clone();
        }
        if let Some(s) = self.speed {
            request.speed = s;
        }
        if let Some(i) = &self.instructions {
            request.instructions = i.clone();
        }
        ctx.progress("Generating the voice", None);
        let generated = cloud_commands::cloud_tts(
            &store,
            &account,
            &request,
            &cloud_commands::generated_root(),
        )?;
        let (material, clip) = import_and_place(session, &generated.path, self.at)?;
        Ok(Outcome::changed(
            format!("spoke {} into {}", self.voice, generated.path.display()),
            json!({
                "path": generated.path,
                "material": material,
                "clip": clip,
                "licence": generated.origin.licence,
            }),
        ))
    }
}

const KINDS: &[&str] = &["video", "photo", "sound"];

/// The kinds of stock media (video, photo, sound) an account's library has.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StockKindsArgs {
    /// The cloud account id; the first stock account when left out.
    #[arg(long)]
    pub account: Option<String>,
}

impl Operation for StockKindsArgs {
    const NAME: &'static str = "stock_kinds";
    fn run(self, _: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let store = CloudStore::user();
        let account = account(&store, self.account.as_deref(), Capability::StockSearch)?;
        let kinds = cloud_commands::cloud_stock_kinds(&store, &account)?;
        Ok(Outcome::read(
            format!("{} has {} kind(s) of stock", account, kinds.len()),
            json!({"account": account, "kinds": kinds}),
        ))
    }
}

/// What to search for in a stock library.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StockQueryArgs {
    /// The words to search for.
    pub query: String,
    /// video (the default), photo or sound.
    #[arg(long, default_value = "video")]
    #[serde(default = "video")]
    pub kind: String,
    /// The page of results, from 1.
    #[arg(long)]
    pub page: Option<u32>,
    /// Results per page; 24 when left out.
    #[arg(long)]
    pub per_page: Option<u32>,
    /// Also show sounds that do not allow commercial use.
    #[arg(long)]
    #[serde(default)]
    pub non_commercial: bool,
    /// The cloud account id (Pexels, Pixabay or Freesound); the first stock
    /// account when left out.
    #[arg(long)]
    pub account: Option<String>,
}

fn video() -> String {
    "video".into()
}

impl StockQueryArgs {
    fn query(&self) -> CliResult<StockQuery> {
        let kind: StockKind = enum_named("stock kind", &self.kind, KINDS)?;
        let mut query = StockQuery::new(self.query.trim(), kind);
        if let Some(page) = self.page {
            query.page = page.max(1);
        }
        if let Some(n) = self.per_page {
            query.per_page = n.clamp(1, 80);
        }
        query.include_non_commercial = self.non_commercial;
        Ok(query)
    }

    fn search(&self) -> CliResult<(String, Vec<StockHit>, Value)> {
        if self.query.trim().is_empty() {
            return Err(CliError::usage("a stock search needs words to search for"));
        }
        let query = self.query()?;
        let store = CloudStore::user();
        let account = account(&store, self.account.as_deref(), Capability::StockSearch)?;
        let page = cloud_commands::cloud_stock_search(&store, &account, &query)?;
        let info = json!({
            "total": page.total,
            "has_more": page.has_more,
            "attribution": page.attribution,
            "attribution_url": page.attribution_url,
        });
        Ok((account, page.hits, info))
    }
}

fn hit_json(hit: &StockHit) -> Value {
    json!({
        "id": hit.id,
        "provider": hit.provider,
        "kind": hit.kind,
        "title": hit.title,
        "creator": hit.creator,
        "source_url": hit.source_url,
        "preview_url": hit.preview_url,
        "width": hit.width,
        "height": hit.height,
        "duration": hit.duration_seconds,
        "licence": hit.licence,
        "credit": hit.credit,
    })
}

/// Search a stock library (Pexels, Pixabay, Freesound). Results are cached
/// for a day. Changes nothing; `stock_download` takes a result's id.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StockSearchArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub search: StockQueryArgs,
}

impl Operation for StockSearchArgs {
    const NAME: &'static str = "stock_search";
    fn run(self, _: &mut Session, _: &Ctx) -> CliResult<Outcome> {
        let (account, hits, mut info) = self.search.search()?;
        let mut message = format!("{} result(s) from {account}", hits.len());
        for hit in &hits {
            message.push_str(&format!(
                "\n  {:<14} {}  ({})",
                hit.id, hit.title, hit.creator
            ));
        }
        info["account"] = json!(account);
        info["hits"] = json!(hits.iter().map(hit_json).collect::<Vec<_>>());
        Ok(Outcome::read(message, info))
    }
}

/// Download one stock search result and import it into the project. The
/// search runs again (from the one-day cache) to find the result by `id`.
/// With `at`, the file also goes on the timeline at that time.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StockDownloadArgs {
    #[command(flatten)]
    #[serde(flatten)]
    pub search: StockQueryArgs,
    /// The id of the result, as `stock search` lists it.
    #[arg(long)]
    pub id: String,
    /// Put the file on the timeline at this time.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for StockDownloadArgs {
    const NAME: &'static str = "stock_download";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let (_, hits, _) = self.search.search()?;
        let hit = hits
            .iter()
            .find(|h| h.id == self.id.trim())
            .ok_or_else(|| {
                CliError::refused(format!(
                    "the search has no result {}; search with the same words, kind and page",
                    self.id
                ))
            })?;
        let progress = |bytes: u64| {
            ctx.progress(
                &format!("Downloading {:.1} MB", bytes as f64 / 1_048_576.0),
                None,
            )
        };
        let path = cloud_commands::cloud_stock_download(hit, &progress, &NEVER)?;
        let (material, clip) = import_and_place(session, &path, self.at)?;
        Ok(Outcome::changed(
            format!("downloaded {} into {}", hit.title, path.display()),
            json!({
                "path": path,
                "material": material,
                "clip": clip,
                "credit": hit.credit,
                "licence": hit.licence,
            }),
        ))
    }
}
