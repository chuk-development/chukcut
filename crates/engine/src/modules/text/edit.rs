//! Putting a title on the timeline, and changing one that is already there.
//!
//! Everything in this file is pure: it reads a `Project` and returns either a
//! new material or the [`EditCommand`] that would place it. That is what makes
//! the policy — how long a title is, which lane it lands on, what happens when
//! the playhead is over an existing title — testable without a GPU, a window or
//! an `AppState`, which is the same argument `transitions::edit` makes.
//!
//! ## Why the placement policy is here and not in TypeScript
//!
//! Minting a uuid, knowing the default duration, knowing that a title belongs
//! on a `TrackKind::Text` lane above the pictures, and knowing how to slide off
//! an occupied instant are four pieces of policy. Policy in the webview is
//! policy in two places, and the second copy is the one that goes stale.

use crate::modules::project::document::{
    new_id, Micros, Project, Segment, TextAlign, TextMaterial, TextShadow, TimeRange, Track,
    TrackKind, Transform, MICROS_PER_SECOND,
};
use crate::modules::timeline::ops::EditCommand;

/// How long a freshly added title sits on the timeline.
///
/// Three seconds rather than the five a still gets: a title is read once and a
/// still is looked at. Both are arbitrary, and both are a drag on the clip edge
/// away from being whatever the user wanted.
pub const DEFAULT_DURATION: Micros = 3 * MICROS_PER_SECOND;

/// What a new title says before anybody types.
///
/// Not an empty string. An empty title rasterises to a transparent layer, so a
/// user who pressed "Text" would see a clip appear on the timeline and nothing
/// at all in the preview, which reads as the feature being broken.
pub const PLACEHOLDER: &str = "Your title here";

/// Font size as a fraction of the canvas's short edge.
///
/// A fixed pixel size is wrong on two canvases at once: 72 px is a heading on
/// 1080x1920 and a caption on 3840x2160. The short edge rather than the height,
/// so a landscape and a portrait project of the same "quality" get the same
/// title.
const SIZE_FRACTION: f32 = 0.085;

/// The generic family every machine resolves.
///
/// Deliberately not the first family the machine happens to enumerate: a
/// project authored here must open on a machine with a different font set, and
/// `font::family_stack` already guarantees a real font behind a generic name.
pub const DEFAULT_FAMILY: &str = "sans-serif";

/// The default outline, in pixels at the default size.
///
/// On by default, and this is the one defaulting decision in the file worth
/// arguing about. White text over bright video is unreadable, the user cannot
/// see that it is unreadable until they export, and every editor that ships
/// with a plain white default gets asked for an outline within a day. Costs
/// four times the plain raster (`docs/research/text-rendering.md`) and that is
/// paid once per edit, not once per frame.
const OUTLINE_FRACTION: f32 = 0.045;

/// A title with sensible defaults for `project`'s canvas.
///
/// The id is minted here so the caller can hand it straight to
/// [`insert_command`] and then select the segment it produced.
pub fn default_material(project: &Project, content: Option<String>) -> TextMaterial {
    let short_edge = project.canvas.width.min(project.canvas.height).max(1) as f32;
    let font_size = (short_edge * SIZE_FRACTION).round().max(8.0);

    TextMaterial {
        id: new_id(),
        content: content.unwrap_or_else(|| PLACEHOLDER.to_string()),
        font_family: DEFAULT_FAMILY.to_string(),
        font_size,
        color: [1.0, 1.0, 1.0, 1.0],
        bold: true,
        italic: false,
        align: TextAlign::Center,
        stroke_width: (font_size * OUTLINE_FRACTION).round().max(1.0),
        stroke_color: [0.0, 0.0, 0.0, 1.0],
        // A shadow as well as an outline is CapCut's default and it is what
        // separates a title from a bright, busy background that happens to be
        // black behind the glyphs. Offset down-right by a twelfth of the size,
        // which stays proportional on every canvas.
        shadow: Some(TextShadow {
            color: [0.0, 0.0, 0.0, 0.55],
            offset: [(font_size / 12.0).round(), (font_size / 12.0).round()],
            blur: (font_size / 8.0).round(),
        }),
        background: None,
    }
}

/// Where a title is about to land: the lane, the instant, and the edit.
#[derive(Debug, Clone)]
pub struct TextPlacement {
    /// The whole placement as one undo step.
    pub command: EditCommand,
    /// The lane the title landed on — what the caller selects and scrolls to.
    pub track_id: String,
    /// Where it actually landed, which is not `at` when that instant was taken.
    pub start: Micros,
    pub segment_id: String,
}

/// The edit that puts `material_id` on the timeline at `at`.
///
/// A title goes on a [`TrackKind::Text`] lane, and a new one is appended to the
/// end of the track list when there is no usable lane. Appended, not inserted:
/// `render_index` is recomputed from track order and lower tracks paint first,
/// so the last lane is the top one — which is where a title has to be or it is
/// hidden behind the picture it is captioning.
///
/// When the aimed-at instant is occupied the title slides to the first gap
/// after it rather than being refused. A drag onto an occupied spot deserves an
/// error, because the user aimed at a microsecond; pressing a button called
/// "Text" does not, because they aimed at "now".
pub fn insert_command(
    project: &Project,
    material_id: &str,
    at: Micros,
    duration: Micros,
) -> Result<TextPlacement, String> {
    if duration <= 0 {
        return Err(format!("a title cannot be {duration} µs long"));
    }
    let wanted = at.max(0);

    let existing = title_lane(project);
    let lane = match existing {
        Some(track) => track.clone(),
        None => Track::new(TrackKind::Text, title_lane_name(project)),
    };

    let start = free_slot(&lane.segments, wanted, duration);
    let segment = title_segment(material_id, start, duration);
    let segment_id = segment.id.clone();
    let track_id = lane.id.clone();
    let index = lane
        .segments
        .iter()
        .filter(|s| s.target_range.start < start)
        .count();

    let mut commands = Vec::new();
    if existing.is_none() {
        commands.push(EditCommand::AddTrack {
            track: lane,
            index: project.tracks.len(),
        });
    }
    commands.push(EditCommand::InsertSegment {
        track_id: track_id.clone(),
        segment,
        index,
    });

    Ok(TextPlacement {
        command: EditCommand::Composite {
            label: "Add title".to_string(),
            commands,
        },
        track_id,
        start,
        segment_id,
    })
}

/// The lane a title should go on, if the project already has a usable one.
fn title_lane(project: &Project) -> Option<&Track> {
    project
        .tracks
        .iter()
        .rev()
        .find(|t| t.kind == TrackKind::Text && !t.locked)
}

fn title_lane_name(project: &Project) -> String {
    let count = project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Text)
        .count();
    format!("Text {}", count + 1)
}

/// First instant at or after `at` where a clip of `duration` fits.
///
/// The same rule as the frontend's `findFreeSlot`, restated here because a
/// title's lane is chosen in Rust and the frontend never sees it. Always
/// succeeds: the space after the last clip is unbounded.
pub fn free_slot(segments: &[Segment], at: Micros, duration: Micros) -> Micros {
    let mut sorted: Vec<&Segment> = segments.iter().collect();
    sorted.sort_by_key(|s| s.target_range.start);

    let mut start = at.max(0);
    for segment in sorted {
        let end = segment.target_range.end();
        if end <= start {
            continue;
        }
        if segment.target_range.start - start >= duration {
            break;
        }
        start = end;
    }
    start
}

/// A segment for a text material.
///
/// `source_range` mirrors `target_range` at speed 1, which is what the
/// document's speed invariant requires and what `split_at` reads. A title has
/// no source to read *from* — the rasteriser ignores the instant entirely — but
/// the invariant is checked for every segment, so the range has to be honest
/// arithmetic rather than a zero.
fn title_segment(material_id: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
        id: new_id(),
        material_id: material_id.to_string(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(0, duration),
        // Recomputed from track order by `ops::reindex` the moment this is
        // applied; the value here is never the one that renders.
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// Reject the values that cannot survive a save.
///
/// `serde_json` writes a NaN or an infinity as `null` and the project then
/// fails to load forever after, so a non-finite number reaching the document is
/// silent data loss rather than a wrong picture. `timeline/ops.rs::check_finite`
/// makes the same check for segments; this is the material's half of it, and it
/// is here because there is no `EditCommand` that carries a `TextMaterial`.
pub fn check_material(material: &TextMaterial) -> Result<(), String> {
    let finite = |values: &[f32]| values.iter().all(|v| v.is_finite());

    if !material.font_size.is_finite() || material.font_size <= 0.0 {
        return Err(format!(
            "font size must be a positive number, not {}",
            material.font_size
        ));
    }
    if !material.stroke_width.is_finite() || material.stroke_width < 0.0 {
        return Err(format!(
            "outline width must be zero or more, not {}",
            material.stroke_width
        ));
    }
    if !finite(&material.color) || !finite(&material.stroke_color) {
        return Err("a text colour is not a finite number".to_string());
    }
    if let Some(shadow) = material.shadow {
        if !finite(&shadow.color) || !finite(&shadow.offset) || !shadow.blur.is_finite() {
            return Err("the shadow has a value that is not a finite number".to_string());
        }
        if shadow.blur < 0.0 {
            return Err(format!(
                "shadow blur must be zero or more, not {}",
                shadow.blur
            ));
        }
    }
    if let Some(background) = material.background {
        if !finite(&background) {
            return Err("the background colour is not a finite number".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::CanvasConfig;

    fn project() -> Project {
        Project::new(
            "titles",
            CanvasConfig {
                width: 1080,
                height: 1920,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        )
    }

    #[test]
    fn the_default_size_follows_the_canvas() {
        let vertical = default_material(&project(), None);

        let mut wide = project();
        wide.canvas.width = 3840;
        wide.canvas.height = 2160;
        let big = default_material(&wide, None);

        assert_eq!(vertical.font_size, (1080.0f32 * SIZE_FRACTION).round());
        assert_eq!(big.font_size, (2160.0f32 * SIZE_FRACTION).round());
        assert!(
            big.font_size > vertical.font_size,
            "a 4K canvas should get a larger title than a 1080p one"
        );
    }

    #[test]
    fn a_new_title_is_outlined_and_readable() {
        let material = default_material(&project(), None);
        assert!(material.stroke_width >= 1.0, "the outline is on by default");
        assert_eq!(material.stroke_color, [0.0, 0.0, 0.0, 1.0]);
        assert!(material.shadow.is_some());
        assert_eq!(material.content, PLACEHOLDER);
        assert!(
            !material.content.is_empty(),
            "an empty title paints nothing"
        );
    }

    #[test]
    fn the_first_title_makes_a_text_lane_at_the_top() {
        let mut project = project();
        project.tracks.push(Track::new(TrackKind::Video, "Video 1"));

        let material = default_material(&project, None);
        let placed = insert_command(&project, &material.id, 0, DEFAULT_DURATION).unwrap();

        let EditCommand::Composite { commands, .. } = &placed.command else {
            panic!("expected a composite");
        };
        assert_eq!(commands.len(), 2, "add the lane, then insert the title");
        match &commands[0] {
            EditCommand::AddTrack { track, index } => {
                assert_eq!(track.kind, TrackKind::Text);
                // Appended: lower tracks paint first, so the last lane is on top.
                assert_eq!(*index, project.tracks.len());
            }
            other => panic!("expected AddTrack, got {other:?}"),
        }
    }

    #[test]
    fn a_second_title_reuses_the_lane() {
        let mut project = project();
        let material = default_material(&project, None);
        let first = insert_command(&project, &material.id, 0, DEFAULT_DURATION).unwrap();
        first.command.apply(&mut project).unwrap();

        let second = insert_command(&project, &material.id, 0, DEFAULT_DURATION).unwrap();
        let EditCommand::Composite { commands, .. } = &second.command else {
            panic!("expected a composite");
        };
        assert_eq!(commands.len(), 1, "no second lane");
        assert_eq!(second.track_id, first.track_id);
    }

    #[test]
    fn a_title_slides_off_one_that_is_already_there() {
        let mut project = project();
        let material = default_material(&project, None);
        let first = insert_command(&project, &material.id, 0, DEFAULT_DURATION).unwrap();
        first.command.apply(&mut project).unwrap();

        // Aimed straight at the middle of the one already placed.
        let second = insert_command(
            &project,
            &material.id,
            DEFAULT_DURATION / 2,
            DEFAULT_DURATION,
        )
        .unwrap();
        assert_eq!(
            second.start, DEFAULT_DURATION,
            "the second title should start where the first ends"
        );

        second.command.apply(&mut project).unwrap();
        // Which is the real check: an overlap would fail validation.
        assert!(
            project
                .validate()
                .iter()
                .all(|i| !i.message.contains("overlap")),
            "{:?}",
            project.validate()
        );
    }

    #[test]
    fn a_locked_text_lane_is_not_used() {
        let mut project = project();
        let mut locked = Track::new(TrackKind::Text, "Text 1");
        locked.locked = true;
        project.tracks.push(locked.clone());

        let material = default_material(&project, None);
        let placed = insert_command(&project, &material.id, 0, DEFAULT_DURATION).unwrap();
        assert_ne!(placed.track_id, locked.id);
    }

    #[test]
    fn a_placed_title_satisfies_the_documents_invariants() {
        let mut project = project();
        let material = default_material(&project, None);
        let placed = insert_command(&project, &material.id, 0, DEFAULT_DURATION).unwrap();
        project.materials.texts.push(material);
        placed.command.apply(&mut project).unwrap();

        let issues = project.validate();
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn non_finite_values_are_refused() {
        let mut material = default_material(&project(), None);
        assert!(check_material(&material).is_ok());

        material.font_size = f32::NAN;
        assert!(check_material(&material).is_err());

        material = default_material(&project(), None);
        material.color[0] = f32::INFINITY;
        assert!(check_material(&material).is_err());

        material = default_material(&project(), None);
        material.stroke_width = -1.0;
        assert!(check_material(&material).is_err());

        material = default_material(&project(), None);
        material.shadow = Some(TextShadow {
            color: [0.0, 0.0, 0.0, 1.0],
            offset: [f32::NAN, 0.0],
            blur: 0.0,
        });
        assert!(check_material(&material).is_err());
    }

    #[test]
    fn free_slot_finds_the_gap_between_two_clips() {
        let a = title_segment("m", 0, 1_000_000);
        let b = title_segment("m", 3_000_000, 1_000_000);
        let segments = vec![a, b];

        // A short title fits in the gap.
        assert_eq!(free_slot(&segments, 0, 500_000), 1_000_000);
        // A long one does not, and goes after everything.
        assert_eq!(free_slot(&segments, 0, 2_500_000), 4_000_000);
        // Past the end, nothing moves.
        assert_eq!(free_slot(&segments, 9_000_000, 1_000_000), 9_000_000);
    }
}
