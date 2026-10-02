//! Titles, end to end: the document, the edits, and the pixels.
//!
//! Three claims are made here, and each is the kind that is normally taken on
//! trust and turns out to be false:
//!
//! - **A title renders identically in the preview and in the export.** Not
//!   "similarly": at the same target size, byte-identical, because both paths
//!   are the same `MediaSourceProvider` and the same compositor, and the only
//!   thing that could make them differ is the raster scale. When the preview is
//!   *smaller* than the canvas, the title has to land in the same place
//!   proportionally — which is the whole point of font sizes being in document
//!   pixels — and that is checked by comparing ink rectangles in normalised
//!   coordinates.
//! - **A title is a segment.** Trim, move, split, copy and delete are supposed
//!   to fall out of it being one. `split_at` in particular does arithmetic on
//!   `source_range`, and a text material has no source to read from, so it is
//!   verified rather than assumed.
//! - **What the inspector can set survives a save.** Every field of
//!   `TextMaterial`, through the real loader, plus what an older file without
//!   them opens as.

mod support;

use std::sync::Arc;

use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::preview::proxy_size;
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TextAlign, TextMaterial, TextShadow, TimeRange, Track,
    TrackKind, Transform,
};
use chukcut_engine::modules::render::{
    Compositor, CompositorConfig, RenderContext, SourceProvider,
};
use chukcut_engine::modules::text::edit;
use chukcut_engine::modules::timeline::ops::{split_at, EditCommand};
use chukcut_engine::modules::timeline::History;

const CANVAS: (u32, u32) = (1280, 720);
const DURATION: Micros = 4_000_000;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A title with everything the inspector can switch on, so a test that draws it
/// exercises the fill, the stroke, the shadow and the box in one pass.
fn loud_title(id: &str) -> TextMaterial {
    TextMaterial {
        id: id.to_string(),
        content: "TITLE".to_string(),
        // Not a named family: the machine running this may have any font set,
        // and `font::family_stack` guarantees a real font behind a generic.
        font_family: "sans-serif".to_string(),
        font_size: 96.0,
        color: [1.0, 1.0, 1.0, 1.0],
        bold: true,
        italic: false,
        align: TextAlign::Center,
        stroke_width: 4.0,
        stroke_color: [0.0, 0.0, 0.0, 1.0],
        shadow: Some(TextShadow {
            color: [0.0, 0.0, 0.0, 0.6],
            offset: [6.0, 6.0],
            blur: 8.0,
        }),
        background: None,
    }
}

fn project_with_title(material: TextMaterial) -> Project {
    let mut project = Project::new(
        "titles",
        CanvasConfig {
            width: CANVAS.0,
            height: CANVAS.1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    let id = material.id.clone();
    project.materials.texts.push(material);

    let mut lane = Track::new(TrackKind::Text, "Text 1");
    lane.segments.push(title_segment(&id, 0, DURATION));
    project.tracks.push(lane);
    project
}

fn title_segment(material_id: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
        id: chukcut_engine::modules::project::new_id(),
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
    }
}

/// Composite one frame exactly as both the preview server and the exporter do:
/// a provider built from the project snapshot, and `render_frame` at a size.
///
/// The two call sites are `preview/server.rs::render_one` and
/// `export/job.rs::run_export`, and the *only* difference between them is the
/// size they pass — which is what makes rendering twice here a faithful stand-in
/// for rendering once down each path.
fn composite(
    ctx: &Arc<RenderContext>,
    project: &Project,
    time: Micros,
    size: (u32, u32),
) -> Vec<u8> {
    // A fresh provider each time, because the export builds one and the preview
    // builds another; sharing one would hide a cache that serves the wrong size.
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(project));
    let compositor = Compositor::with_config(
        Arc::clone(ctx),
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );
    compositor
        .render_frame(project, time, size, sources.as_ref())
        .expect("composite the frame")
}

/// Where the title's ink is, as a fraction of the frame: `[x0, y0, x1, y1]`.
///
/// "Ink" is any pixel brighter than the black canvas by more than the shadow's
/// darkest tint, so the box is the glyphs and their outline rather than the
/// shadow's blur. Returns `None` for an empty frame, which is itself a useful
/// failure — it means nothing was drawn.
fn ink_box(rgba: &[u8], width: u32, height: u32) -> Option<[f64; 4]> {
    const BRIGHT: u8 = 128;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let luma = rgba[i].max(rgba[i + 1]).max(rgba[i + 2]);
            if luma < BRIGHT {
                continue;
            }
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX {
        return None;
    }
    Some([
        x0 as f64 / width as f64,
        y0 as f64 / height as f64,
        (x1 + 1) as f64 / width as f64,
        (y1 + 1) as f64 / height as f64,
    ])
}

fn bright_pixels(rgba: &[u8]) -> usize {
    rgba.chunks_exact(4)
        .filter(|p| p[0] > 200 && p[3] > 0)
        .count()
}

// ---------------------------------------------------------------------------
// The same title in the preview and in the export
// ---------------------------------------------------------------------------

/// At one resolution the two paths must agree exactly, not approximately.
///
/// `proxy_size` returns the canvas unchanged for anything up to 1920 on the
/// long edge, so for an ordinary 720p or 1080p project the preview renders at
/// the export's size and the frames have to be the same bytes. Anything that
/// made the raster depend on which path asked for it — a different scale, a
/// different vertical alignment, a cache keyed on the wrong thing — shows up
/// here as a diff of thousands of pixels.
#[test]
fn a_title_is_the_same_pixels_in_the_preview_and_the_export() {
    let ctx = require_gpu!();
    let project = project_with_title(loud_title("t1"));

    let export_size = (project.canvas.width, project.canvas.height);
    let preview_size = proxy_size(export_size, None);
    assert_eq!(
        preview_size, export_size,
        "a 1280x720 project previews at its own size; this test is about the case where it does"
    );

    let at_export = composite(&ctx, &project, DURATION / 2, export_size);
    let at_preview = composite(&ctx, &project, DURATION / 2, preview_size);

    // A title that drew nothing would pass a byte comparison trivially.
    assert!(
        bright_pixels(&at_export) > 500,
        "the title did not paint: only {} bright pixels",
        bright_pixels(&at_export)
    );

    assert_eq!(at_export.len(), at_preview.len());
    let differing = at_export
        .chunks_exact(4)
        .zip(at_preview.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ between the preview's frame and the export's"
    );
}

/// And when the preview *is* smaller, the title has to be in the same place.
///
/// This is the property that font sizes being in document pixels exists to
/// give: a 96 px title on a 3840x2160 canvas is 96 px of a 3840-wide frame and
/// 48 px of a 1920-wide one, so its ink covers the same *fraction* of both.
/// Getting this wrong is the classic bug where the preview looks right and the
/// export has a title half the size, and it is invisible to any test that only
/// renders one of them.
#[test]
fn a_downscaled_preview_puts_the_title_in_the_same_place() {
    let ctx = require_gpu!();
    let mut project = project_with_title(loud_title("t1"));
    project.canvas.width = 3840;
    project.canvas.height = 2160;

    let export_size = (project.canvas.width, project.canvas.height);
    let preview_size = proxy_size(export_size, None);
    assert!(
        preview_size.0 < export_size.0,
        "the proxy should be smaller"
    );

    let big = composite(&ctx, &project, DURATION / 2, export_size);
    let small = composite(&ctx, &project, DURATION / 2, preview_size);

    let big_box = ink_box(&big, export_size.0, export_size.1).expect("the export drew the title");
    let small_box =
        ink_box(&small, preview_size.0, preview_size.1).expect("the preview drew the title");

    // One preview pixel is 1/1920 of the frame; two of them is the whole
    // budget, which covers hinting and rounding differing between the two
    // rasterisations (they are deliberately shaped at their own size — see
    // `docs/research/text-rendering.md`).
    let tolerance = 2.0 / preview_size.0 as f64;
    for (i, name) in ["x0", "y0", "x1", "y1"].iter().enumerate() {
        let delta = (big_box[i] - small_box[i]).abs();
        assert!(
            delta <= tolerance,
            "{name}: the export has the title's ink at {:.4} and the preview at {:.4} \
             ({delta:.4} apart, budget {tolerance:.4})",
            big_box[i],
            small_box[i]
        );
    }
}

/// The size check in `text_frame` is what makes the above true, so break it on
/// purpose: render small first, then large, through **one** provider, and the
/// large frame must not be the small raster stretched.
#[test]
fn one_provider_does_not_serve_the_preview_raster_to_the_export() {
    let ctx = require_gpu!();
    let project = project_with_title(loud_title("t1"));

    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let compositor = Compositor::with_config(
        Arc::clone(&ctx),
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );

    let half = (CANVAS.0 / 2, CANVAS.1 / 2);
    let small = compositor
        .render_frame(&project, 0, half, sources.as_ref())
        .expect("small frame");
    let large = compositor
        .render_frame(&project, 0, CANVAS, sources.as_ref())
        .expect("large frame");

    assert_eq!(small.len(), (half.0 * half.1 * 4) as usize);
    assert_eq!(large.len(), (CANVAS.0 * CANVAS.1 * 4) as usize);

    // The same provider, asked twice: the second answer has to be its own
    // rasterisation. Compared against a frame that never met the small one.
    let fresh = composite(&ctx, &project, 0, CANVAS);
    let differing = large
        .chunks_exact(4)
        .zip(fresh.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(
        differing, 0,
        "{differing} pixels differ: the cached preview-sized raster leaked into the full-size frame"
    );
}

/// A title has no source time, and must not acquire one.
///
/// The provider's generic cache compares `source_time`; the text path's own
/// lookup ignores it. If that ever changed, a title would flicker between
/// rasterisations during playback — so two instants of the same clip are
/// checked to be the same picture.
#[test]
fn a_title_does_not_change_with_time() {
    let ctx = require_gpu!();
    let project = project_with_title(loud_title("t1"));

    let early = composite(&ctx, &project, 100_000, CANVAS);
    let late = composite(&ctx, &project, DURATION - 100_000, CANVAS);

    assert_eq!(
        early, late,
        "the title changed between two instants of one clip"
    );
}

/// What the outline is for, in pixels.
///
/// The single most-used title feature, and the one whose absence is only
/// noticed after the upload: white text on a bright background. So render the
/// same title over a white canvas with and without its outline, and check that
/// the outlined one actually puts dark pixels between the glyphs and the
/// background.
#[test]
fn an_outline_puts_dark_pixels_around_white_glyphs() {
    let ctx = require_gpu!();

    let mut plain = loud_title("t1");
    plain.stroke_width = 0.0;
    plain.shadow = None;
    let mut outlined = plain.clone();
    outlined.stroke_width = 6.0;

    let dark_pixels = |material: TextMaterial| {
        let mut project = project_with_title(material);
        // A white canvas is the case the outline exists for.
        project.canvas.background = [1.0, 1.0, 1.0, 1.0];
        let frame = composite(&ctx, &project, 0, CANVAS);
        frame
            .chunks_exact(4)
            .filter(|p| p[0] < 64 && p[1] < 64 && p[2] < 64)
            .count()
    };

    let without = dark_pixels(plain);
    let with = dark_pixels(outlined);

    assert!(
        with > without + 1000,
        "the outline added only {} dark pixels ({without} without, {with} with) — \
         white text on white is exactly what it is for",
        with.saturating_sub(without)
    );
}

// ---------------------------------------------------------------------------
// A title is a segment
// ---------------------------------------------------------------------------

/// `split_at` does arithmetic on `source_range`, and a title has no source.
///
/// The halves must still satisfy the document's speed invariant — `validate()`
/// checks it for every segment, text or not — and both must keep pointing at
/// the same material, because a split is a cut and not a copy.
#[test]
fn splitting_a_title_leaves_two_halves_that_validate() {
    let mut project = project_with_title(loud_title("t1"));
    let original = project.tracks[0].segments[0].clone();
    let at = original.target_range.start + DURATION / 4;

    let command = split_at(&project, &original.id, at).expect("split the title");
    let mut history = History::new();
    history
        .apply(&mut project, command)
        .expect("apply the split");

    let segments = &project.tracks[0].segments;
    assert_eq!(segments.len(), 2);

    let (left, right) = (&segments[0], &segments[1]);
    assert_eq!(left.material_id, "t1");
    assert_eq!(right.material_id, "t1");
    assert_eq!(left.target_range.start, 0);
    assert_eq!(left.target_range.duration, DURATION / 4);
    assert_eq!(right.target_range.start, at);
    assert_eq!(right.target_range.duration, DURATION - DURATION / 4);
    assert_eq!(
        left.target_range.duration + right.target_range.duration,
        DURATION,
        "the split lost or gained time"
    );
    // At speed 1 the source range mirrors the target range, and the right half
    // reads from where the left one stopped — meaningless for a title, but it
    // is what `validate()` and every other consumer expect to see.
    assert_eq!(right.source_range.start, left.source_range.duration);

    let issues = project.validate();
    assert!(issues.is_empty(), "{issues:?}");

    // And it undoes back to one clip, in one step.
    history.undo(&mut project).expect("undo the split");
    assert_eq!(project.tracks[0].segments.len(), 1);
    assert_eq!(
        project.tracks[0].segments[0].target_range.duration,
        DURATION
    );
}

/// Both halves of a split title draw the same thing, because they name the same
/// material — the cut is in time, not in content.
#[test]
fn both_halves_of_a_split_title_still_draw_it() {
    let ctx = require_gpu!();
    let mut project = project_with_title(loud_title("t1"));
    let original = project.tracks[0].segments[0].clone();
    let at = DURATION / 2;

    let command = split_at(&project, &original.id, at).expect("split the title");
    command.apply(&mut project).expect("apply the split");

    let left = composite(&ctx, &project, at / 2, CANVAS);
    let right = composite(&ctx, &project, at + at / 2, CANVAS);

    assert!(bright_pixels(&left) > 500, "the left half drew nothing");
    assert_eq!(left, right, "the two halves of one title drew differently");
}

/// Trim, move, copy and delete, each through the real command path and each
/// undone.
///
/// Not one test per verb: what is being checked is that a text segment goes
/// through the same machinery as a video one, and the interesting failure would
/// be any of them refusing at the boundary because the material is not a file.
#[test]
fn a_title_trims_moves_copies_and_deletes_like_a_clip() {
    let mut project = project_with_title(loud_title("t1"));
    let mut history = History::new();
    let original = project.tracks[0].segments[0].clone();

    // Trim the tail in. The source range has to move with it or the speed
    // invariant fails.
    let shorter = TimeRange::new(0, DURATION / 2);
    history
        .apply(
            &mut project,
            EditCommand::TrimSegment {
                segment_id: original.id.clone(),
                before_target: original.target_range,
                before_source: original.source_range,
                after_target: shorter,
                after_source: shorter,
            },
        )
        .expect("trim the title");
    assert_eq!(
        project.tracks[0].segments[0].target_range.duration,
        DURATION / 2
    );
    assert!(project.validate().is_empty(), "{:?}", project.validate());

    // Move it along the lane.
    let lane = project.tracks[0].id.clone();
    history
        .apply(
            &mut project,
            EditCommand::MoveSegment {
                segment_id: original.id.clone(),
                from_track: lane.clone(),
                to_track: lane.clone(),
                from_start: 0,
                to_start: 5_000_000,
            },
        )
        .expect("move the title");
    assert_eq!(project.tracks[0].segments[0].target_range.start, 5_000_000);

    // Copy: a second segment naming the same material, which is what a
    // duplicate is.
    let copy = title_segment("t1", 0, DURATION / 2);
    history
        .apply(
            &mut project,
            EditCommand::InsertSegment {
                track_id: lane.clone(),
                segment: copy,
                index: 0,
            },
        )
        .expect("duplicate the title");
    assert_eq!(project.tracks[0].segments.len(), 2);
    assert!(project.validate().is_empty(), "{:?}", project.validate());

    // Delete the copy.
    let removed = project.tracks[0].segments[0].clone();
    history
        .apply(
            &mut project,
            EditCommand::RemoveSegment {
                track_id: lane,
                segment: removed,
                index: 0,
            },
        )
        .expect("delete the title");
    assert_eq!(project.tracks[0].segments.len(), 1);

    // Every one of them undoes, in order, back to where it started.
    for _ in 0..4 {
        history.undo(&mut project).expect("undo");
    }
    let back = &project.tracks[0].segments[0];
    assert_eq!(back.id, original.id);
    assert_eq!(back.target_range, original.target_range);
    assert_eq!(back.source_range, original.source_range);
    assert!(project.validate().is_empty(), "{:?}", project.validate());
}

/// Deleting the last clip that used a title leaves the material behind, and
/// that is deliberate — `RemoveSegment` carries the segment, not the material,
/// so undo has something to point at.
#[test]
fn deleting_the_only_title_clip_keeps_the_material_for_the_undo() {
    let mut project = project_with_title(loud_title("t1"));
    let mut history = History::new();
    let segment = project.tracks[0].segments[0].clone();
    let lane = project.tracks[0].id.clone();

    history
        .apply(
            &mut project,
            EditCommand::RemoveSegment {
                track_id: lane,
                segment,
                index: 0,
            },
        )
        .expect("delete");

    assert!(project.tracks[0].segments.is_empty());
    assert_eq!(project.materials.texts.len(), 1);

    history.undo(&mut project).expect("undo the delete");
    assert_eq!(project.tracks[0].segments.len(), 1);
    assert!(project.validate().is_empty(), "{:?}", project.validate());
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

/// Everything the inspector can set has to come back off disk unchanged.
///
/// Through `migrate::load` rather than `serde_json::from_str`, because that is
/// the only sanctioned way bytes become a `Project` and it is where the
/// non-finite repair lives.
#[test]
fn every_field_of_a_title_round_trips_through_a_save() {
    let material = TextMaterial {
        id: "t1".into(),
        content: "Two lines\nand an emoji 🎬".into(),
        font_family: "DejaVu Sans".into(),
        font_size: 73.5,
        color: [0.1, 0.2, 0.3, 0.9],
        bold: true,
        italic: true,
        align: TextAlign::Right,
        stroke_width: 5.5,
        stroke_color: [0.4, 0.5, 0.6, 0.8],
        shadow: Some(TextShadow {
            color: [0.7, 0.0, 0.1, 0.5],
            offset: [-3.0, 7.5],
            blur: 11.25,
        }),
        background: Some([0.0, 0.0, 0.0, 0.6]),
    };
    let project = project_with_title(material.clone());

    let json = serde_json::to_string_pretty(&project).expect("serialize");
    let loaded = chukcut_engine::modules::project::migrate::load(&json).expect("load");
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);

    let back = &loaded.project.materials.texts[0];
    assert_eq!(back.id, material.id);
    assert_eq!(back.content, material.content);
    assert_eq!(back.font_family, material.font_family);
    assert_eq!(back.font_size, material.font_size);
    assert_eq!(back.color, material.color);
    assert_eq!(back.bold, material.bold);
    assert_eq!(back.italic, material.italic);
    assert_eq!(back.align, material.align);
    assert_eq!(back.stroke_width, material.stroke_width);
    assert_eq!(back.stroke_color, material.stroke_color);
    let shadow = back.shadow.expect("the shadow survived");
    assert_eq!(shadow.color, [0.7, 0.0, 0.1, 0.5]);
    assert_eq!(shadow.offset, [-3.0, 7.5]);
    assert_eq!(shadow.blur, 11.25);
    assert_eq!(back.background, material.background);

    // A save of an unchanged document produces an unchanged file.
    let again = serde_json::to_string_pretty(&loaded.project).expect("serialize again");
    assert_eq!(json, again);
}

/// A file written before the outline, the shadow and the box existed still
/// opens, with each of them documented-off rather than absent.
#[test]
fn a_title_without_the_optional_fields_takes_the_documented_defaults() {
    let mut project = project_with_title(loud_title("t1"));
    project.tracks.clear();

    let mut json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&project).expect("serialize")).expect("parse");
    let material = &mut json["materials"]["texts"][0];
    for gone in [
        "font_size",
        "color",
        "bold",
        "italic",
        "align",
        "stroke_width",
        "stroke_color",
        "shadow",
        "background",
    ] {
        material
            .as_object_mut()
            .expect("a material is an object")
            .remove(gone);
    }

    let loaded = chukcut_engine::modules::project::migrate::load(&json.to_string())
        .expect("an older file still opens");
    let back = &loaded.project.materials.texts[0];

    assert_eq!(back.font_size, 48.0, "the documented default font size");
    assert_eq!(
        back.color,
        [1.0, 1.0, 1.0, 1.0],
        "white, not transparent black"
    );
    assert!(!back.bold);
    assert!(!back.italic);
    assert_eq!(back.align, TextAlign::Center);
    assert_eq!(back.stroke_width, 0.0);
    assert!(back.shadow.is_none());
    assert!(back.background.is_none());
}

/// A title added by the command layer's own planner is a valid document.
///
/// The planner is pure and has its own unit tests; this is the seam between it
/// and the rest — that what it produces goes through `History::apply` and
/// leaves `validate()` with nothing to say.
#[test]
fn the_planner_produces_a_document_that_validates_and_undoes() {
    let mut project = Project::new("titles", CanvasConfig::default(), 30.0);
    project.tracks.push(Track::new(TrackKind::Video, "Video 1"));

    let material = edit::default_material(&project, None);
    let material_id = material.id.clone();
    let placement =
        edit::insert_command(&project, &material_id, 1_000_000, edit::DEFAULT_DURATION).unwrap();
    project.materials.texts.push(material);

    let mut history = History::new();
    history
        .apply(&mut project, placement.command)
        .expect("place the title");

    let issues = project.validate();
    assert!(issues.is_empty(), "{issues:?}");

    let lane = project
        .tracks
        .iter()
        .find(|t| t.id == placement.track_id)
        .expect("the lane the placement named");
    assert_eq!(lane.kind, TrackKind::Text);
    assert_eq!(lane.segments.len(), 1);
    assert_eq!(lane.segments[0].target_range.start, 1_000_000);
    // The text lane was appended, so its segments carry the highest
    // `render_index` and the title paints over the picture.
    let video_index = project.tracks[0].segments.first().map(|s| s.render_index);
    assert!(
        video_index.is_none() || lane.segments[0].render_index > video_index.unwrap(),
        "the title has to paint on top"
    );

    history.undo(&mut project).expect("undo");
    assert!(
        project
            .tracks
            .iter()
            .all(|t| t.kind != TrackKind::Text || t.segments.is_empty()),
        "one undo should take the whole placement, lane and all"
    );
}

/// An empty title is a real thing a user makes by clearing the text box, and it
/// must not take the frame down with it.
#[test]
fn an_empty_title_renders_a_blank_frame_rather_than_failing() {
    let ctx = require_gpu!();
    let mut material = loud_title("t1");
    material.content = String::new();
    let project = project_with_title(material);

    let frame = composite(&ctx, &project, 0, CANVAS);
    assert_eq!(frame.len(), (CANVAS.0 * CANVAS.1 * 4) as usize);
    assert_eq!(bright_pixels(&frame), 0, "an empty title painted something");
}

// ---------------------------------------------------------------------------
// Through the whole export
// ---------------------------------------------------------------------------

/// The strongest form of "identical in preview and export": encode the timeline
/// with the real exporter, decode the file with a decoder that never saw the
/// compositor, and compare the picture against the frame the preview would have
/// shown at the same instant.
///
/// The comparison is a PSNR rather than an equality because H.264 is lossy —
/// everything between `render_frame` and the file (RGBA→NV12, the encoder's
/// quantiser, the decoder's own colour conversion) moves code values around.
/// What it can still catch is every way a title goes *wrong*: absent, in the
/// wrong place, the wrong size, or drawn at the preview's scale.
///
/// The controls are what make the number mean something. A blank frame and a
/// frame of a project with no title at all are scored the same way, and both
/// come out far below the bar — so "40 dB" is a claim about this title being in
/// this file, not about two dark images being similar.
#[test]
fn a_title_survives_a_real_export_and_matches_the_preview() {
    use chukcut_engine::modules::export::{
        job, resolve_settings, run_export, ExportJob, ExportOverrides, ExportRequest, FnSink,
    };
    use chukcut_engine::modules::media::VideoDecoder;
    use std::sync::atomic::AtomicBool;

    let ctx = require_gpu!();
    let project = project_with_title(loud_title("t1"));

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("text");
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let path = dir.join("title_export.mp4");
    let _ = std::fs::remove_file(&path);

    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: Some(ExportOverrides {
            width: Some(CANVAS.0),
            height: Some(CANVAS.1),
            fps: Some(30.0),
            ..Default::default()
        }),
        // Software, always: a hardware encoder is the driver's opinion and this
        // test is about the title, not about the encoder.
        hardware: None,
        include_audio: false,
        range: None,
    };

    let settings = resolve_settings(&project, &request).expect("resolve the settings");
    let job = ExportJob {
        job_id: "text-title".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            Arc::clone(&ctx),
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let outcome = run_export(&job, &FnSink(|_| {})).expect("the export should finish");
    assert!(!outcome.cancelled);

    // Half a second in, which is a whole GOP past the first frame.
    let at: Micros = 500_000;
    let mut decoder = VideoDecoder::open(&path).expect("open the exported file");
    let decoded = decoder.seek_and_decode(at).expect("decode the export");
    assert_eq!((decoded.width, decoded.height), CANVAS);

    let previewed = composite(&ctx, &project, at, CANVAS);
    let score = psnr(&previewed, &decoded.data);

    // Controls, scored the same way.
    let blank = vec![0u8; previewed.len()];
    let blank_score = psnr(&previewed, &blank);
    let mut titleless = project.clone();
    titleless.tracks[0].segments.clear();
    titleless.tracks[0]
        .segments
        .push(title_segment("t1", DURATION * 4, DURATION));
    let without = composite(&ctx, &titleless, at, CANVAS);
    let without_score = psnr(&previewed, &without);

    assert!(
        score > 30.0,
        "the exported title scores {score:.1} dB against the preview's frame \
         (blank control {blank_score:.1}, no-title control {without_score:.1})"
    );
    assert!(
        score > without_score + 8.0,
        "the export scores {score:.1} dB and a frame with no title at all scores \
         {without_score:.1} — the title is not distinguishable in the file"
    );

    let _ = std::fs::remove_file(&path);
}

/// Peak signal-to-noise ratio over R, G and B. Infinite when identical.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let mut sum = 0.0f64;
    let mut count = 0usize;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for c in 0..3 {
            let d = pa[c] as f64 - pb[c] as f64;
            sum += d * d;
            count += 1;
        }
    }
    if sum == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64.powi(2) / (sum / count as f64)).log10()
}
