//! Music and sound effects with licences a monetised video can carry.
//!
//! **Music** is Kevin MacLeod's Incompetech catalogue, CC BY 4.0: free for
//! any use with the credit line he asks for, which the export writes into the
//! credits file. [`CURATED`] is fifty tracks by mood, known by heart so the
//! list shows without a network; the whole catalogue (about 1,400 tracks,
//! `pieces.json`) is there to search, cached for a week. A track is
//! downloaded once, on first play or add, with its licence record — which is
//! also the user's proof when a Content ID claim lands on a widely used
//! track.
//!
//! **Sound effects** are CC0 packs: Kenney's audio packs and rubberduck's and
//! SubspaceAudio's sets on OpenGameArt. A pack is a zip, fetched the first
//! time it is opened and unpacked into one directory per sound, each with its
//! `asset.json`, so a sound behaves like any other library file from then
//! on. Kenney's download links carry a hash that changes with a new version
//! of a pack; when the stored link stops answering, the pack's page is read
//! for the current one.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::licence;
use super::net;
use crate::modules::cloud::provenance::{self, Origin, OriginKind};

/// How long the full music list is trusted.
pub const CATALOGUE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// Where the music comes from. Tests point it at a local server.
#[derive(Debug, Clone)]
pub struct MusicSource {
    /// `https://incompetech.com/music/royalty-free`
    pub base: String,
}

impl Default for MusicSource {
    fn default() -> Self {
        Self {
            base: "https://incompetech.com/music/royalty-free".into(),
        }
    }
}

/// One track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub title: String,
    /// The file name on incompetech.com.
    pub file: String,
    pub seconds: u32,
    pub bpm: u32,
    /// Our mood for a curated track, else Incompetech's first "feel".
    pub mood: String,
    /// Incompetech's feel words, comma-separated.
    #[serde(default)]
    pub feel: String,
}

/// The moods of the curated list, in the order the panel shows them.
pub const MOODS: [&str; 5] = ["Upbeat", "Chill", "Funny", "Cinematic", "Calm"];

/// (title, file, seconds, bpm, mood). Chosen from `pieces.json` by feel; every
/// one is CC BY 4.0 like the rest of the catalogue.
pub const CURATED: &[(&str, &str, u32, u32, &str)] = &[
    (
        "Monkeys Spinning Monkeys",
        "Monkeys Spinning Monkeys.mp3",
        125,
        144,
        "Upbeat",
    ),
    ("Carefree", "Carefree.mp3", 205, 96, "Upbeat"),
    ("Wallpaper", "Wallpaper.mp3", 220, 92, "Upbeat"),
    ("Happy Alley", "Happy Alley.mp3", 80, 112, "Upbeat"),
    (
        "Beachfront Celebration",
        "Beachfront Celebration.mp3",
        187,
        120,
        "Upbeat",
    ),
    ("Feelin Good", "Feelin Good.mp3", 225, 131, "Upbeat"),
    ("Cipher", "Cipher2.mp3", 231, 150, "Upbeat"),
    (
        "Neon Laser Horizon",
        "Neon Laser Horizon.mp3",
        179,
        160,
        "Upbeat",
    ),
    ("Electrodoodle", "Electrodoodle.mp3", 166, 120, "Upbeat"),
    ("Overcast", "Overcast.mp3", 228, 120, "Upbeat"),
    (
        "Local Forecast - Elevator",
        "Local Forecast - Elevator.mp3",
        189,
        82,
        "Chill",
    ),
    ("Chill Wave", "Chill Wave.mp3", 240, 100, "Chill"),
    ("Cool Vibes", "Cool Vibes.mp3", 218, 83, "Chill"),
    ("Lobby Time", "Lobby Time.mp3", 193, 128, "Chill"),
    ("Bossa Antigua", "Bossa Antigua.mp3", 283, 70, "Chill"),
    ("Easy Lemon", "Easy Lemon.mp3", 126, 82, "Chill"),
    ("Jazz Brunch", "Jazz Brunch.mp3", 323, 100, "Chill"),
    ("Werq", "Werq.mp3", 162, 125, "Chill"),
    ("Smooth Lovin", "Smooth Lovin.mp3", 259, 75, "Chill"),
    ("Airport Lounge", "Airport Lounge.mp3", 308, 129, "Chill"),
    ("Sneaky Snitch", "Sneaky Snitch.mp3", 137, 87, "Funny"),
    ("Fluffing a Duck", "Fluffing a Duck.mp3", 67, 122, "Funny"),
    ("Investigations", "Investigations.mp3", 94, 94, "Funny"),
    ("Hyperfun", "Hyperfun.mp3", 233, 200, "Funny"),
    ("Pixelland", "Pixelland.mp3", 234, 230, "Funny"),
    (
        "Spazzmatica Polka",
        "Spazzmatica Polka.mp3",
        96,
        140,
        "Funny",
    ),
    ("The Builder", "The Builder.mp3", 118, 123, "Funny"),
    ("Vivacity", "Vivacity.mp3", 232, 142, "Funny"),
    ("Hidden Agenda", "Hidden Agenda.mp3", 135, 132, "Funny"),
    (
        "Itty Bitty 8 Bit",
        "Itty Bitty 8 Bit.mp3",
        194,
        108,
        "Funny",
    ),
    ("Heroic Age", "Heroic Age.mp3", 97, 129, "Cinematic"),
    ("Five Armies", "Five Armies.mp3", 156, 238, "Cinematic"),
    ("Crusade", "Crusade.mp3", 199, 89, "Cinematic"),
    (
        "Volatile Reaction",
        "Volatile Reaction.mp3",
        165,
        155,
        "Cinematic",
    ),
    ("Impact Prelude", "Impact Prelude.mp3", 203, 80, "Cinematic"),
    ("Ether Vox", "Ether Vox.mp3", 206, 60, "Cinematic"),
    ("Lightless Dawn", "Lightless Dawn.mp3", 380, 90, "Cinematic"),
    ("Blue Feather", "Blue Feather.mp3", 278, 42, "Cinematic"),
    (
        "Sovereign Quarter",
        "Sovereign Quarter.mp3",
        291,
        109,
        "Cinematic",
    ),
    (
        "Ethernight Club",
        "Ethernight Club.mp3",
        306,
        117,
        "Cinematic",
    ),
    ("Dreamer", "Dreamer.mp3", 204, 100, "Calm"),
    ("Midnight Tale", "Midnight Tale.mp3", 162, 73, "Calm"),
    (
        "Meditation Impromptu 01",
        "Meditation Impromptu 01.mp3",
        213,
        0,
        "Calm",
    ),
    ("Inspired", "Inspired.mp3", 286, 120, "Calm"),
    ("Fretless", "Fretless.mp3", 336, 100, "Calm"),
    ("Life of Riley", "Life of Riley.mp3", 235, 102, "Calm"),
    ("Sunshine", "Sunshine A.mp3", 202, 88, "Calm"),
    ("Sincerely", "Sincerely.mp3", 375, 72, "Calm"),
    ("Wholesome", "Wholesome.mp3", 364, 128, "Calm"),
    ("Cattails", "Cattails.mp3", 159, 77, "Calm"),
];

/// The curated tracks, optionally of one mood.
pub fn curated(mood: Option<&str>) -> Vec<Track> {
    CURATED
        .iter()
        .filter(|t| mood.is_none_or(|m| t.4 == m))
        .map(|&(title, file, seconds, bpm, mood)| Track {
            title: title.into(),
            file: file.into(),
            seconds,
            bpm,
            mood: mood.into(),
            feel: String::new(),
        })
        .collect()
}

/// `00:02:05` → 125.
fn parse_length(text: &str) -> u32 {
    text.split(':')
        .try_fold(0u32, |acc, part| {
            part.trim().parse::<u32>().ok().map(|v| acc * 60 + v)
        })
        .unwrap_or(0)
}

/// The whole Incompetech catalogue, newest first as the site lists it.
/// `stale` is true when it came from an old copy.
pub fn catalogue(source: &MusicSource, dir: &Path) -> Result<(Vec<Track>, bool), String> {
    let fetched = net::cached(
        &format!("{}/pieces.json", source.base),
        &dir.join("incompetech.json"),
        CATALOGUE_TTL,
        "the music list from incompetech.com",
    )?;
    let value: serde_json::Value = serde_json::from_slice(&fetched.bytes)
        .map_err(|e| format!("the music list did not read ({e})"))?;
    let text = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let tracks = value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|piece| {
            let title = text(piece, "title");
            let file = text(piece, "filename");
            if title.is_empty() || !file.to_ascii_lowercase().ends_with(".mp3") {
                return None;
            }
            let feel = text(piece, "feel");
            Some(Track {
                mood: feel.split(',').next().unwrap_or("").trim().to_string(),
                title,
                file,
                seconds: parse_length(&text(piece, "length")),
                bpm: text(piece, "bpm").parse().unwrap_or(0),
                feel,
            })
        })
        .collect();
    Ok((tracks, fetched.stale))
}

/// Tracks whose title or feel contains `query`.
pub fn search(tracks: &[Track], query: &str) -> Vec<Track> {
    let query = query.trim().to_lowercase();
    tracks
        .iter()
        .filter(|t| {
            query.is_empty()
                || t.title.to_lowercase().contains(&query)
                || t.feel.to_lowercase().contains(&query)
                || t.mood.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}

impl Track {
    /// The credit line Kevin MacLeod asks for, on one line.
    pub fn credit(&self) -> String {
        format!(
            "\"{}\" Kevin MacLeod (incompetech.com), Licensed under Creative Commons: By Attribution 4.0 License, http://creativecommons.org/licenses/by/4.0/",
            self.title
        )
    }

    pub fn origin(&self) -> Origin {
        let mut licence = licence::spdx("CC-BY-4.0");
        licence.note =
            "widely used: a Content ID claim can happen; this record is the proof to dispute it"
                .into();
        Origin {
            title: self.title.clone(),
            source_id: self.file.clone(),
            creator: "Kevin MacLeod".into(),
            creator_url: "https://incompetech.com".into(),
            source_url: "https://incompetech.com/music/royalty-free/music.html".into(),
            licence,
            credit: self.credit(),
            ..Origin::new(OriginKind::Library, "incompetech")
        }
    }

    /// Where the track is kept under `root`.
    pub fn path(&self, root: &Path) -> PathBuf {
        let stem = provenance::slug(&self.title);
        root.join("incompetech")
            .join(&stem)
            .join(format!("{stem}.mp3"))
    }
}

/// The track's file with its licence record, downloaded once under `root`.
pub fn fetch_track(source: &MusicSource, track: &Track, root: &Path) -> Result<PathBuf, String> {
    let path = track.path(root);
    if path.exists() && provenance::read_sidecar(&path).is_some() {
        return Ok(path);
    }
    let url = format!(
        "{}/mp3-royaltyfree/{}",
        source.base,
        crate::modules::cloud::http::encode(&track.file)
    );
    net::download(&url, &path)
        .map_err(|e| net::unreachable(&format!("\"{}\"", track.title), &e))?;
    provenance::write_sidecar(&path, &track.origin())?;
    Ok(path)
}

// --- sound effects -------------------------------------------------------------

/// A CC0 sound-effect pack.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SfxPack {
    /// Our id, also the directory name.
    pub id: &'static str,
    pub name: &'static str,
    /// `kenney` or `opengameart`.
    pub provider: &'static str,
    pub creator: &'static str,
    /// The pack's page: the source of the licence, and where Kenney's current
    /// download link is read from.
    pub page: &'static str,
    pub zip: &'static str,
    /// Megabytes, for the panel to say before a download.
    pub megabytes: f32,
}

/// The packs, all CC0, checked by hand on their pages (2026-10-03).
pub const SFX_PACKS: &[SfxPack] = &[
    SfxPack { id: "kenney-interface", name: "Interface", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/interface-sounds", zip: "https://kenney.nl/media/pages/assets/interface-sounds/fa43c1dd4d-1677589452/kenney_interface-sounds.zip", megabytes: 0.8 },
    SfxPack { id: "kenney-ui", name: "UI clicks", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/ui-audio", zip: "https://kenney.nl/media/pages/assets/ui-audio/490d233f68-1677590494/kenney_ui-audio.zip", megabytes: 0.4 },
    SfxPack { id: "kenney-impact", name: "Impacts", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/impact-sounds", zip: "https://kenney.nl/media/pages/assets/impact-sounds/87b4ddecda-1677589768/kenney_impact-sounds.zip", megabytes: 0.8 },
    SfxPack { id: "kenney-digital", name: "Digital", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/digital-audio", zip: "https://kenney.nl/media/pages/assets/digital-audio/216eac4753-1677590265/kenney_digital-audio.zip", megabytes: 1.0 },
    SfxPack { id: "kenney-rpg", name: "Foley", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/rpg-audio", zip: "https://kenney.nl/media/pages/assets/rpg-audio/8e99002d76-1677590336/kenney_rpg-audio.zip", megabytes: 1.0 },
    SfxPack { id: "kenney-casino", name: "Casino", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/casino-audio", zip: "https://kenney.nl/media/pages/assets/casino-audio/2472606a04-1721639069/kenney_casino-audio.zip", megabytes: 0.9 },
    SfxPack { id: "kenney-scifi", name: "Sci-fi", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/sci-fi-sounds", zip: "https://kenney.nl/media/pages/assets/sci-fi-sounds/6b296f9ecf-1677589334/kenney_sci-fi-sounds.zip", megabytes: 5.9 },
    SfxPack { id: "kenney-jingles", name: "Jingles", provider: "kenney", creator: "Kenney", page: "https://kenney.nl/assets/music-jingles", zip: "https://kenney.nl/media/pages/assets/music-jingles/f37e530b9e-1677590399/kenney_music-jingles.zip", megabytes: 1.2 },
    SfxPack { id: "oga-100-sfx", name: "Everyday 1", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/100-cc0-sfx", zip: "https://opengameart.org/sites/default/files/100-CC0-SFX_0.zip", megabytes: 2.9 },
    SfxPack { id: "oga-100-sfx-2", name: "Everyday 2", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/100-cc0-sfx-2", zip: "https://opengameart.org/sites/default/files/sfx_100_v2.zip", megabytes: 2.4 },
    SfxPack { id: "oga-retro-synth", name: "Retro synth", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/50-cc0-retro-synth-sfx", zip: "https://opengameart.org/sites/default/files/50-CC0-retro-synth-SFX.zip", megabytes: 1.9 },
    SfxPack { id: "oga-creatures", name: "Creatures", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/80-cc0-creature-sfx", zip: "https://opengameart.org/sites/default/files/80-CC0-creature-SFX_0.zip", megabytes: 1.9 },
    SfxPack { id: "oga-bangs", name: "Bangs and fireworks", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/25-cc0-bang-firework-sfx", zip: "https://opengameart.org/sites/default/files/25-CC0-bang-sfx.zip", megabytes: 1.4 },
    SfxPack { id: "oga-water", name: "Water and slime", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/40-cc0-water-splash-slime-sfx", zip: "https://opengameart.org/sites/default/files/water-splash-slime-sfx.zip", megabytes: 2.3 },
    SfxPack { id: "oga-breaking", name: "Breaking and falling", provider: "opengameart", creator: "rubberduck", page: "https://opengameart.org/content/75-cc0-breaking-falling-hit-sfx", zip: "https://opengameart.org/sites/default/files/sfx_breaking_and_falling.zip", megabytes: 1.6 },
    SfxPack { id: "oga-retro-512", name: "Retro game (512)", provider: "opengameart", creator: "SubspaceAudio", page: "https://opengameart.org/content/512-sound-effects-8-bit-style", zip: "https://opengameart.org/sites/default/files/The%20Essential%20Retro%20Video%20Game%20Sound%20Effects%20Collection%20%5B512%20sounds%5D.zip", megabytes: 20.6 },
];

/// One sound from a pack, unpacked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sound {
    /// "Jingles / jingles_NES00" style: the file name without extension, with
    /// the folder inside the zip when it says something.
    pub name: String,
    pub path: PathBuf,
    pub pack: String,
}

impl SfxPack {
    pub fn by_id(id: &str) -> Option<&'static SfxPack> {
        SFX_PACKS.iter().find(|p| p.id == id)
    }

    fn dir(&self, root: &Path) -> PathBuf {
        root.join(self.provider).join(self.id)
    }

    /// Whether the pack is unpacked under `root` already.
    pub fn is_ready(&self, root: &Path) -> bool {
        self.dir(root).join(INDEX).exists()
    }

    fn origin(&self, sound: &str) -> Origin {
        let site = if self.provider == "kenney" {
            "kenney.nl"
        } else {
            "opengameart.org"
        };
        Origin {
            title: sound.to_string(),
            source_id: format!("{}/{sound}", self.id),
            creator: self.creator.into(),
            source_url: self.page.into(),
            licence: licence::spdx("CC0-1.0"),
            credit: format!(
                "\"{sound}\" from \"{}\" by {} ({site}), CC0",
                self.name, self.creator
            ),
            ..Origin::new(OriginKind::Library, self.provider)
        }
    }
}

const INDEX: &str = "sounds.json";
const AUDIO: [&str; 4] = ["ogg", "wav", "mp3", "flac"];

/// The current zip link on a Kenney asset page.
fn kenney_link(page: &str) -> Option<String> {
    let html = String::from_utf8(net::get(page).ok()?).ok()?;
    let start = html.find("https://kenney.nl/media/pages/assets/")?;
    let end = html[start..].find(".zip")? + start + 4;
    Some(html[start..end].to_string())
}

/// The pack's sounds, downloading and unpacking it under `root` the first
/// time.
pub fn open_pack(pack: &SfxPack, root: &Path) -> Result<Vec<Sound>, String> {
    let dir = pack.dir(root);
    let index = dir.join(INDEX);
    if let Ok(bytes) = std::fs::read(&index) {
        if let Ok(sounds) = serde_json::from_slice::<Vec<Sound>>(&bytes) {
            if sounds.iter().all(|s| s.path.exists()) {
                return Ok(sounds);
            }
        }
    }
    let zip_path = dir.join("pack.zip");
    let what = format!("the sound pack \"{}\"", pack.name);
    if let Err(error) = net::download(pack.zip, &zip_path) {
        let fallback = (pack.provider == "kenney")
            .then(|| kenney_link(pack.page))
            .flatten()
            .filter(|link| link != pack.zip);
        match fallback {
            Some(link) => {
                net::download(&link, &zip_path).map_err(|e| net::unreachable(&what, &e))?;
            }
            None => return Err(net::unreachable(&what, &error)),
        }
    }
    let sounds = unpack(pack, &zip_path, &dir);
    let _ = std::fs::remove_file(&zip_path);
    let sounds = sounds?;
    std::fs::write(
        &index,
        serde_json::to_vec_pretty(&sounds).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("cannot write the pack's index: {e}"))?;
    Ok(sounds)
}

fn unpack(pack: &SfxPack, zip_path: &Path, dir: &Path) -> Result<Vec<Sound>, String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("the pack is not a zip ({e})"))?;
    let mut sounds = Vec::new();
    let mut used = std::collections::HashSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        // `enclosed_name` refuses `../` and absolute paths: a zip never
        // writes outside the pack's directory.
        let Some(inner) = entry.enclosed_name() else {
            continue;
        };
        let Some(ext) = inner
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
        else {
            continue;
        };
        if !AUDIO.contains(&ext.as_str()) || inner.components().any(|c| c.as_os_str() == "__MACOSX")
        {
            continue;
        }
        let stem = inner
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if stem.eq_ignore_ascii_case("preview") {
            continue;
        }
        // A folder inside the zip that is not just "Audio" names a group.
        let folder = inner
            .parent()
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().to_string())
            .filter(|f| !f.eq_ignore_ascii_case("audio") && !f.is_empty());
        let name = match &folder {
            Some(folder) => format!("{folder} / {stem}"),
            None => stem.clone(),
        };
        // Two sounds whose names slug alike get their own directories.
        let slug = provenance::slug(&name);
        let mut slug_unique = slug.clone();
        let mut n = 2;
        while !used.insert(slug_unique.clone()) {
            slug_unique = format!("{slug}-{n}");
            n += 1;
        }
        let path = dir
            .join(&slug_unique)
            .join(format!("{}.{ext}", provenance::slug(&stem)));
        std::fs::create_dir_all(path.parent().unwrap_or(dir)).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("the pack is damaged ({e})"))?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        provenance::write_sidecar(&path, &pack.origin(&name))?;
        sounds.push(Sound {
            name,
            path,
            pack: pack.id.to_string(),
        });
    }
    if sounds.is_empty() {
        return Err(format!("\"{}\" holds no sounds", pack.name));
    }
    sounds.sort_by_key(|s| s.name.to_lowercase());
    Ok(sounds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cloud::http::test_server;
    use std::io::Write as _;

    #[test]
    fn the_curated_list_is_by_mood_and_credits_the_composer() {
        assert_eq!(curated(None).len(), CURATED.len());
        for mood in MOODS {
            assert_eq!(curated(Some(mood)).len(), 10, "{mood}");
        }
        let track = &curated(Some("Funny"))[0];
        assert!(track
            .credit()
            .starts_with("\"Sneaky Snitch\" Kevin MacLeod (incompetech.com)"));
        let origin = track.origin();
        assert_eq!(origin.licence.id, "CC-BY-4.0");
        assert!(origin.licence.attribution_required);
        assert_eq!(origin.credit_line(), track.credit());
    }

    #[test]
    fn the_catalogue_reads_pieces_json() {
        let dir = super::super::scratch("sounds-catalogue");
        let body = br#"[{"title":"The Britons","filename":"The Britons.mp3","length":"00:05:07","bpm":"180","feel":"Ren Faire, Medieval"},
                        {"title":"Broken","filename":"","length":"00:00:01"}]"#;
        let server = test_server::serve(vec![(200, "application/json", body.to_vec())]);
        let source = MusicSource {
            base: server.url.clone(),
        };
        let (tracks, _) = catalogue(&source, &dir).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].seconds, 307);
        assert_eq!(tracks[0].mood, "Ren Faire");
        assert_eq!(search(&tracks, "medieval").len(), 1);
        assert!(search(&tracks, "jazz").is_empty());
    }

    #[test]
    fn a_track_downloads_once_with_its_record() {
        let root = super::super::scratch("sounds-track");
        let server = test_server::serve(vec![(200, "audio/mpeg", b"ID3".to_vec())]);
        let source = MusicSource {
            base: server.url.clone(),
        };
        let track = &curated(Some("Calm"))[6];
        let path = fetch_track(&source, track, &root).unwrap();
        assert!(path.ends_with("incompetech/sunshine/sunshine.mp3"));
        assert_eq!(
            server.requests.lock().unwrap()[0].request_line,
            "GET /mp3-royaltyfree/Sunshine%20A.mp3 HTTP/1.1"
        );
        assert_eq!(
            provenance::read_sidecar(&path).unwrap().provider,
            "incompetech"
        );
        assert_eq!(fetch_track(&source, track, &root).unwrap(), path);
    }

    fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut out);
            for (name, bytes) in files {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        out.into_inner()
    }

    #[test]
    fn a_pack_unpacks_into_one_licensed_file_per_sound() {
        let root = super::super::scratch("sounds-pack");
        let zip = zip_of(&[
            ("License.txt", b"CC0"),
            ("Audio/click_001.ogg", b"OggS1"),
            ("Audio/8-Bit jingles/jingles_NES00.ogg", b"OggS2"),
            ("Preview.ogg", b"OggS3"),
            ("../escape.ogg", b"evil"),
        ]);
        let server = test_server::serve(vec![(200, "application/zip", zip)]);
        let zip_url: &'static str = Box::leak(format!("{}/pack.zip", server.url).into_boxed_str());
        let pack = SfxPack {
            zip: zip_url,
            ..SFX_PACKS[0]
        };
        let sounds = open_pack(&pack, &root).unwrap();
        let names: Vec<&str> = sounds.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["8-Bit jingles / jingles_NES00", "click_001"]);
        for sound in &sounds {
            assert!(sound.path.starts_with(&root));
            let origin = provenance::read_sidecar(&sound.path).unwrap();
            assert_eq!(origin.licence.id, "CC0-1.0");
            assert!(!origin.licence.attribution_required);
        }
        assert!(!root.join("kenney/escape.ogg").exists());
        assert!(pack.is_ready(&root));
        // Opened again: from the index, no request.
        assert_eq!(open_pack(&pack, &root).unwrap(), sounds);
    }
}
