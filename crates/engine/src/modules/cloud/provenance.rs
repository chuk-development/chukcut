//! Where a file came from and what its licence allows.
//!
//! Every file the cloud module writes — a voiceover, a sound effect, a stock
//! clip, a fal result — sits in a directory of its own next to an
//! `asset.json` sidecar that holds an [`Origin`]. Importing the file copies
//! the record into the project (`MaterialPool::origins`), so a project keeps
//! its credits after the cache is cleared or the file moves to another
//! machine, and the export dialog can say "1 item is non-commercial" before
//! the upload rather than after a claim.
//!
//! Generated files cost money and are user data: they go under
//! `data_root()/generated`, never the cache. Stock files can be fetched
//! again and go under `cache_root()/library`
//! (`docs/research/integrations.md` §7.5, `open-assets.md`).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::modules::workspace::paths;

/// The sidecar's name, in the asset's own directory.
pub const SIDECAR: &str = "asset.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginKind {
    /// Made by a model with the user's key.
    #[default]
    Generated,
    /// Downloaded from a stock library.
    Stock,
    /// From chukcut's built-in asset library: a curated track, a sound-effect
    /// pack, a sticker (`modules::library`). Credited like stock, but not a
    /// stock provider the panel has to name.
    Library,
}

/// May the result go into a monetised video?
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Commercial {
    Yes,
    No,
    /// The vendor's terms do not say, or chukcut could not tell (a plan it
    /// could not read, a model with its own licence).
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Licence {
    /// An SPDX id where one exists (`CC-BY-4.0`, `CC0-1.0`), else a
    /// `LicenseRef-` of our own (`LicenseRef-Pexels`).
    pub id: String,
    /// What the user reads: "CC BY 4.0", "Pexels License".
    pub name: String,
    #[serde(default)]
    pub url: String,
    pub commercial: Commercial,
    /// The credit must appear with the video.
    #[serde(default)]
    pub attribution_required: bool,
    /// The video must carry the same licence (CC BY-SA).
    #[serde(default)]
    pub share_alike: bool,
    /// One line on the vendor's terms or the plan, for the export summary.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// "synthid", "c2pa", when the vendor marks its output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watermark: Option<String>,
}

impl Licence {
    /// A Creative Commons licence from its deed URL, as Freesound gives it
    /// (`http://creativecommons.org/licenses/by-nc/4.0/`).
    pub fn from_cc_url(url: &str) -> Self {
        let lower = url.to_ascii_lowercase();
        let version = lower
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .unwrap_or("4.0")
            .to_string();
        if lower.contains("publicdomain/zero") {
            return Self {
                id: "CC0-1.0".into(),
                name: "CC0".into(),
                url: url.into(),
                commercial: Commercial::Yes,
                ..Self::default()
            };
        }
        if lower.contains("sampling+") || lower.contains("sampling-plus") {
            return Self {
                id: "LicenseRef-CC-Sampling-Plus".into(),
                name: "CC Sampling+".into(),
                url: url.into(),
                commercial: Commercial::Unknown,
                attribution_required: true,
                note: "Sampling+ is retired; check the sound's page".into(),
                ..Self::default()
            };
        }
        let part = lower
            .split("/licenses/")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("");
        if part.is_empty() {
            return Self {
                id: "LicenseRef-Unknown".into(),
                name: "Unknown licence".into(),
                url: url.into(),
                ..Self::default()
            };
        }
        let nc = part.contains("nc");
        let sa = part.contains("sa");
        let nd = part.contains("nd");
        Self {
            id: format!("CC-{}-{version}", part.to_ascii_uppercase()),
            name: format!("CC {} {version}", part.to_ascii_uppercase()),
            url: url.into(),
            commercial: if nc { Commercial::No } else { Commercial::Yes },
            attribution_required: true,
            share_alike: sa,
            note: if nd {
                "NoDerivatives: the item may not be edited".into()
            } else {
                String::new()
            },
            watermark: None,
        }
    }

    /// Free to use, no credit needed: Pexels and Pixabay.
    pub fn free(id: &str, name: &str, url: &str) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            url: url.into(),
            commercial: Commercial::Yes,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub amount: f64,
    /// `USD`, or `credits`, `characters` where that is what the vendor bills.
    pub unit: String,
    /// `true` when this is the estimate shown before running, not a bill.
    #[serde(default)]
    pub estimated: bool,
}

/// The provenance record of one file. Never holds a key or anything about
/// the user beyond the plan name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub kind: OriginKind,
    /// The provider id: `elevenlabs`, `pexels`, `fal`, …
    pub provider: String,
    /// The file this record describes, by name, in the same directory.
    #[serde(default)]
    pub file: String,
    /// What the user would call it: the prompt, the stock title.
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// The API path, or fal's endpoint id.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    /// The prompt, or the spoken text for speech.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
    /// The voice, for speech.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub voice: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub request_id: String,
    /// RFC 3339, UTC.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// The plan at generation time (ElevenLabs `free`, `creator`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_tier: Option<String>,
    /// Stock: the item's own id at the provider.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub creator: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub creator_url: String,
    /// The item's page at the provider.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_url: String,
    pub licence: Licence,
    /// The line for the credits file: `"Rain" by InspectorJ
    /// (freesound.org/s/401275/) licensed under CC BY 4.0`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub credit: String,
}

impl Origin {
    pub fn new(kind: OriginKind, provider: &str) -> Self {
        Self {
            kind,
            provider: provider.to_string(),
            created_at: now_rfc3339(),
            ..Self::default()
        }
    }

    /// The credit line, built from the fields when the provider gave none.
    pub fn credit_line(&self) -> String {
        if !self.credit.is_empty() {
            return self.credit.clone();
        }
        let provider = provider_name(&self.provider);
        match self.kind {
            OriginKind::Stock | OriginKind::Library => {
                let what = if self.title.is_empty() {
                    "Item".to_string()
                } else {
                    format!("\"{}\"", self.title)
                };
                let by = if self.creator.is_empty() {
                    String::new()
                } else {
                    format!(" by {}", self.creator)
                };
                let link = if self.source_url.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", self.source_url)
                };
                format!("{what}{by} on {provider}{link}, {}", self.licence.name)
            }
            OriginKind::Generated => {
                let model = if self.model.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", self.model)
                };
                let plan = self
                    .account_tier
                    .as_deref()
                    .map(|t| format!(", {t} plan"))
                    .unwrap_or_default();
                format!("Generated with {provider}{model}{plan}")
            }
        }
    }
}

/// The vendor's display name for a provider id.
pub fn provider_name(id: &str) -> &str {
    match id {
        "elevenlabs" => "ElevenLabs",
        "fal" => "fal.ai",
        "pexels" => "Pexels",
        "pixabay" => "Pixabay",
        "freesound" => "Freesound",
        "deepl" => "DeepL",
        "openai_compatible" => "an OpenAI-compatible service",
        "openai" => "OpenAI",
        "incompetech" => "Incompetech",
        "kenney" => "Kenney",
        "opengameart" => "OpenGameArt",
        "fluent-emoji" => "Fluent Emoji",
        "noto-emoji" => "Noto Emoji",
        "iconify" => "Iconify",
        other => other,
    }
}

/// Now, as `2026-10-03T12:00:00Z`.
pub fn now_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    rfc3339(seconds)
}

/// Unix seconds as an RFC 3339 UTC time. Howard Hinnant's civil-from-days,
/// which is exact for every date this program will see; a date crate for one
/// timestamp would be more code than this.
pub fn rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// A file name made safe: letters, digits, `-` and `_`, at most 40 of them.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.chars().count() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "asset".to_string()
    } else {
        out
    }
}

/// A new directory for one generated result under `root`:
/// `<time>-<provider>-<id>`, so a listing sorts by when it was made.
pub fn asset_dir(root: &Path, provider: &str) -> PathBuf {
    let stamp = now_rfc3339()
        .replace(['-', ':'], "")
        .replace('T', "-")
        .trim_end_matches('Z')
        .to_string();
    let short = &uuid::Uuid::new_v4().simple().to_string()[..6];
    root.join(format!("{stamp}-{provider}-{short}"))
}

/// The cache directory of one stock item.
pub fn stock_dir(provider: &str, id: &str) -> PathBuf {
    paths::cache_root()
        .join("library")
        .join(provider)
        .join(slug(id))
}

/// Write `bytes` as `dir/file_name`, with the sidecar beside it. Returns the
/// file's path.
pub fn write_asset(
    dir: &Path,
    file_name: &str,
    bytes: &[u8],
    origin: &Origin,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join(file_name);
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    write_sidecar(&path, origin)?;
    Ok(path)
}

/// Write the sidecar for the file at `path`.
pub fn write_sidecar(path: &Path, origin: &Origin) -> Result<(), String> {
    let mut origin = origin.clone();
    origin.file = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    let text = serde_json::to_string_pretty(&origin).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(SIDECAR), text)
        .map_err(|e| format!("cannot write the licence record: {e}"))
}

/// The record for the file at `path`, when its directory has a sidecar that
/// names it.
pub fn read_sidecar(path: &Path) -> Option<Origin> {
    let sidecar = path.parent()?.join(SIDECAR);
    let text = std::fs::read_to_string(sidecar).ok()?;
    let origin: Origin = serde_json::from_str(&text).ok()?;
    let name = path.file_name()?.to_string_lossy();
    (origin.file == name).then_some(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creative_commons_urls_become_licences() {
        let by = Licence::from_cc_url("https://creativecommons.org/licenses/by/4.0/");
        assert_eq!(by.id, "CC-BY-4.0");
        assert_eq!(by.commercial, Commercial::Yes);
        assert!(by.attribution_required);
        let nc = Licence::from_cc_url("http://creativecommons.org/licenses/by-nc/3.0/");
        assert_eq!(nc.id, "CC-BY-NC-3.0");
        assert_eq!(nc.commercial, Commercial::No);
        let zero = Licence::from_cc_url("http://creativecommons.org/publicdomain/zero/1.0/");
        assert_eq!(zero.id, "CC0-1.0");
        assert!(!zero.attribution_required);
        let sa = Licence::from_cc_url("https://creativecommons.org/licenses/by-sa/4.0/");
        assert!(sa.share_alike);
    }

    #[test]
    fn times_are_rfc3339() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_115_200), "2026-10-04T12:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn a_sidecar_belongs_to_the_file_it_names() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/cloud/provenance");
        let _ = std::fs::remove_dir_all(&dir);
        let mut origin = Origin::new(OriginKind::Stock, "freesound");
        origin.title = "Rain".into();
        origin.creator = "InspectorJ".into();
        origin.source_url = "https://freesound.org/s/401275/".into();
        origin.licence = Licence::from_cc_url("https://creativecommons.org/licenses/by/4.0/");
        let path = write_asset(&dir, "rain.mp3", b"mp3", &origin).unwrap();
        let back = read_sidecar(&path).unwrap();
        assert_eq!(back.file, "rain.mp3");
        assert_eq!(back.title, "Rain");
        assert!(read_sidecar(&dir.join("other.mp3")).is_none());
        assert_eq!(
            back.credit_line(),
            "\"Rain\" by InspectorJ on Freesound (https://freesound.org/s/401275/), CC BY 4.0"
        );
    }

    #[test]
    fn slugs_are_safe_file_names() {
        assert_eq!(slug("Hello, World! / ../x"), "hello-world-x");
        assert_eq!(slug("///"), "asset");
    }
}
