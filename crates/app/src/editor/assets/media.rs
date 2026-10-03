//! The Media and Audio tabs: imported files as tiles, their poster frames,
//! and the two ways onto the timeline — the "+" on a tile, or a drag.

use std::collections::HashSet;

use chukcut_engine::modules::media::thumbnail_strip;
use chukcut_engine::modules::project::{new_id, Segment, TimeRange, Transform};
use gpui::{img, ObjectFit};

use super::*;
use crate::ui::{Badge, Tone};

/// What a tile carries while it is dragged towards the timeline.
#[derive(Clone)]
pub(crate) struct MediaDrag {
    pub material_id: String,
    pub name: String,
}

impl Render for MediaDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(R_SM))
            .bg(rgb(OVERLAY))
            .border_1()
            .border_color(rgb(ACCENT))
            .shadow_md()
            .text_size(px(TEXT_LABEL))
            .text_color(rgb(TEXT))
            .child(self.name.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Video,
    Image,
    Audio,
}

struct Item {
    id: String,
    name: String,
    path: String,
    kind: Kind,
    duration: Micros,
}

impl Editor {
    /// Every material in the pool of the given kinds, in import order.
    fn library(&self, kinds: &[Kind]) -> Vec<Item> {
        let pool = &self.project.materials;
        let mut items = Vec::new();
        if kinds.contains(&Kind::Video) {
            items.extend(pool.videos.iter().map(|m| Item {
                id: m.id.clone(),
                name: file_name(&m.path),
                path: m.path.clone(),
                kind: Kind::Video,
                duration: m.duration,
            }));
        }
        if kinds.contains(&Kind::Image) {
            items.extend(pool.images.iter().map(|m| Item {
                id: m.id.clone(),
                name: file_name(&m.path),
                path: m.path.clone(),
                kind: Kind::Image,
                duration: 0,
            }));
        }
        if kinds.contains(&Kind::Audio) {
            items.extend(pool.audios.iter().map(|m| Item {
                id: m.id.clone(),
                name: file_name(&m.path),
                path: m.path.clone(),
                kind: Kind::Audio,
                duration: m.duration,
            }));
        }
        items
    }

    /// The materials some clip on the timeline uses.
    fn used_materials(&self) -> HashSet<String> {
        self.project
            .tracks
            .iter()
            .flat_map(|track| track.segments.iter())
            .map(|segment| segment.material_id.clone())
            .collect()
    }

    /// The picture of a tile. A video's poster frame is decoded once, in
    /// the background, through the engine's thumbnail cache.
    fn tile_picture(&mut self, item: &Item, cx: &mut Context<Self>) -> AnyElement {
        let placeholder = |icon: IconSrc, background: u32, tint: u32| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(background))
                .child(icon.svg(22.0, rgb(tint)))
                .into_any_element()
        };
        let film = || placeholder(icons::MEDIA.into(), PANEL_RAISED, TEXT_MUTED);
        match item.kind {
            Kind::Image => img(PathBuf::from(&item.path))
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            Kind::Audio => placeholder(icons::AUDIO.into(), CLIP_AUDIO, CLIP_AUDIO_WAVE),
            Kind::Video => match self.assets.thumbs.get(&item.id) {
                Some(Thumb::Ready(path)) => img(path.clone())
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                Some(Thumb::Failed) | Some(Thumb::Loading) => film(),
                None => {
                    self.assets.thumbs.insert(item.id.clone(), Thumb::Loading);
                    let (id, path) = (item.id.clone(), item.path.clone());
                    cx.spawn(async move |this, cx| {
                        let strip = cx
                            .background_executor()
                            .spawn(async move { thumbnail_strip(&path, 1, 144) })
                            .await;
                        let thumb = match strip {
                            Ok(paths) if !paths.is_empty() => {
                                Thumb::Ready(PathBuf::from(&paths[0]))
                            }
                            Ok(_) => Thumb::Failed,
                            Err(error) => {
                                tracing::warn!(%error, "no poster frame for a media tile");
                                Thumb::Failed
                            }
                        };
                        let _ = this.update(cx, |editor, cx| {
                            editor.assets.thumbs.insert(id, thumb);
                            cx.notify();
                        });
                    })
                    .detach();
                    film()
                }
            },
        }
    }

    fn media_tile(&mut self, item: Item, used: bool, cx: &mut Context<Self>) -> AnyElement {
        let picture = self.tile_picture(&item, cx);
        let picked = self.assets.picked.as_deref() == Some(item.id.as_str());
        let badge = (item.duration > 0).then(|| {
            div().absolute().left(px(5.0)).bottom(px(5.0)).child(
                Badge::new(badge_time(item.duration))
                    .tone(Tone::OnMedia)
                    .mono(),
            )
        });
        // A file that is already on the timeline says so.
        let added = used.then(|| {
            div()
                .absolute()
                .top(px(5.0))
                .left(px(5.0))
                .child(Badge::new("Added").tone(Tone::OnMedia).icon(icons::CHECK))
        });
        let picture = div()
            .size_full()
            .relative()
            .child(picture)
            .children(badge)
            .children(added)
            .into_any_element();

        let add_id = item.id.clone();
        let editor = cx.entity().downgrade();
        let pick_id = item.id.clone();
        Self::tile(
            SharedString::from(format!("media-{}", item.id)),
            picture,
            item.name.clone(),
            picked,
            move |_, _, cx| {
                let id = add_id.clone();
                let _ = editor.update(cx, |this, cx| this.add_material(&id, cx));
            },
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.assets.picked = Some(pick_id.clone());
            cx.notify();
        }))
        .on_drag(
            MediaDrag {
                material_id: item.id.clone(),
                name: item.name.clone(),
            },
            |drag, _, _, cx| cx.new(|_| drag.clone()),
        )
        .into_any_element()
    }

    pub(super) fn render_media_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.assets.query(cx);
        let library = self.library(&[Kind::Video, Kind::Image, Kind::Audio]);
        if library.is_empty() && category == 0 {
            return self
                .drop_zone(
                    "media-drop-zone",
                    "Import",
                    "Drag videos, photos and audio here",
                    cx,
                )
                .into_any_element();
        }
        let used = self.used_materials();
        let items: Vec<Item> = library
            .into_iter()
            .filter(|item| category == 0 || used.contains(&item.id))
            .filter(|item| matches(&item.name, &query))
            .collect();
        if items.is_empty() {
            let (title, hint) = if category == 1 {
                (
                    "Nothing in use yet",
                    "Media you put on the timeline shows up here.",
                )
            } else {
                ("No matches", "No media matches the search.")
            };
            return EmptyState::new("media-empty", Lucide::Search, title)
                .hint(hint)
                .into_any_element();
        }

        let mut tiles = Vec::with_capacity(items.len() + 1);
        if category == 0 {
            tiles.push(self.import_tile(cx));
        }
        for item in items {
            let on_timeline = used.contains(&item.id);
            tiles.push(self.media_tile(item, on_timeline, cx));
        }
        Self::tile_area("media-grid")
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x(px(TILE_GAP))
                    .gap_y(px(12.0))
                    .pt(px(2.0))
                    .pl(px(2.0))
                    .children(tiles),
            )
            .into_any_element()
    }

    /// The first tile of the library: opens the file picker.
    fn import_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("media-import-tile")
            .w(px(TILE_W))
            .h(px(TILE_H))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .rounded(px(R_SM))
            .bg(rgb(WELL))
            .border_1()
            .border_color(rgb(BORDER))
            .cursor_pointer()
            .group("media-import-tile")
            .hover(|style| style.border_color(rgb(ACCENT)).bg(accent_drop()))
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.border_color(rgb(ACCENT)).bg(accent_drop())
            })
            .on_click(cx.listener(|this, _, window, cx| this.on_import(&Import, window, cx)))
            .child(
                icons::glyph(icons::IMPORT, 18.0, rgb(TEXT_DIM))
                    .group_hover("media-import-tile", |style| style.text_color(rgb(ACCENT))),
            )
            .child(
                div()
                    .text_size(px(TEXT_CAPTION))
                    .text_color(rgb(TEXT_DIM))
                    .group_hover("media-import-tile", |style| style.text_color(rgb(TEXT)))
                    .child("Import"),
            )
            .into_any_element()
    }

    pub(super) fn render_audio_tab(
        &mut self,
        category: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.assets.query(cx);
        let library = self.library(&[Kind::Audio]);
        if library.is_empty() && category == 0 {
            return self
                .drop_zone(
                    "audio-drop-zone",
                    "Import audio",
                    "Drag music and sound effects here",
                    cx,
                )
                .into_any_element();
        }
        let used = self.used_materials();
        let rows: Vec<AnyElement> = library
            .into_iter()
            .filter(|item| category == 0 || used.contains(&item.id))
            .filter(|item| matches(&item.name, &query))
            .map(|item| self.audio_row(item, &used, cx))
            .collect();
        if rows.is_empty() {
            let (title, hint) = if category == 1 {
                (
                    "Nothing in use yet",
                    "Audio you put on the timeline shows up here.",
                )
            } else {
                ("No matches", "No audio matches the search.")
            };
            return EmptyState::new("audio-empty", icons::AUDIO, title)
                .hint(hint)
                .into_any_element();
        }
        Self::tile_area("audio-list")
            .child(div().flex().flex_col().gap(px(6.0)).children(rows))
            .into_any_element()
    }

    fn audio_row(&self, item: Item, used: &HashSet<String>, cx: &mut Context<Self>) -> AnyElement {
        let id = item.id.clone();
        let picked = self.assets.picked.as_deref() == Some(item.id.as_str());
        let pick_id = item.id.clone();
        let on_timeline = used.contains(&item.id);
        let duration = badge_time(item.duration);
        div()
            .id(SharedString::from(format!("audio-{}", item.id)))
            .h(px(52.0))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .px(px(8.0))
            .rounded(px(R_SM))
            .bg(if picked {
                accent_soft()
            } else {
                hsla(PANEL_RAISED)
            })
            .border_1()
            .border_color(rgb(if picked { ACCENT } else { HAIRLINE }))
            .hover(|style| style.border_color(rgb(if picked { ACCENT } else { BORDER_STRONG })))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.assets.picked = Some(pick_id.clone());
                cx.notify();
            }))
            .on_drag(
                MediaDrag {
                    material_id: item.id.clone(),
                    name: item.name.clone(),
                },
                |drag, _, _, cx| cx.new(|_| drag.clone()),
            )
            .child(
                div()
                    .size(px(36.0))
                    .flex_none()
                    .rounded(px(R_SM))
                    .bg(rgb(CLIP_AUDIO))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icons::glyph(icons::AUDIO, 18.0, rgb(CLIP_AUDIO_WAVE))),
            )
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
                            .child(item.name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.0))
                            .text_size(px(TEXT_CAPTION))
                            .text_color(rgb(TEXT_MUTED))
                            .child(div().font_family(FONT_MONO).child(duration))
                            .when(on_timeline, |this| this.child("·").child("on the timeline")),
                    ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("audio-add-{}", item.id)))
                    .group(SharedString::from(format!("audio-add-group-{}", item.id)))
                    .size(px(26.0))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(rgb(OVERLAY))
                    .hover(|style| style.bg(rgb(ACCENT)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.add_material(&id, cx);
                    }))
                    .child(icons::glyph(icons::PLUS, 15.0, rgb(TEXT)).group_hover(
                        SharedString::from(format!("audio-add-group-{}", item.id)),
                        |style| style.text_color(rgb(ON_ACCENT)),
                    )),
            )
            .into_any_element()
    }

    /// The "+" on a tile: the material at the end of the first lane of its
    /// kind.
    fn add_material(&mut self, material_id: &str, cx: &mut Context<Self>) {
        let command = edits::append(&self.project, material_id);
        self.apply(command, cx);
    }

    /// Import files into the library without putting them on the timeline,
    /// which is what CapCut's Import does. (Files named on the command line
    /// still go straight onto the timeline: see `import_paths`.)
    pub(crate) fn import_to_library(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let state = Arc::clone(&self.state);
        self.status = Some("Importing…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut errors = Vec::new();
            let mut imported = 0;
            for path in paths {
                let path = path.to_string_lossy().to_string();
                match project_commands::project_import_media(&state, path.clone()).await {
                    Ok(_) => imported += 1,
                    Err(error) => errors.push(format!("{}: {error}", file_name(&path))),
                }
            }
            let _ = this.update(cx, |editor, cx| {
                editor.refresh(cx);
                editor.status = Some(if errors.is_empty() {
                    match imported {
                        1 => "Imported 1 file".into(),
                        n => format!("Imported {n} files").into(),
                    }
                } else {
                    errors.join(" · ").into()
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// A tile dropped somewhere in the window. On the timeline it lands
    /// where it was let go, on the lane under the pointer when that lane
    /// takes this kind of media; anywhere else nothing happens.
    pub(crate) fn on_media_drop(
        &mut self,
        drag: &MediaDrag,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((at, lane)) = self.drop_target(window.mouse_position()) else {
            return;
        };
        let command = drop_command(&self.project, &drag.material_id, lane.as_deref(), at);
        self.apply(command, cx);
    }
}

/// What dropping a material at `at` over `lane` does: an insert there, or an
/// append when that is refused. The timeline draws its drop ghost from this
/// same command, so the ghost is the result.
pub(crate) fn drop_command(
    project: &Project,
    material_id: &str,
    lane: Option<&str>,
    at: Micros,
) -> Result<EditCommand, String> {
    insert_at(project, material_id, lane, at).or_else(|_| edits::append(project, material_id))
}

/// Put a material at `at` on `lane` — or on the first unlocked lane of its
/// kind when `lane` does not take it. Refused where it would overlap a clip;
/// the caller then appends instead.
fn insert_at(
    project: &Project,
    material_id: &str,
    lane: Option<&str>,
    at: Micros,
) -> Result<EditCommand, String> {
    let pool = &project.materials;
    let (kind, duration) = if let Some(video) = pool.videos.iter().find(|m| m.id == material_id) {
        (TrackKind::Video, video.duration)
    } else if pool.images.iter().any(|m| m.id == material_id) {
        (TrackKind::Video, edits::STILL_DURATION)
    } else if let Some(audio) = pool.audios.iter().find(|m| m.id == material_id) {
        (TrackKind::Audio, audio.duration)
    } else {
        return Err(format!("unknown material {material_id}"));
    };
    if duration <= 0 {
        return Err("the material has no duration".into());
    }
    let track = lane
        .and_then(|id| project.track(id))
        .filter(|track| track.kind == kind && !track.locked)
        .or_else(|| {
            project
                .tracks
                .iter()
                .find(|track| track.kind == kind && !track.locked)
        })
        .ok_or("there is no unlocked lane for this kind of media")?;
    let start = at.max(0);
    let end = start + duration;
    let collides = track.segments.iter().any(|other| {
        start < other.target_range.start + other.target_range.duration
            && other.target_range.start < end
    });
    if collides {
        return Err("another clip is in the way".into());
    }
    let index = track
        .segments
        .iter()
        .filter(|s| s.target_range.start < start)
        .count();
    Ok(EditCommand::InsertSegment {
        track_id: track.id.clone(),
        index,
        segment: Segment {
            id: new_id(),
            material_id: material_id.to_string(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(0, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        },
    })
}
