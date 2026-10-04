//! The asset library from the command line: emoji and icon stickers, music
//! and sound effects onto the timeline, and the library's lists for
//! `catalog`.
//!
//! Everything here downloads on first use (Fluent/Noto emoji, Iconify,
//! Incompetech, the sound packs, Fontsource) and keeps the file and its
//! licence record in the library cache, as the app's asset panel does.

use std::path::PathBuf;

use chukcut_engine::modules::library::commands as library_commands;
use chukcut_engine::modules::library::fonts::FontCategory;
use chukcut_engine::modules::library::stickers::StickerStyle;
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{cloud::import_and_place, enum_named, Ctx, Operation, Outcome};
use crate::error::{CliError, CliResult};
use crate::session::{absolute, Session};
use crate::values::{seconds, Time};
use crate::On;

const STYLES: &[&str] = &["fluent3d", "fluent_flat", "noto"];
const CATEGORIES: &[&str] = &[
    "popular",
    "all",
    "sans_serif",
    "serif",
    "display",
    "handwriting",
    "monospace",
];

fn style(name: Option<&str>) -> CliResult<StickerStyle> {
    match name {
        None => Ok(StickerStyle::Fluent3d),
        Some(n) => enum_named("sticker style", n, STYLES),
    }
}

/// Put a sticker on an overlay lane, centred, for three seconds: an emoji
/// (by name or the emoji itself, in the Fluent 3D, Fluent flat or Noto
/// drawing), an animated emoji (Noto Animated Emoji, Lottie, CC BY 4.0), an
/// icon (`prefix:name` or a search word, from the icon sets the licence
/// policy allows), or a file of your own — a picture, or an animated sticker
/// (Lottie `.json`, animated GIF or WebP). Animated stickers loop; --once
/// plays them one time and holds the last frame. One undo step.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct StickerArgs {
    /// An emoji: its name ("red heart") or the emoji itself.
    #[arg(long)]
    pub emoji: Option<String>,
    /// An animated emoji: its name ("smile"), a tag, or the emoji itself, as
    /// `catalog animated_emoji` lists them.
    #[arg(long)]
    pub animated: Option<String>,
    /// An icon: `prefix:name` as `catalog icons` lists it, or a search word
    /// (the first hit is used).
    #[arg(long)]
    pub icon: Option<String>,
    /// An image file of your own.
    #[arg(long)]
    pub file: Option<PathBuf>,
    /// The emoji drawing: fluent3d (the default), fluent_flat or noto.
    #[arg(long)]
    pub style: Option<String>,
    /// Where it starts on the timeline. Default 0.
    #[arg(long)]
    pub at: Option<Time>,
    /// An animated sticker plays once and holds its last frame instead of
    /// looping.
    #[arg(long)]
    #[serde(default)]
    pub once: bool,
}

impl Operation for StickerArgs {
    const NAME: &'static str = "sticker";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let chosen = [
            self.emoji.is_some(),
            self.animated.is_some(),
            self.icon.is_some(),
            self.file.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        if chosen != 1 {
            return Err(CliError::usage(
                "sticker needs exactly one of --emoji, --animated, --icon or --file",
            ));
        }
        let (path, name) = if let Some(emoji) = &self.emoji {
            let style = style(self.style.as_deref())?;
            ctx.progress("Reading the emoji index", None);
            let index = library_commands::library_sticker_index()?;
            let hits = index.search(emoji, None, style);
            let wanted = emoji.trim().to_lowercase();
            let sticker = hits
                .iter()
                .find(|s| s.name == wanted || s.glyph == emoji.trim())
                .or_else(|| hits.first())
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "no emoji matches {emoji:?} in that style; `chukcut-cli catalog emoji --search ...` lists them"
                    ))
                })?;
            ctx.progress(&format!("Fetching {}", sticker.name), None);
            (
                library_commands::library_sticker_fetch(sticker, style)?,
                sticker.name.clone(),
            )
        } else if let Some(wanted) = &self.animated {
            ctx.progress("Reading the animated emoji list", None);
            let hits = library_commands::library_animated_search(wanted)?;
            let lower = wanted.trim().to_lowercase();
            let emoji = hits
                .iter()
                .find(|e| e.name == lower || e.glyph == wanted.trim())
                .or_else(|| hits.first())
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "no animated emoji matches {wanted:?}; `chukcut-cli catalog animated_emoji --search ...` lists them"
                    ))
                })?;
            ctx.progress(&format!("Fetching {}", emoji.name), None);
            (
                library_commands::library_animated_fetch(emoji)?,
                emoji.name.clone(),
            )
        } else if let Some(icon) = &self.icon {
            let wanted = icon.trim();
            let query = wanted.rsplit(':').next().unwrap_or(wanted);
            let hits = library_commands::library_icon_search(query)?;
            let hit = hits
                .iter()
                .find(|h| h.id() == wanted)
                .or_else(|| (!wanted.contains(':')).then(|| hits.first()).flatten())
                .ok_or_else(|| CliError::usage(format!("no icon matches {wanted:?}")))?;
            ctx.progress(&format!("Fetching {}", hit.id()), None);
            (library_commands::library_icon_fetch(hit)?, hit.id())
        } else {
            let file = absolute(self.file.as_deref().expect("checked above"));
            let name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (file, name)
        };
        let at = self.at.map_or(0, |t| t.resolve(session.fps()));
        let before: Vec<String> = session.with(|p| {
            p.tracks
                .iter()
                .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
                .collect()
        });
        pollster::block_on(library_commands::library_sticker_add(
            &session.state,
            &path,
            at,
        ))?;
        let added: Option<String> = session.with(|p| {
            p.tracks
                .iter()
                .flat_map(|t| t.segments.iter())
                .find(|s| !before.contains(&s.id))
                .map(|s| s.id.clone())
        });
        if self.once {
            let id = added
                .clone()
                .ok_or_else(|| CliError::usage("the sticker did not land on the timeline"))?;
            chukcut_engine::modules::animated::commands::animated_set_playback(
                &session.state,
                id,
                chukcut_engine::modules::animated::playback::Playback::Once,
            )?;
        }
        let clip = session.with(|p| {
            p.tracks
                .iter()
                .enumerate()
                .flat_map(|(t, track)| {
                    track
                        .segments
                        .iter()
                        .enumerate()
                        .map(move |(i, s)| (t, i, s))
                })
                .find(|(_, _, s)| !before.contains(&s.id))
                .map(|(t, i, s)| super::summary::clip(p, t, i, s))
        });
        Ok(Outcome::changed(
            format!("put the {name} sticker at {:.3} s", seconds(at)),
            json!({"path": path, "clip": clip}),
        ))
    }
}

/// Put a music track from the library on the timeline: one of the curated
/// tracks or any of Incompetech's catalogue (CC BY 4.0; the credit goes in
/// the credits file). The first track whose title matches is used.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct MusicArgs {
    /// The track's title, or words from it.
    pub title: String,
    /// Where it starts on the timeline. Default: imported, not placed.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for MusicArgs {
    const NAME: &'static str = "music";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        let wanted = self.title.trim().to_lowercase();
        let curated = library_commands::library_music_curated(None);
        let track = match curated.iter().find(|t| t.title.to_lowercase() == wanted) {
            Some(t) => t.clone(),
            None => {
                ctx.progress("Searching the music catalogue", None);
                let (hits, _) = library_commands::library_music_search(&self.title)?;
                hits.iter()
                    .find(|t| t.title.to_lowercase() == wanted)
                    .or_else(|| hits.first())
                    .cloned()
                    .ok_or_else(|| {
                        CliError::usage(format!(
                            "no track matches {:?}; `chukcut-cli catalog music --search ...` lists them",
                            self.title
                        ))
                    })?
            }
        };
        if !library_commands::library_music_cached(&track) {
            ctx.progress(&format!("Downloading {}", track.title), None);
        }
        let path = library_commands::library_music_fetch(&track)?;
        let (material, clip) = import_and_place(session, &path, self.at)?;
        Ok(Outcome::changed(
            format!("added {}", track.title),
            json!({"track": track, "path": path, "material": material, "clip": clip}),
        ))
    }
}

/// Put a sound effect from one of the CC0 sound packs on the timeline. The
/// pack is downloaded the first time it is opened.
#[derive(Debug, Clone, Default, Args, Deserialize, JsonSchema)]
pub struct SfxArgs {
    /// The pack's id (`catalog sfx` lists them).
    #[arg(long)]
    pub pack: String,
    /// The sound's name, or words from it (`catalog sfx --filter <pack>`).
    pub sound: String,
    /// Where it starts on the timeline. Default: imported, not placed.
    #[arg(long)]
    pub at: Option<Time>,
}

impl Operation for SfxArgs {
    const NAME: &'static str = "sfx";
    fn run(self, session: &mut Session, ctx: &Ctx) -> CliResult<Outcome> {
        ctx.progress(&format!("Opening the {} pack", self.pack), None);
        let sounds = library_commands::library_sfx_open(self.pack.trim())?;
        let wanted = self.sound.trim().to_lowercase();
        let sound = sounds
            .iter()
            .find(|s| s.name.to_lowercase() == wanted)
            .or_else(|| sounds.iter().find(|s| s.name.to_lowercase().contains(&wanted)))
            .ok_or_else(|| {
                CliError::usage(format!(
                    "the pack has no sound like {:?}; `chukcut-cli catalog sfx --filter {}` lists them",
                    self.sound, self.pack
                ))
            })?;
        let (material, clip) = import_and_place(session, &sound.path, self.at)?;
        Ok(Outcome::changed(
            format!("added {}", sound.name),
            json!({"sound": sound, "material": material, "clip": clip}),
        ))
    }
}

/// The library's lists for `catalog`: `None` when `kind` is not one of
/// them.
pub fn catalog(kind: &str, search: Option<&str>, filter: Option<&str>) -> Option<CliResult<Value>> {
    let search = search.map(str::trim).filter(|s| !s.is_empty());
    let filter = filter.map(str::trim).filter(|s| !s.is_empty());
    Some(match kind {
        "font_catalogue" | "fontsource" => (|| {
            let category = match filter {
                Some(c) => enum_named::<FontCategory>("font category", c, CATEGORIES)?,
                None => FontCategory::All,
            };
            match search {
                Some(q) => Ok(json!(library_commands::library_font_search(q, category)?)),
                None => {
                    let list = library_commands::library_font_catalogue()?;
                    Ok(json!(list.fonts))
                }
            }
        })(),
        "fonts_installed" => Ok(json!(library_commands::library_fonts_installed())),
        "fonts_system" => Ok(json!(library_commands::library_system_fonts())),
        "animated_emoji" | "animated" => (|| {
            let list = match search {
                Some(q) => library_commands::library_animated_search(q)?,
                None => library_commands::library_animated_index()?.0.clone(),
            };
            Ok(json!(list))
        })(),
        "emoji" | "stickers" => (|| {
            let style = style(filter)?;
            let index = library_commands::library_sticker_index()?;
            Ok(json!(index.search(search.unwrap_or(""), None, style)))
        })(),
        "icons" => match search {
            Some(q) => library_commands::library_icon_search(q)
                .map(|hits| {
                    json!(hits
                        .iter()
                        .map(|h| json!({"id": h.id(), "set": h.set}))
                        .collect::<Vec<_>>())
                })
                .map_err(CliError::refused),
            None => Err(CliError::usage("catalog icons needs --search")),
        },
        "music" => (|| {
            let tracks = match search {
                Some(q) => library_commands::library_music_search(q)?.0,
                None => library_commands::library_music_curated(filter),
            };
            Ok(json!(tracks
                .into_iter()
                .map(|t| {
                    let cached = library_commands::library_music_cached(&t);
                    let mut v = json!(t);
                    v["downloaded"] = json!(cached);
                    v
                })
                .collect::<Vec<_>>()))
        })(),
        "sfx" => match filter {
            Some(pack) => library_commands::library_sfx_open(pack)
                .map(|sounds| {
                    let sounds = match search {
                        Some(q) => {
                            let q = q.to_lowercase();
                            sounds
                                .into_iter()
                                .filter(|s| s.name.to_lowercase().contains(&q))
                                .collect()
                        }
                        None => sounds,
                    };
                    json!(sounds)
                })
                .map_err(CliError::refused),
            None => Ok(json!(library_commands::library_sfx_packs()
                .iter()
                .map(|p| {
                    let mut v = json!(p);
                    v["downloaded"] = json!(library_commands::library_sfx_ready(p));
                    v
                })
                .collect::<Vec<_>>())),
        },
        "looks" => library_commands::library_looks_install()
            .map(|_| json!(library_commands::library_looks()))
            .map_err(CliError::refused),
        "library_settings" => Ok(json!(library_commands::library_settings())),
        _ => return None,
    })
}

/// Top-level subcommands for the library.
#[derive(Subcommand)]
pub enum LibraryCommand {
    /// Put an emoji, icon or image sticker on the timeline.
    Sticker(On<StickerArgs>),
    /// Put a library music track on the timeline.
    Music(On<MusicArgs>),
    /// Put a sound effect from a library pack on the timeline.
    Sfx(On<SfxArgs>),
}

impl LibraryCommand {
    pub fn dispatch(self, dry: bool, ctx: &Ctx) -> CliResult<(&'static str, Outcome, bool)> {
        match self {
            Self::Sticker(o) => crate::on(o, dry, ctx),
            Self::Music(o) => crate::on(o, dry, ctx),
            Self::Sfx(o) => crate::on(o, dry, ctx),
        }
    }
}
