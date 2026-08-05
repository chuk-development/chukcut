//! Compositing real decoded media, with the answer known in advance.
//!
//! The unit tests in `render::compositor` drive the pipeline with solid-colour
//! textures built in-process. These drive it with frames that came out of a
//! video file, through `MediaSourceProvider` — the same path the preview and
//! the exporter take — so they cover the joins the unit tests cannot: the
//! decoder's output size deciding the fit, the sRGB upload deciding what
//! blending means, and container rotation reaching the canvas.
//!
//! Every assertion is a pixel at a coordinate whose value was worked out from
//! the geometry beforehand. "It did not crash" is not a test of a compositor;
//! a compositor that draws everything at half scale does not crash.
//!
//! Tolerances are wide enough for a video codec and a GPU filter and no wider.
//! The interesting failures — a clip in the wrong place, the wrong clip on top,
//! blending in the wrong colour space — are all much larger than the tolerance.

mod support;

use std::sync::Arc;

use chukcut_lib::modules::media::MediaSourceProvider;
use chukcut_lib::modules::project::document::{
    AnimatableProperty, CanvasConfig, Crop, Easing, Keyframe, KeyframeTrack, Micros, Project,
    Track, TrackKind,
};
use chukcut_lib::modules::render::{Compositor, Frame, SourceProvider};

use support::{assert_pixel_near, material_for, segment};

/// Colours as they come back from the compositor, opaque.
const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A codec round trip and a bilinear sample between them; anything wrong with
/// the *geometry* is off by far more than this.
const CODEC_TOLERANCE: i32 = 8;

fn project(canvas: (u32, u32), background: [f32; 4]) -> Project {
    Project::new(
        "compositor integration",
        CanvasConfig {
            width: canvas.0,
            height: canvas.1,
            background,
        },
        30.0,
    )
}

/// Add `file` to the pool as `id` and place it on a new track for
/// `[0, duration)`.
fn place(project: &mut Project, id: &str, file: &std::path::Path, duration: Micros) -> String {
    let material = material_for(id, file).expect("probe the fixture");
    project.materials.videos.push(material);
    let segment = segment(id, 0, duration);
    let segment_id = segment.id.clone();
    let mut track = Track::new(TrackKind::Video, format!("V{}", project.tracks.len() + 1));
    track.segments.push(segment);
    project.tracks.push(track);
    // Track order is what decides compositing order, and the edit commands
    // normally maintain it; nothing here goes through them, so set it by hand.
    let index = project.tracks.len() as i32 - 1;
    for segment in &mut project.tracks[index as usize].segments {
        segment.render_index = index;
    }
    segment_id
}

/// Render `project` at `time` through the real decoder.
fn render(project: &Project, time: Micros, size: (u32, u32)) -> Option<Frame> {
    let ctx = support::gpu()?;
    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(project));
    Some(
        compositor
            .render(project, time, size, sources.as_ref())
            .expect("render"),
    )
}

/// `require_media!` and `require_gpu!` in one, returning the rendered frame.
macro_rules! rendered {
    ($project:expr, $time:expr, $size:expr) => {{
        let _ = require_gpu!();
        match render($project, $time, $size) {
            Some(frame) => frame,
            None => return,
        }
    }};
}

// ---------------------------------------------------------------------------
// Fit and letterboxing
// ---------------------------------------------------------------------------

#[test]
fn a_clip_whose_aspect_matches_the_canvas_covers_every_pixel_of_it() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 1.0, 1.0]);
    place(&mut p, "red", &media.solid_red_landscape, 2_000_000);

    let frame = rendered!(&p, 1_000_000, (320, 240));

    // Corners included: a fit that is off by a pixel leaves a line of the blue
    // background, and a fit that scales down leaves a border of it.
    for (x, y) in [(0, 0), (319, 0), (0, 239), (319, 239), (160, 120), (7, 200)] {
        assert_pixel_near(
            frame.pixel(x, y),
            RED,
            CODEC_TOLERANCE,
            &format!("({x},{y}) should be the clip"),
        );
    }
}

#[test]
fn a_wide_clip_on_a_square_canvas_is_letterboxed_with_the_background_showing() {
    let media = require_media!();
    // 16:9 source, 1:1 canvas. Width is the binding constraint, so the clip is
    // 320x180 centred in 320x320: 70 rows of background above and below.
    let mut p = project((320, 320), [0.0, 0.0, 1.0, 1.0]);
    place(&mut p, "wide", &media.solid_white_wide, 2_000_000);

    let frame = rendered!(&p, 500_000, (320, 320));

    assert_pixel_near(frame.pixel(160, 160), WHITE, CODEC_TOLERANCE, "centre");
    assert_pixel_near(
        frame.pixel(160, 100),
        WHITE,
        CODEC_TOLERANCE,
        "inside the top edge",
    );
    assert_pixel_near(
        frame.pixel(160, 220),
        WHITE,
        CODEC_TOLERANCE,
        "inside the bottom edge",
    );
    // 70 rows of letterbox: sample a few pixels clear of the seam so a
    // half-pixel of bilinear blur is not what the test is measuring.
    assert_pixel_near(frame.pixel(160, 5), BLUE, 2, "the top band is background");
    assert_pixel_near(frame.pixel(160, 60), BLUE, 2, "still background at row 60");
    assert_pixel_near(
        frame.pixel(160, 314),
        BLUE,
        2,
        "the bottom band is background",
    );
    // And nothing is pillarboxed: the clip reaches both side walls.
    assert_pixel_near(frame.pixel(2, 160), WHITE, CODEC_TOLERANCE, "left edge");
    assert_pixel_near(frame.pixel(317, 160), WHITE, CODEC_TOLERANCE, "right edge");
}

#[test]
fn a_tall_clip_on_a_wide_canvas_is_pillarboxed_instead() {
    let media = require_media!();
    // 3:4 source, 4:3 canvas. Height binds: 180x240 centred in 320x240, so
    // 70 columns of background left and right.
    let mut p = project((320, 240), [0.0, 0.0, 1.0, 1.0]);
    place(&mut p, "portrait", &media.solid_green_portrait, 2_000_000);

    let frame = rendered!(&p, 500_000, (320, 240));

    assert_pixel_near(frame.pixel(160, 120), GREEN, CODEC_TOLERANCE, "centre");
    assert_pixel_near(frame.pixel(5, 120), BLUE, 2, "the left band is background");
    assert_pixel_near(
        frame.pixel(314, 120),
        BLUE,
        2,
        "the right band is background",
    );
    assert_pixel_near(frame.pixel(160, 3), GREEN, CODEC_TOLERANCE, "top edge");
    assert_pixel_near(frame.pixel(160, 236), GREEN, CODEC_TOLERANCE, "bottom edge");
}

#[test]
fn an_empty_canvas_is_the_background_colour_and_nothing_is_decoded() {
    let _ = require_media!();
    let p = project((160, 120), [0.25, 0.5, 0.75, 1.0]);
    let frame = rendered!(&p, 0, (160, 120));

    // 0.25/0.5/0.75 linear, encoded to sRGB on the way out of the target.
    let want = [
        (linear_to_srgb(0.25) * 255.0).round() as u8,
        (linear_to_srgb(0.5) * 255.0).round() as u8,
        (linear_to_srgb(0.75) * 255.0).round() as u8,
        255,
    ];
    assert_pixel_near(frame.pixel(80, 60), want, 2, "the background");
    assert_eq!(frame.data.len(), 160 * 120 * 4);
}

/// The transfer function the render target applies on the way out.
fn linear_to_srgb(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

// ---------------------------------------------------------------------------
// Stacking and opacity
// ---------------------------------------------------------------------------

#[test]
fn the_higher_track_paints_over_the_lower_one() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    place(&mut p, "under", &media.solid_red_landscape, 2_000_000);
    place(&mut p, "over", &media.quadrants, 1_000_000);

    let frame = rendered!(&p, 500_000, (320, 240));

    // The quadrant clip is fully opaque and the same shape as the canvas, so
    // none of the red underneath should be visible anywhere.
    assert_pixel_near(
        frame.pixel(80, 60),
        RED,
        CODEC_TOLERANCE,
        "top left quadrant",
    );
    assert_pixel_near(
        frame.pixel(240, 60),
        GREEN,
        CODEC_TOLERANCE,
        "top right quadrant",
    );
    assert_pixel_near(
        frame.pixel(80, 180),
        BLUE,
        CODEC_TOLERANCE,
        "bottom left quadrant",
    );
    assert_pixel_near(
        frame.pixel(240, 180),
        WHITE,
        CODEC_TOLERANCE,
        "bottom right quadrant",
    );
}

#[test]
fn a_half_transparent_clip_blends_with_the_one_underneath_in_linear_light() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    place(&mut p, "under", &media.solid_red_landscape, 2_000_000);
    let over = place(&mut p, "over", &media.solid_white_wide, 2_000_000);
    p.segment_mut(&over).expect("the clip").transform.opacity = 0.5;

    let frame = rendered!(&p, 500_000, (320, 240));

    // Half white over red: red stays saturated, the other two channels come up
    // to half *in linear light*, which is 188 once the target re-encodes it to
    // sRGB. A compositor blending sRGB values directly would produce 128 here,
    // and cross-fades would look muddy for exactly that reason.
    let [r, g, b, a] = frame.pixel(160, 120);
    let half = (linear_to_srgb(0.5) * 255.0).round() as i32;
    assert!(
        (r as i32 - 255).abs() <= CODEC_TOLERANCE,
        "red channel should stay saturated, got {r}"
    );
    for (channel, value) in [("green", g), ("blue", b)] {
        assert!(
            (value as i32 - half).abs() <= CODEC_TOLERANCE,
            "{channel} should be {half} (half of white in linear light), got {value}"
        );
    }
    assert_eq!(a, 255, "the composite is opaque");

    // The white clip is 16:9 on a 4:3 canvas, so the letterbox bands show the
    // red underneath at full strength — which also proves the blend is per
    // pixel and not a whole-frame fade.
    assert_pixel_near(
        frame.pixel(160, 5),
        RED,
        CODEC_TOLERANCE,
        "above the wide clip",
    );
}

#[test]
fn a_fully_transparent_clip_is_not_drawn_at_all() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    let over = place(&mut p, "over", &media.solid_red_landscape, 2_000_000);
    p.segment_mut(&over).expect("the clip").transform.opacity = 0.0;

    let frame = rendered!(&p, 500_000, (320, 240));
    assert_pixel_near(frame.pixel(160, 120), BLACK, 2, "nothing was painted");
}

// ---------------------------------------------------------------------------
// Crop and transform
// ---------------------------------------------------------------------------

#[test]
fn a_crop_keeps_only_the_selected_region_and_grows_it_to_fit() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 1.0, 1.0]);
    let id = place(&mut p, "quads", &media.quadrants, 1_000_000);
    // Keep the left half. The kept region is 160x240 — 2:3 — and cropping
    // happens *before* the fit, so it is height-bound on a 4:3 canvas and ends
    // up 160 wide, centred, with 80 columns of background either side.
    p.segment_mut(&id).expect("the clip").crop = Some(Crop {
        left: 0.0,
        top: 0.0,
        right: 0.5,
        bottom: 1.0,
    });

    let frame = rendered!(&p, 500_000, (320, 240));

    assert_pixel_near(frame.pixel(160, 60), RED, CODEC_TOLERANCE, "kept top left");
    assert_pixel_near(
        frame.pixel(160, 180),
        BLUE,
        CODEC_TOLERANCE,
        "kept bottom left",
    );
    assert_pixel_near(
        frame.pixel(100, 60),
        RED,
        CODEC_TOLERANCE,
        "left of centre, still kept",
    );
    // The cropped-away half is gone rather than blank: the background shows.
    assert_pixel_near(frame.pixel(10, 120), BLUE, 2, "cropped away on the left");
    assert_pixel_near(frame.pixel(310, 120), BLUE, 2, "cropped away on the right");
}

#[test]
fn a_transform_moves_and_scales_the_clip_by_the_documented_amounts() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    let id = place(&mut p, "red", &media.solid_red_landscape, 2_000_000);
    {
        let transform = &mut p.segment_mut(&id).expect("the clip").transform;
        // Half size, then moved right by a quarter of the canvas width.
        // `position` is in half-canvas units, so 0.5 is a quarter of 320 = 80.
        transform.scale = [0.5, 0.5];
        transform.position = [0.5, 0.0];
    }

    let frame = rendered!(&p, 500_000, (320, 240));

    // The clip is 160x120 centred on (240, 120): x in 160..320, y in 60..180.
    assert_pixel_near(
        frame.pixel(240, 120),
        RED,
        CODEC_TOLERANCE,
        "the clip's new centre",
    );
    assert_pixel_near(
        frame.pixel(170, 120),
        RED,
        CODEC_TOLERANCE,
        "inside the left edge",
    );
    assert_pixel_near(
        frame.pixel(310, 120),
        RED,
        CODEC_TOLERANCE,
        "inside the right edge",
    );
    assert_pixel_near(frame.pixel(140, 120), BLACK, 2, "outside it to the left");
    assert_pixel_near(frame.pixel(240, 30), BLACK, 2, "above it");
    assert_pixel_near(frame.pixel(240, 210), BLACK, 2, "below it");
    // Where it used to be is now empty.
    assert_pixel_near(frame.pixel(40, 120), BLACK, 2, "the old position");
}

// ---------------------------------------------------------------------------
// Keyframes
// ---------------------------------------------------------------------------

#[test]
fn a_keyframed_opacity_ramp_reads_correctly_at_three_points_along_it() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    let id = place(&mut p, "white", &media.solid_red_landscape, 2_000_000);
    {
        let segment = p.segment_mut(&id).expect("the clip");
        // A two-second fade from invisible to opaque, authored against the
        // segment rather than the timeline.
        segment.keyframes = vec![KeyframeTrack {
            property: AnimatableProperty::Opacity,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: 2_000_000,
                    value: 1.0,
                    easing: Easing::Linear,
                },
            ],
        }];
    }

    // Start of the ramp: transparent, so the background survives.
    let start = rendered!(&p, 0, (320, 240));
    assert_pixel_near(start.pixel(160, 120), BLACK, 2, "the start of the fade");

    // Halfway: red at half alpha over black is half of red in linear light.
    let middle = rendered!(&p, 1_000_000, (320, 240));
    let half = (linear_to_srgb(0.5) * 255.0).round() as i32;
    let [r, g, b, _] = middle.pixel(160, 120);
    assert!(
        (r as i32 - half).abs() <= CODEC_TOLERANCE,
        "halfway through the fade the red channel should be {half}, got {r}"
    );
    assert!(
        g < 16 && b < 16,
        "and nothing else should appear, got {g} and {b}"
    );

    // The last instant of the clip: all but opaque.
    let end = rendered!(&p, 1_999_999, (320, 240));
    let [r, _, _, _] = end.pixel(160, 120);
    assert!(
        r as i32 > 240,
        "the end of the fade should be almost fully opaque, got {r}"
    );
}

// ---------------------------------------------------------------------------
// Rotation reaching the canvas
// ---------------------------------------------------------------------------

#[test]
fn a_rotated_source_composites_upright_and_at_its_display_aspect() {
    let media = require_media!();
    // The file is coded 320x240 and displays 240x320. On a 240x320 canvas the
    // upright picture fits exactly; if rotation were lost somewhere between the
    // decoder and the compositor it would be letterboxed and sideways.
    let mut p = project((240, 320), [0.0, 0.0, 1.0, 1.0]);
    place(&mut p, "rotated", &media.quadrants_rot90, 1_000_000);

    let frame = rendered!(&p, 500_000, (240, 320));

    assert_pixel_near(
        frame.pixel(60, 80),
        BLUE,
        CODEC_TOLERANCE,
        "displayed top left",
    );
    assert_pixel_near(
        frame.pixel(180, 80),
        RED,
        CODEC_TOLERANCE,
        "displayed top right",
    );
    assert_pixel_near(
        frame.pixel(60, 240),
        WHITE,
        CODEC_TOLERANCE,
        "displayed bottom left",
    );
    assert_pixel_near(
        frame.pixel(180, 240),
        GREEN,
        CODEC_TOLERANCE,
        "displayed bottom right",
    );
    // Nothing letterboxed: the rotated picture fills the portrait canvas.
    // Sampled off the vertical midline, which is the seam between two
    // quadrants and therefore a blend of both.
    assert_pixel_near(frame.pixel(60, 2), BLUE, CODEC_TOLERANCE, "top edge");
    assert_pixel_near(frame.pixel(60, 317), WHITE, CODEC_TOLERANCE, "bottom edge");
    assert_pixel_near(frame.pixel(2, 80), BLUE, CODEC_TOLERANCE, "left edge");
    assert_pixel_near(frame.pixel(237, 240), GREEN, CODEC_TOLERANCE, "right edge");
}

// ---------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------

#[test]
fn a_clip_is_drawn_only_while_the_playhead_is_inside_it() {
    let media = require_media!();
    let mut p = project((160, 120), [0.0, 0.0, 0.0, 1.0]);
    let material = material_for("red", &media.solid_red_landscape).expect("probe");
    p.materials.videos.push(material);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment("red", 1_000_000, 1_000_000));
    p.tracks.push(track);

    assert_pixel_near(
        rendered!(&p, 999_999, (160, 120)).pixel(80, 60),
        BLACK,
        2,
        "just before the clip starts",
    );
    assert_pixel_near(
        rendered!(&p, 1_000_000, (160, 120)).pixel(80, 60),
        RED,
        CODEC_TOLERANCE,
        "the clip's first instant",
    );
    assert_pixel_near(
        rendered!(&p, 1_999_999, (160, 120)).pixel(80, 60),
        RED,
        CODEC_TOLERANCE,
        "the clip's last instant",
    );
    // Half open, exactly like every other range in the document.
    assert_pixel_near(
        rendered!(&p, 2_000_000, (160, 120)).pixel(80, 60),
        BLACK,
        2,
        "the instant the clip ends",
    );
}

#[test]
fn the_source_range_decides_which_part_of_the_file_is_shown() {
    let media = require_media!();
    // The counter clip writes its own frame index into its pixels, so this can
    // check *which* frame the compositor asked the decoder for — which is the
    // one place `Segment::source_time_at` is actually exercised end to end.
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    let material = material_for("counter", &media.counter).expect("probe");
    p.materials.videos.push(material);

    let mut segment = support::segment("counter", 1_000_000, 1_000_000);
    // Show the second starting two seconds into the file.
    segment.source_range =
        chukcut_lib::modules::project::document::TimeRange::new(2_000_000, 1_000_000);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment);
    p.tracks.push(track);

    let frame = rendered!(&p, 1_500_000, (320, 240));
    // Half a second into the segment is 2.5 s into the file, which at 30 fps
    // is frame 75.
    assert_eq!(
        support::read_counter_rgba(&frame.data, frame.width, frame.height),
        Some(75),
        "the compositor asked for the wrong instant of the source"
    );
}

#[test]
fn a_speed_change_maps_timeline_time_onto_source_time() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 0.0, 1.0]);
    let material = material_for("counter", &media.counter).expect("probe");
    p.materials.videos.push(material);

    let mut segment = support::segment("counter", 0, 2_000_000);
    segment.speed = 2.0;
    segment.source_range = chukcut_lib::modules::project::document::TimeRange::new(0, 4_000_000);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment);
    p.tracks.push(track);

    // One second into a segment running at 2x is two seconds into the file:
    // frame 60.
    let frame = rendered!(&p, 1_000_000, (320, 240));
    assert_eq!(
        support::read_counter_rgba(&frame.data, frame.width, frame.height),
        Some(60)
    );
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn rendering_the_same_frame_twice_produces_the_same_bytes() {
    let media = require_media!();
    let _ = require_gpu!();

    let mut p = project((320, 240), [0.05, 0.1, 0.2, 1.0]);
    place(&mut p, "quads", &media.quadrants, 1_000_000);
    let id = place(&mut p, "wide", &media.solid_white_wide, 1_000_000);
    p.segment_mut(&id).expect("the clip").transform.opacity = 0.6;

    // The preview and the exporter share one compositor and must agree about
    // every pixel; if `render_frame` were not a pure function of its arguments,
    // an export would not match what the user approved.
    let ctx = support::gpu().expect("checked above");
    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&p));

    let first = compositor
        .render(&p, 500_000, (320, 240), sources.as_ref())
        .expect("render");
    let second = compositor
        .render(&p, 500_000, (320, 240), sources.as_ref())
        .expect("render");
    assert_eq!(
        first.data, second.data,
        "the same frame rendered differently"
    );

    // A fresh provider decodes from scratch rather than answering out of its
    // texture cache, which is the version an export would produce.
    let cold: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&p));
    let third = compositor
        .render(&p, 500_000, (320, 240), cold.as_ref())
        .expect("render");
    assert_eq!(
        first.data, third.data,
        "a cold decoder produced a different frame from a warm one"
    );
}

#[test]
fn rendering_at_a_smaller_size_is_the_same_composition_scaled_down() {
    let media = require_media!();
    let mut p = project((320, 240), [0.0, 0.0, 1.0, 1.0]);
    place(&mut p, "wide", &media.solid_white_wide, 1_000_000);

    // The preview renders the canvas small and the export renders it large;
    // both have to frame the shot identically or the delivered file is not what
    // was approved. The 16:9 clip letterboxes at 30/240 of the height either
    // way — 30 rows at full size, 15 at half.
    let full = rendered!(&p, 500_000, (320, 240));
    let half = rendered!(&p, 500_000, (160, 120));

    assert_pixel_near(full.pixel(160, 120), WHITE, CODEC_TOLERANCE, "full centre");
    assert_pixel_near(half.pixel(80, 60), WHITE, CODEC_TOLERANCE, "half centre");
    assert_pixel_near(full.pixel(160, 5), BLUE, 2, "full letterbox");
    assert_pixel_near(half.pixel(80, 2), BLUE, 2, "half letterbox");
    // Just inside the clip on both.
    assert_pixel_near(
        full.pixel(160, 40),
        WHITE,
        CODEC_TOLERANCE,
        "full, below the band",
    );
    assert_pixel_near(
        half.pixel(80, 20),
        WHITE,
        CODEC_TOLERANCE,
        "half, below the band",
    );
}

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

#[test]
fn a_missing_media_file_draws_the_placeholder_rather_than_failing_the_frame() {
    let media = require_media!();
    let mut p = project((160, 120), [0.0, 1.0, 0.0, 1.0]);
    place(&mut p, "red", &media.solid_red_landscape, 2_000_000);
    // Relink is a UI concern; a project with a dead link still has to open and
    // still has to preview. It used to leave a hole — the background showed
    // through — which read as the clip having been deleted. Now the clip's
    // area is an unmistakable flat field; `tests/missing_media.rs` covers the
    // whole missing-media contract around it.
    p.materials.videos[0].path = "/nonexistent/gone.mp4".into();

    let frame = rendered!(&p, 500_000, (160, 120));
    assert_pixel_near(
        frame.pixel(80, 60),
        chukcut_lib::modules::media::MISSING_MEDIA_RGBA,
        6,
        "the clip's area is the missing-media placeholder",
    );
}
