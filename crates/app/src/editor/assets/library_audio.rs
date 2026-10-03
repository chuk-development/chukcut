//! The Audio tab's library categories: music (Incompetech, CC BY 4.0) and
//! sound effects (CC0 packs).
//!
//! "Play" fetches a track once into the library cache with its licence
//! record and plays it through the audition player; "Add" puts the same file
//! at the playhead. The credit line travels with the file into the project
//! and from there into the credits file the export writes.

use chukcut_engine::modules::library::commands as library;
use chukcut_engine::modules::library::sounds::{Sound, Track, MOODS};
use gpui::component::button::{Button, ButtonVariants as _};
use gpui::component::{Disableable as _, Sizable as _};

use super::library_panel::{chip, chips, notice, PackState};
use super::*;
use crate::editor::cloud::Placement;
use crate::ui::{Badge, Tone};

/// The Audio tab's library categories, after "Import" and "Project audio".
pub(super) const AUDIO_LIBRARY: [&str; 2] = ["Music library", "Sound library"];

fn mmss(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

impl Editor {
    /// One of [`AUDIO_LIBRARY`], by index.
    pub(super) fn render_audio_library(
        &mut self,
        which: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match which {
            0 => self.render_music_library(cx),
            _ => self.render_sound_library(cx),
        }
    }

    fn render_music_library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.assets.library.music_chip;
        let all = MOODS.len();
        let mood_chips = chips(
            MOODS
                .iter()
                .map(|m| m.to_string())
                .chain(["All tracks".to_string()])
                .enumerate()
                .map(|(i, label)| {
                    chip(
                        ("music-mood", i),
                        label,
                        i == chosen,
                        cx.listener(move |this, _, _, cx| {
                            this.assets.library.music_chip = i;
                            cx.notify();
                        }),
                    )
                }),
        );
        let query = self.assets.query(cx).unwrap_or_default();
        let (tracks, note): (Vec<Track>, Option<(String, u32)>) = if chosen < all {
            let tracks = library::library_music_curated(Some(MOODS[chosen]))
                .into_iter()
                .filter(|t| matches(&t.title, &Some(query.clone())) || query.is_empty())
                .collect();
            (tracks, None)
        } else {
            self.load_music_catalogue(cx);
            match &self.assets.library.music_all {
                None => (
                    Vec::new(),
                    Some(("Loading the music list\u{2026}".into(), TEXT_MUTED)),
                ),
                Some(Err(error)) => (Vec::new(), Some((error.clone(), DANGER))),
                Some(Ok((list, stale))) => {
                    let found = chukcut_engine::modules::library::sounds::search(list, &query);
                    let note = stale.then(|| {
                        (
                            "Offline: the list is from the last time; downloaded tracks still play.".to_string(),
                            TEXT_MUTED,
                        )
                    });
                    (found.into_iter().take(200).collect(), note)
                }
            }
        };
        let rows: Vec<AnyElement> = tracks
            .into_iter()
            .map(|track| self.track_row(track, cx))
            .collect();
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(mood_chips)
            .child(Self::hint(
                "Kevin MacLeod (incompetech.com), CC BY 4.0. Export writes the credit.",
            ))
            .children(note.map(|(text, tone)| notice(text, tone)))
            .when(
                chosen == all && matches!(self.assets.library.music_all, Some(Err(_))),
                |column| {
                    column.child(chips([chip(
                        "music-retry",
                        "Try again",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.assets.library.music_all = None;
                            cx.notify();
                        }),
                    )]))
                },
            )
            .child(
                Self::tile_area("music-list")
                    .child(div().flex().flex_col().gap(px(6.0)).children(rows)),
            )
            .into_any_element()
    }

    fn load_music_catalogue(&mut self, cx: &mut Context<Self>) {
        let library = &mut self.assets.library;
        if library.music_all.is_some() || library.music_loading {
            return;
        }
        library.music_loading = true;
        self.library_task(
            cx,
            || library::library_music_search(""),
            |editor, result, _| {
                editor.assets.library.music_loading = false;
                editor.assets.library.music_all = Some(result);
            },
        );
    }

    fn track_row(&mut self, track: Track, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("music:{}", track.file);
        let busy = self.assets.library.busy.contains(&key);
        let playing = self.is_auditioning(&key);
        let downloaded = library::library_music_cached(&track);
        let meta = if track.bpm > 0 {
            format!(
                "{} \u{b7} {} bpm \u{b7} {}",
                mmss(track.seconds),
                track.bpm,
                track.mood
            )
        } else {
            format!("{} \u{b7} {}", mmss(track.seconds), track.mood)
        };
        let (play_track, add_track) = (track.clone(), track.clone());
        let (play_key, add_key) = (key.clone(), key.clone());
        audio_row(
            SharedString::from(key.clone()),
            track.title.clone(),
            meta,
            Badge::new("Credit needed").tone(Tone::Accent),
            downloaded,
            busy,
            playing,
            cx.listener(move |this, _, _, cx| {
                let track = play_track.clone();
                this.fetch_audio(
                    play_key.clone(),
                    move || library::library_music_fetch(&track),
                    false,
                    cx,
                )
            }),
            cx.listener(move |this, _, _, cx| {
                let track = add_track.clone();
                this.fetch_audio(
                    add_key.clone(),
                    move || library::library_music_fetch(&track),
                    true,
                    cx,
                )
            }),
        )
    }

    /// Fetch an audio file (once), then play it or put it at the playhead.
    fn fetch_audio(
        &mut self,
        key: String,
        fetch: impl FnOnce() -> Result<PathBuf, String> + Send + 'static,
        add: bool,
        cx: &mut Context<Self>,
    ) {
        if !add && self.is_auditioning(&key) {
            chukcut_engine::modules::cloud::audition::stop();
            self.assets.library.playing = None;
            cx.notify();
            return;
        }
        if !self.assets.library.busy.insert(key.clone()) {
            return;
        }
        let at = self.clock.position();
        self.library_task(cx, fetch, move |editor, result, cx| {
            editor.assets.library.busy.remove(&key);
            match result {
                Ok(path) if add => editor.cloud_import(path, Placement::At(at), Vec::new(), cx),
                Ok(path) => editor.audition(key, path, cx),
                Err(error) => editor.report(Err(error), cx),
            }
        });
        cx.notify();
    }

    fn render_sound_library(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.assets.library.open_pack;
        let pack_chips = chips(library::library_sfx_packs().iter().map(|pack| {
            let id = pack.id;
            chip(
                SharedString::from(format!("sfx-pack-{id}")),
                pack.name,
                id == open,
                cx.listener(move |this, _, _, cx| {
                    this.assets.library.open_pack = id;
                    cx.notify();
                }),
            )
        }));
        let pack = library::library_sfx_packs()
            .iter()
            .find(|p| p.id == open)
            .copied()
            .unwrap_or(library::library_sfx_packs()[0]);
        let about = format!(
            "\"{}\" by {} ({}), CC0: no credit needed.",
            pack.name,
            pack.creator,
            if pack.provider == "kenney" {
                "kenney.nl"
            } else {
                "opengameart.org"
            }
        );
        let query = self.assets.query(cx);
        let body: AnyElement = match self.assets.library.packs.get(open) {
            None => {
                let ready = library::library_sfx_ready(&pack);
                if ready {
                    self.open_sfx_pack(open, cx);
                    notice("Opening\u{2026}", TEXT_MUTED).into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(notice(
                            format!(
                                "This pack downloads once ({:.1} MB) and then works offline.",
                                pack.megabytes
                            ),
                            TEXT_MUTED,
                        ))
                        .child(
                            div().flex().child(
                                Button::new(SharedString::from(format!("sfx-get-{open}")))
                                    .label("Download pack")
                                    .small()
                                    .primary()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_sfx_pack(open, cx)
                                    })),
                            ),
                        )
                        .into_any_element()
                }
            }
            Some(PackState::Loading) => {
                notice("Downloading the pack\u{2026}", TEXT_MUTED).into_any_element()
            }
            Some(PackState::Failed(error)) => {
                let error = error.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(notice(error, DANGER))
                    .child(chip(
                        "sfx-retry",
                        "Try again",
                        false,
                        cx.listener(move |this, _, _, cx| {
                            this.assets.library.packs.remove(open);
                            this.open_sfx_pack(open, cx);
                        }),
                    ))
                    .into_any_element()
            }
            Some(PackState::Ready(sounds)) => {
                let sounds: Vec<Sound> = sounds
                    .iter()
                    .filter(|s| matches(&s.name, &query))
                    .cloned()
                    .collect();
                let rows: Vec<AnyElement> =
                    sounds.into_iter().map(|s| self.sound_row(s, cx)).collect();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(rows)
                    .into_any_element()
            }
        };
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(pack_chips)
            .child(Self::hint(about))
            .child(Self::tile_area("sfx-list").child(body))
            .into_any_element()
    }

    fn open_sfx_pack(&mut self, id: &'static str, cx: &mut Context<Self>) {
        if matches!(
            self.assets.library.packs.get(id),
            Some(PackState::Loading | PackState::Ready(_))
        ) {
            return;
        }
        self.assets.library.packs.insert(id, PackState::Loading);
        self.library_task(
            cx,
            move || library::library_sfx_open(id),
            move |editor, result, _| {
                let state = match result {
                    Ok(sounds) => PackState::Ready(sounds),
                    Err(error) => PackState::Failed(error),
                };
                editor.assets.library.packs.insert(id, state);
            },
        );
        cx.notify();
    }

    fn sound_row(&mut self, sound: Sound, cx: &mut Context<Self>) -> AnyElement {
        let key = format!("sfx:{}", sound.path.display());
        let playing = self.is_auditioning(&key);
        let (play_path, add_path) = (sound.path.clone(), sound.path.clone());
        let play_key = key.clone();
        audio_row(
            SharedString::from(key),
            sound.name.clone(),
            String::new(),
            Badge::new("Free to use").tone(Tone::Success),
            // Every sound of an open pack is on disk; the badge would say
            // nothing.
            false,
            false,
            playing,
            cx.listener(move |this, _, _, cx| {
                this.audition(play_key.clone(), play_path.clone(), cx)
            }),
            cx.listener(move |this, _, _, cx| {
                let at = this.clock.position();
                this.cloud_import(add_path.clone(), Placement::At(at), Vec::new(), cx)
            }),
        )
    }
}

/// One track or sound: name, details, licence, play and add.
#[allow(clippy::too_many_arguments)]
fn audio_row(
    id: SharedString,
    title: String,
    meta: String,
    licence: Badge,
    downloaded: bool,
    busy: bool,
    playing: bool,
    on_play: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_add: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id.clone())
        .px(px(8.0))
        .py(px(6.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .rounded(px(R_SM))
        .bg(rgb(PANEL_RAISED))
        .border_1()
        .border_color(rgb(if playing { ACCENT } else { HAIRLINE }))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(TEXT_BODY))
                        .text_color(rgb(TEXT))
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(licence)
                        .when(downloaded, |r| r.child(Badge::new("Offline")))
                        .when(!meta.is_empty(), |r| {
                            r.child(
                                div()
                                    .text_size(px(TEXT_CAPTION))
                                    .text_color(rgb(TEXT_MUTED))
                                    .font_family(FONT_MONO)
                                    .child(meta),
                            )
                        }),
                ),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-play")))
                .label(if busy {
                    "\u{2026}"
                } else if playing {
                    "Stop"
                } else {
                    "Play"
                })
                .xsmall()
                .ghost()
                .disabled(busy)
                .on_click(on_play),
        )
        .child(
            Button::new(SharedString::from(format!("{id}-add")))
                .label("Add")
                .xsmall()
                .disabled(busy)
                .on_click(on_add),
        )
        .into_any_element()
}
