//! The templates chukcut ships: built here, from our own parts.
//!
//! Every piece is ours — our title styles and text templates
//! (`text::presets`), our animations (`motion`), effects (`fx`), transitions
//! (`transitions`, including the MIT gl-transitions port), looks
//! (`library::looks`), placeholders and synthesised music (`assets`,
//! `music`). No ByteDance asset is involved, by construction.
//!
//! A template is assembled with the same edit builders the commands use and
//! applied through a `DocumentHistory` of its own, so it is exactly the
//! document a user clicking through the app would have made, and it stays
//! valid as those builders change. Building one touches no file; drawing the
//! placeholders and the music it names is `assets::ensure_for`'s job.

use crate::modules::fx::catalog as fx_catalog;
use crate::modules::fx::edit::{self as fx_edit, Corner, SplitLayout};
use crate::modules::inspector::edit as inspector_edit;
use crate::modules::library::looks;
use crate::modules::motion::edit as motion_edit;
use crate::modules::project::animation::{AnimationPreset, AnimationSlot, ClipAnimation, Ease};
use crate::modules::project::document::{
    new_id, AudioMaterial, CanvasConfig, ImageMaterial, LutRef, Micros, Project, Segment,
    TimeRange, Track, TrackKind, Transform, TransitionKind,
};
use crate::modules::text::{edit as text_edit, presets};
use crate::modules::timeline::ops::EditCommand;
use crate::modules::transitions::edit as transitions_edit;
use crate::modules::workspace::paths;
use crate::state::DocumentHistory;

use super::assets;
use super::format::TemplateFile;
use super::music::{self, Bed};
use super::slot::{SlotMarker, SlotMedia};

/// Bumped whenever a built-in changes, so their preview tiles are redrawn.
pub const BUILTIN_VERSION: u32 = 1;

/// Assembles one template.
struct Build {
    project: Project,
    history: DocumentHistory,
    main: String,
    next_slot: u32,
}

/// Where a slot goes.
struct SlotSpec<'a> {
    lane: &'a str,
    start: Micros,
    duration: Micros,
    aspect: [u32; 2],
    label: Option<&'a str>,
    accepts: SlotMedia,
    /// Whether the clip's own sound plays. Off under music: a template's
    /// cuts are timed to its track, and twenty phone clips' wind noise is
    /// not what anyone wanted under it.
    sound: bool,
}

impl Build {
    fn new(name: &str, width: u32, height: u32) -> Self {
        let mut project = Project::new(
            name,
            CanvasConfig {
                width,
                height,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        );
        project.canvas_chosen = true;
        let main = Track::new(TrackKind::Video, "Main");
        let id = main.id.clone();
        project.tracks.push(main);
        Self {
            project,
            history: DocumentHistory::new(),
            main: id,
            next_slot: 1,
        }
    }

    fn apply(&mut self, command: EditCommand) -> Result<(), String> {
        self.history.apply(&mut self.project, command)
    }

    /// A new lane on top of the others.
    fn lane(&mut self, kind: TrackKind, name: &str) -> Result<String, String> {
        let track = Track::new(kind, name);
        let id = track.id.clone();
        let index = self.project.tracks.len();
        self.apply(EditCommand::AddTrack { track, index })?;
        Ok(id)
    }

    /// The canvas's own shape, reduced.
    fn canvas_aspect(&self) -> [u32; 2] {
        super::slot::reduce_aspect(self.project.canvas.width, self.project.canvas.height)
    }

    /// A slot on `spec.lane`, showing its placeholder.
    fn slot(&mut self, spec: SlotSpec<'_>) -> Result<String, String> {
        let index = self.next_slot;
        self.next_slot += 1;
        let path = assets::placeholder_path(index, spec.aspect)
            .to_string_lossy()
            .into_owned();
        let (width, height) = assets::placeholder_size(spec.aspect);
        let pool = &mut self.project.materials;
        let material_id = match pool.images.iter().find(|m| m.path == path) {
            Some(existing) => existing.id.clone(),
            None => {
                let id = new_id();
                pool.images.push(ImageMaterial {
                    id: id.clone(),
                    path,
                    width,
                    height,
                });
                id
            }
        };
        let marker = SlotMarker {
            index,
            label: spec.label.map(str::to_string),
            aspect: spec.aspect,
            accepts: spec.accepts,
            filled: false,
        };
        let (marker_id, value) = marker.new_entry();
        pool.extras.insert(marker_id.clone(), value);

        let segment = Segment {
            id: new_id(),
            material_id,
            target_range: TimeRange::new(spec.start, spec.duration),
            source_range: TimeRange::new(0, spec.duration),
            render_index: 0,
            speed: 1.0,
            volume: if spec.sound { 1.0 } else { 0.0 },
            transform: Transform::default(),
            crop: None,
            extras: vec![marker_id],
            keyframes: Vec::new(),
        };
        let id = segment.id.clone();
        let track = self
            .project
            .track(spec.lane)
            .ok_or("a template lane went missing")?;
        let at = track
            .segments
            .iter()
            .filter(|s| s.target_range.start < spec.start)
            .count();
        self.apply(EditCommand::InsertSegment {
            track_id: spec.lane.to_string(),
            segment,
            index: at,
        })?;
        Ok(id)
    }

    /// `count` full-canvas slots of `beats` beats each, back to back on the
    /// main lane from `start`. Answers their ids.
    fn run(
        &mut self,
        start: Micros,
        count: usize,
        each: Micros,
        accepts: SlotMedia,
        labels: &[&str],
    ) -> Result<Vec<String>, String> {
        let aspect = self.canvas_aspect();
        let main = self.main.clone();
        (0..count)
            .map(|n| {
                self.slot(SlotSpec {
                    lane: &main,
                    start: start + n as Micros * each,
                    duration: each,
                    aspect,
                    label: labels.get(n).copied(),
                    accepts,
                    sound: false,
                })
            })
            .collect()
    }

    fn animate(
        &mut self,
        segment: &str,
        slot: AnimationSlot,
        preset: AnimationPreset,
        duration: Micros,
        easing: Ease,
    ) -> Result<(), String> {
        let command = motion_edit::set_slot_command(
            &self.project,
            segment,
            slot,
            Some(ClipAnimation {
                preset,
                duration,
                easing,
                strength: 1.0,
            }),
        )?;
        self.apply(command)
    }

    /// A library transition (`seamless:…`, `gl:…`) at the head of `segment`.
    fn transition(&mut self, segment: &str, preset: &str, duration: Micros) -> Result<(), String> {
        let command =
            transitions_edit::add_preset_command(&self.project, segment, preset, Some(duration))?;
        self.apply(command)
    }

    fn builtin_transition(
        &mut self,
        segment: &str,
        kind: TransitionKind,
        duration: Micros,
    ) -> Result<(), String> {
        let command = transitions_edit::add_command(&self.project, segment, kind, Some(duration))?;
        self.apply(command)
    }

    /// The same transition on every cut of `segments`.
    fn transitions(
        &mut self,
        segments: &[String],
        preset: &str,
        duration: Micros,
    ) -> Result<(), String> {
        for segment in segments.iter().skip(1) {
            self.transition(segment, preset, duration)?;
        }
        Ok(())
    }

    fn effect(&mut self, segment: &str, kind: &str) -> Result<(), String> {
        let (material, command) = fx_edit::add_command(&self.project, segment, kind)?;
        self.project.materials.effects.push(material);
        self.apply(command)
    }

    /// An effect clip over everything beneath it.
    fn effect_clip(&mut self, kind: &str, start: Micros, duration: Micros) -> Result<(), String> {
        let placed = fx_edit::clip_command(&self.project, kind, start, duration, None)?;
        self.project.materials.effects.push(placed.material);
        self.apply(placed.command)
    }

    /// One of our looks on `segment`, by name.
    fn look(&mut self, segment: &str, name: &str, intensity: f32) -> Result<(), String> {
        let look = looks::LOOKS
            .iter()
            .find(|l| l.name == name)
            .ok_or_else(|| format!("there is no look called {name}"))?;
        let path = looks::path_in(&paths::luts_dir(), look)
            .to_string_lossy()
            .into_owned();
        let (material, command) =
            inspector_edit::lut_command(&self.project, segment, Some(LutRef { path, intensity }))?;
        if let Some(material) = material {
            self.project.materials.color_adjusts.push(material);
        }
        self.apply(command)
    }

    /// A title in style `style` saying `text`, at `start` for `duration`,
    /// with an optional entrance. `position` overrides the style's place.
    fn title(
        &mut self,
        style: &str,
        text: &str,
        start: Micros,
        duration: Micros,
        position: Option<[f32; 2]>,
        intro: Option<(AnimationPreset, Micros)>,
    ) -> Result<String, String> {
        self.title_in(None, style, text, start, duration, position, intro)
    }

    /// [`Self::title`] on `lane`: two titles shown at once need two lanes.
    #[allow(clippy::too_many_arguments)]
    fn title_in(
        &mut self,
        lane: Option<&str>,
        style: &str,
        text: &str,
        start: Micros,
        duration: Micros,
        position: Option<[f32; 2]>,
        intro: Option<(AnimationPreset, Micros)>,
    ) -> Result<String, String> {
        let style = presets::style(style).ok_or_else(|| format!("no title style {style}"))?;
        let material = style.material(&self.project, Some(text.to_string()));
        let mut transform = style.transform();
        if let Some(position) = position {
            transform.position = position;
        }
        let mut placement =
            text_edit::insert_command_on(&self.project, &material.id, start, duration, lane)?;
        text_edit::dress_placement(&mut placement, transform, None, None);
        self.project.materials.texts.push(material);
        self.apply(placement.command)?;
        if let Some((preset, length)) = intro {
            self.animate(
                &placement.segment_id,
                AnimationSlot::In,
                preset,
                length,
                Ease::EaseOut,
            )?;
        }
        Ok(placement.segment_id)
    }

    /// A title from one of our text templates (a style and its motion).
    fn text_template(
        &mut self,
        template: &str,
        text: &str,
        start: Micros,
        duration: Micros,
    ) -> Result<String, String> {
        let template =
            presets::template(template).ok_or_else(|| format!("no text template {template}"))?;
        let style = template.title_style();
        let material = style.material(&self.project, Some(text.to_string()));
        let mut placement =
            text_edit::insert_command_on(&self.project, &material.id, start, duration, None)?;
        text_edit::dress_placement(
            &mut placement,
            style.transform(),
            Some(template.animation()),
            Some(template.name),
        );
        self.project.materials.texts.push(material);
        self.apply(placement.command)?;
        Ok(placement.segment_id)
    }

    /// The music bed under the whole template, `volume` loud.
    fn music(&mut self, bed: Bed, volume: f32) -> Result<(), String> {
        let length = self.project.duration();
        let seconds = ((length + 999_999) / 1_000_000).max(1) as u32;
        let path = assets::music_dir()
            .join(music::file_name(bed, seconds))
            .to_string_lossy()
            .into_owned();
        let material = AudioMaterial {
            id: new_id(),
            path,
            duration: seconds as Micros * 1_000_000,
            sample_rate: music::SAMPLE_RATE,
            channels: 2,
        };
        let lane = self.lane(TrackKind::Audio, "Music")?;
        let segment = Segment {
            id: new_id(),
            material_id: material.id.clone(),
            target_range: TimeRange::new(0, length),
            source_range: TimeRange::new(0, length),
            render_index: 0,
            speed: 1.0,
            volume,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };
        self.project.materials.audios.push(material);
        self.apply(EditCommand::InsertSegment {
            track_id: lane,
            segment,
            index: 0,
        })
    }

    fn finish(
        self,
        id: &str,
        description: &str,
        category: &str,
        cover_time: Option<Micros>,
    ) -> Result<TemplateFile, String> {
        let name = self.project.name.clone();
        let mut file = TemplateFile::new(id.to_string(), name, &self.project)?;
        file.description = description.to_string();
        file.category = category.to_string();
        file.cover_time = cover_time;
        Ok(file)
    }
}

const SEC: Micros = 1_000_000;

fn quick_cuts() -> Result<TemplateFile, String> {
    let mut b = Build::new("Quick Cuts", 1080, 1920);
    let beat = Bed::Pulse.beat();
    let slots = b.run(0, 6, beat * 2, SlotMedia::Any, &["Opening shot"])?;
    b.transitions(&slots, "seamless:whip", 250_000)?;
    b.animate(
        &slots[5],
        AnimationSlot::Combo,
        AnimationPreset::Pulse,
        beat,
        Ease::EaseInOut,
    )?;
    b.title(
        "big-impact",
        "TODAY",
        0,
        beat * 4,
        None,
        Some((AnimationPreset::Pop, 300_000)),
    )?;
    b.music(Bed::Pulse, 0.9)?;
    b.finish(
        "quick-cuts",
        "Six fast cuts on the beat with whip pans and a punchy title.",
        "Social",
        Some(beat * 3),
    )
}

fn travel_diary() -> Result<TemplateFile, String> {
    let mut b = Build::new("Travel Diary", 1080, 1920);
    let beat = Bed::Chill.beat();
    let slots = b.run(
        0,
        4,
        beat * 4,
        SlotMedia::Any,
        &["Arrival", "The view", "Street life", "Sunset"],
    )?;
    for slot in &slots {
        b.look(slot, "Teal & Orange", 0.8)?;
        b.animate(
            slot,
            AnimationSlot::In,
            AnimationPreset::ZoomOut,
            800_000,
            Ease::EaseOut,
        )?;
    }
    for slot in slots.iter().skip(1) {
        b.builtin_transition(slot, TransitionKind::Dissolve, 500_000)?;
    }
    b.title(
        "elegant",
        "Somewhere new",
        200_000,
        beat * 4,
        None,
        Some((AnimationPreset::Fade, 600_000)),
    )?;
    b.music(Bed::Chill, 0.8)?;
    b.finish(
        "travel-diary",
        "Four slow shots with a warm look, soft dissolves and an elegant title.",
        "Travel & vlog",
        Some(beat * 2),
    )
}

fn product_spotlight() -> Result<TemplateFile, String> {
    let mut b = Build::new("Product Spotlight", 1080, 1080);
    let beat = Bed::Bright.beat();
    let slots = b.run(
        0,
        3,
        beat * 4,
        SlotMedia::Any,
        &["The product", "A detail", "In use"],
    )?;
    b.transitions(&slots, "seamless:push", 300_000)?;
    b.effect(&slots[1], fx_catalog::LIGHT_SWEEP)?;
    b.title(
        "pill",
        "NEW IN",
        0,
        beat * 4,
        Some([0.0, 0.62]),
        Some((AnimationPreset::SlideUp, 350_000)),
    )?;
    b.title(
        "bold-outline",
        "Shop now",
        beat * 8,
        beat * 4,
        None,
        Some((AnimationPreset::Pop, 300_000)),
    )?;
    b.music(Bed::Bright, 0.85)?;
    b.finish(
        "product-spotlight",
        "Three square shots, a light sweep on the detail, and a call to action.",
        "Business",
        Some(beat * 6),
    )
}

fn vlog_intro() -> Result<TemplateFile, String> {
    let mut b = Build::new("Vlog Intro", 1920, 1080);
    let beat = Bed::Bright.beat();
    let slots = b.run(0, 3, beat * 4, SlotMedia::Video, &["Hello", "", "Let's go"])?;
    b.transitions(&slots, "seamless:zoom_in", 350_000)?;
    b.text_template("pop-headline", "Welcome back", beat, beat * 6)?;
    b.music(Bed::Bright, 0.8)?;
    b.finish(
        "vlog-intro",
        "A landscape opener: three clips, zoom-through cuts and a pop headline.",
        "Travel & vlog",
        Some(beat * 3),
    )
}

fn cinematic_trailer() -> Result<TemplateFile, String> {
    let mut b = Build::new("Cinematic Trailer", 1920, 1080);
    let beat = Bed::Cinematic.beat();
    let slots = b.run(0, 4, beat * 4, SlotMedia::Video, &["Establishing shot"])?;
    for slot in &slots {
        b.look(slot, "Blockbuster", 0.9)?;
    }
    for slot in slots.iter().skip(1) {
        b.builtin_transition(slot, TransitionKind::DipToColor, 600_000)?;
    }
    let length = b.project.duration();
    b.effect_clip(fx_catalog::LETTERBOX, 0, length)?;
    b.effect_clip(fx_catalog::GRAIN, 0, length)?;
    b.title(
        "elegant",
        "COMING SOON",
        beat * 12 + 300_000,
        beat * 4 - 300_000,
        Some([0.0, 0.0]),
        Some((AnimationPreset::Fade, 900_000)),
    )?;
    b.music(Bed::Cinematic, 0.9)?;
    b.finish(
        "cinematic-trailer",
        "Letterboxed, graded and grainy, with dips to black and a closing card.",
        "Cinematic",
        Some(beat * 2),
    )
}

fn memories() -> Result<TemplateFile, String> {
    let mut b = Build::new("Memories", 1080, 1920);
    let beat = Bed::Chill.beat();
    let slots = b.run(0, 5, beat * 3, SlotMedia::Any, &[])?;
    for slot in &slots {
        b.look(slot, "Faded Matte", 1.0)?;
    }
    for slot in slots.iter().skip(1) {
        b.builtin_transition(slot, TransitionKind::Dissolve, 450_000)?;
    }
    let length = b.project.duration();
    b.effect_clip(fx_catalog::GRAIN, 0, length)?;
    b.title(
        "soft-glow",
        "Remember this",
        300_000,
        beat * 5,
        None,
        Some((AnimationPreset::Rise, 700_000)),
    )?;
    b.music(Bed::Chill, 0.8)?;
    b.finish(
        "memories",
        "Five faded, grainy moments with a soft glowing title.",
        "Travel & vlog",
        Some(beat * 2),
    )
}

fn before_after() -> Result<TemplateFile, String> {
    let mut b = Build::new("Before / After", 1080, 1920);
    let length = 6 * SEC;
    let top_lane = b.lane(TrackKind::Video, "Top")?;
    let main = b.main.clone();
    // The split puts the first clip in the top cell. Each cell is the full
    // width and half the height: 1080×960, 9:8.
    let before = b.slot(SlotSpec {
        lane: &main,
        start: 0,
        duration: length,
        aspect: [9, 8],
        label: Some("Before"),
        accepts: SlotMedia::Any,
        sound: false,
    })?;
    let after = b.slot(SlotSpec {
        lane: &top_lane,
        start: 0,
        duration: length,
        aspect: [9, 8],
        label: Some("After"),
        accepts: SlotMedia::Any,
        sound: false,
    })?;
    let command = fx_edit::split_command(
        &b.project,
        &[before.clone(), after.clone()],
        SplitLayout::TwoRows,
    )?;
    b.apply(command)?;
    b.title("pill", "BEFORE", 0, length, Some([0.0, 0.84]), None)?;
    let second = b.lane(TrackKind::Text, "Labels")?;
    b.title_in(
        Some(&second),
        "pill",
        "AFTER",
        0,
        length,
        Some([0.0, -0.16]),
        None,
    )?;
    b.music(Bed::Pulse, 0.7)?;
    b.finish(
        "before-after",
        "Two clips stacked, labelled before and after.",
        "Social",
        Some(2 * SEC),
    )
}

fn photo_dump() -> Result<TemplateFile, String> {
    let mut b = Build::new("Photo Dump", 1080, 1920);
    let beat = Bed::Pulse.beat();
    let slots = b.run(0, 8, beat, SlotMedia::Image, &[])?;
    for slot in &slots {
        b.animate(
            slot,
            AnimationSlot::In,
            AnimationPreset::Pop,
            200_000,
            Ease::Back,
        )?;
    }
    b.title(
        "typewriter",
        "photo dump",
        0,
        beat * 3,
        Some([0.0, -0.55]),
        None,
    )?;
    b.music(Bed::Pulse, 0.9)?;
    b.finish(
        "photo-dump",
        "Eight photos, one per beat, each popping in.",
        "Social",
        Some(beat / 2),
    )
}

fn talking_points() -> Result<TemplateFile, String> {
    let mut b = Build::new("Talking Points", 1080, 1920);
    let aspect = b.canvas_aspect();
    let main = b.main.clone();
    let length = 9 * SEC;
    b.slot(SlotSpec {
        lane: &main,
        start: 0,
        duration: length,
        aspect,
        label: Some("You, talking"),
        accepts: SlotMedia::Video,
        sound: true,
    })?;
    for (n, words) in ["First, the problem", "Then, the fix", "Finally, the result"]
        .iter()
        .enumerate()
    {
        b.title(
            "lower-third",
            words,
            n as Micros * 3 * SEC + 200_000,
            3 * SEC - 400_000,
            None,
            Some((AnimationPreset::SlideRight, 400_000)),
        )?;
    }
    b.music(Bed::Chill, 0.2)?;
    b.finish(
        "talking-points",
        "One talking-head clip with three lower thirds and quiet music under the voice.",
        "Business",
        Some(SEC),
    )
}

fn retro_tape() -> Result<TemplateFile, String> {
    let mut b = Build::new("Retro Tape", 1920, 1080);
    let beat = Bed::Pulse.beat();
    let slots = b.run(0, 4, beat * 4, SlotMedia::Any, &[])?;
    for slot in &slots {
        b.look(slot, "Seventies", 0.9)?;
    }
    b.transitions(&slots, "gl:glitchmemories", 400_000)?;
    let length = b.project.duration();
    b.effect_clip(fx_catalog::VHS, 0, length)?;
    b.title(
        "synthwave",
        "REWIND",
        0,
        beat * 4,
        None,
        Some((AnimationPreset::ZoomIn, 400_000)),
    )?;
    b.music(Bed::Pulse, 0.85)?;
    b.finish(
        "retro-tape",
        "Old tape: a VHS wobble, a seventies grade and glitch cuts.",
        "Cinematic",
        Some(beat * 2),
    )
}

fn reaction() -> Result<TemplateFile, String> {
    let mut b = Build::new("Reaction", 1080, 1920);
    let length = 8 * SEC;
    let aspect = b.canvas_aspect();
    let main = b.main.clone();
    b.slot(SlotSpec {
        lane: &main,
        start: 0,
        duration: length,
        aspect,
        label: Some("What you react to"),
        accepts: SlotMedia::Any,
        sound: true,
    })?;
    let face_lane = b.lane(TrackKind::Video, "Face")?;
    let face = b.slot(SlotSpec {
        lane: &face_lane,
        start: 0,
        duration: length,
        aspect: [3, 4],
        label: Some("Your face"),
        accepts: SlotMedia::Video,
        sound: true,
    })?;
    let (frame, command) = fx_edit::pip_command(&b.project, &face, Corner::TopRight)?;
    b.project.materials.effects.push(frame);
    b.apply(command)?;
    b.title(
        "comic",
        "WAIT FOR IT",
        0,
        2 * SEC,
        Some([0.0, -0.6]),
        Some((AnimationPreset::Bounce, 500_000)),
    )?;
    b.finish(
        "reaction",
        "A clip with your face in the corner and a comic-style hook.",
        "Social",
        Some(SEC),
    )
}

/// Every built-in, built. One that fails to build is left out and logged:
/// a broken template must not take the tab down with it, and the test suite
/// builds them all.
pub fn all() -> Vec<TemplateFile> {
    builders()
        .into_iter()
        .filter_map(|(id, build)| match build() {
            Ok(file) => Some(file),
            Err(error) => {
                tracing::warn!(%id, %error, "a built-in template did not build");
                None
            }
        })
        .collect()
}

/// The built-in `id`, built.
pub fn get(id: &str) -> Option<Result<TemplateFile, String>> {
    builders()
        .into_iter()
        .find(|(known, _)| *known == id)
        .map(|(_, build)| build())
}

type Builder = fn() -> Result<TemplateFile, String>;

fn builders() -> Vec<(&'static str, Builder)> {
    vec![
        ("quick-cuts", quick_cuts),
        ("travel-diary", travel_diary),
        ("product-spotlight", product_spotlight),
        ("vlog-intro", vlog_intro),
        ("cinematic-trailer", cinematic_trailer),
        ("memories", memories),
        ("before-after", before_after),
        ("photo-dump", photo_dump),
        ("talking-points", talking_points),
        ("retro-tape", retro_tape),
        ("reaction", reaction),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::Severity;

    #[test]
    fn every_builtin_builds_into_a_valid_project_with_slots() {
        let all = builders();
        assert!((8..=12).contains(&all.len()));
        for (id, build) in all {
            let file = build().unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(file.id, id);
            let project = file.project(None).unwrap();
            let slots = super::super::slot::slots(&project);
            assert!(!slots.is_empty(), "{id} has no slots");
            for (n, slot) in slots.iter().enumerate() {
                assert_eq!(slot.index, n as u32 + 1, "{id}: slots numbered in order");
                assert!(!slot.filled);
            }
            let errors: Vec<_> = project
                .validate()
                .into_iter()
                .filter(|i| i.severity == Severity::Error)
                .collect();
            assert!(errors.is_empty(), "{id}: {errors:?}");
            assert!(project.duration() > 2 * SEC, "{id} is too short");
            assert!(!file.description.is_empty() && !file.category.is_empty());
        }
    }

    #[test]
    fn titles_meant_to_show_together_start_together() {
        let project = get("before-after").unwrap().unwrap().project(None).unwrap();
        let starts: Vec<Micros> = project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Text)
            .flat_map(|t| t.segments.iter().map(|s| s.target_range.start))
            .collect();
        assert_eq!(starts, vec![0, 0]);
    }

    #[test]
    fn music_covers_the_whole_template() {
        let file = get("quick-cuts").unwrap().unwrap();
        let project = file.project(None).unwrap();
        let music = project
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .expect("a music lane");
        assert_eq!(music.segments[0].target_range.duration, project.duration());
    }
}
