//! Crop keyframes end to end: the edit layer (keyframes at a time, the
//! diamond, reset, undo to the byte), files that round-trip, the compositor
//! drawing the crop the keyframes give at each instant, and an export with
//! the same pictures.
//!
//! The quadrants fixture is 320x240 with red, green, blue and white corners,
//! so which part of the picture a crop keeps is read off a pixel.

mod support;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::inspector::edit::{
    crop_keyframe_command, set_crop_command, toggle_crop_keyframe_command,
};
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    AnimatableProperty, CanvasConfig, Crop, Micros, Project, Track, TrackKind,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};
use chukcut_engine::modules::timeline::ops::split_at;
use chukcut_engine::modules::timeline::History;

use support::{assert_pixel_near, canonical, material_for, pixel, segment};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
const CODEC_TOLERANCE: i32 = 8;

const LEFT_HALF: Crop = Crop {
    left: 0.0,
    top: 0.0,
    right: 0.5,
    bottom: 1.0,
};

/// A 1 s clip of material `m` on a 320x240, 30 fps canvas, with no media
/// behind it: enough for the edit layer.
fn one_clip() -> (Project, String) {
    let mut project = Project::new(
        "crop keyframes",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    let mut track = Track::new(TrackKind::Video, "V1");
    let clip = segment("m", 0, 1_000_000);
    let id = clip.id.clone();
    track.segments.push(clip);
    project.tracks.push(track);
    (project, id)
}

/// The full picture at the clip's start, its left half from 0.9 s on.
fn animate(project: &mut Project, history: &mut History, id: &str) {
    let full = crop_keyframe_command(project, id, None, 0).expect("a key at the start");
    history.apply(project, full).unwrap();
    let half = crop_keyframe_command(project, id, Some(LEFT_HALF), 900_000).expect("a later key");
    history.apply(project, half).unwrap();
}

fn crop_at(project: &Project, id: &str, time: Micros) -> Crop {
    let (_, s) = project.segment(id).unwrap();
    s.crop_at(time).unwrap_or_default()
}

#[test]
fn keyframes_at_two_times_interpolate_and_undo_to_the_byte() {
    let (mut project, id) = one_clip();
    let before = canonical(&project);
    let mut history = History::default();
    animate(&mut project, &mut history, &id);

    let (_, s) = project.segment(&id).unwrap();
    let crop_tracks: Vec<_> = s
        .keyframes
        .iter()
        .filter(|t| t.property.is_crop())
        .collect();
    assert_eq!(crop_tracks.len(), 4, "all four edges are keyed");
    assert!(crop_tracks.iter().all(|t| t.keyframes.len() == 2));
    assert_eq!(crop_at(&project, &id, 0).right, 1.0);
    assert!((crop_at(&project, &id, 450_000).right - 0.75).abs() < 1e-6);
    assert_eq!(crop_at(&project, &id, 950_000).right, 0.5);

    // Setting the crop again at a keyed instant (within half a frame) moves
    // that keyframe instead of adding one.
    let tweak = crop_keyframe_command(
        &project,
        &id,
        Some(Crop {
            right: 0.6,
            ..LEFT_HALF
        }),
        910_000,
    )
    .unwrap();
    history.apply(&mut project, tweak).unwrap();
    let (_, s) = project.segment(&id).unwrap();
    let right = s
        .keyframes
        .iter()
        .find(|t| t.property == AnimatableProperty::CropRight)
        .unwrap();
    assert_eq!(right.keyframes.len(), 2);
    assert_eq!(right.keyframes[1].time, 900_000);
    assert_eq!(right.keyframes[1].value, 0.6);

    for _ in 0..3 {
        history.undo(&mut project).unwrap();
    }
    assert_eq!(
        canonical(&project),
        before,
        "three undos, the file as it was"
    );
}

#[test]
fn the_diamond_keys_what_is_shown_and_takes_it_away_again() {
    let (mut project, id) = one_clip();
    project.segment_mut(&id).unwrap().crop = Some(LEFT_HALF);
    let before = canonical(&project);
    let mut history = History::default();

    let add = toggle_crop_keyframe_command(&project, &id, 300_000).unwrap();
    history.apply(&mut project, add).unwrap();
    // Adding a keyframe changes nothing on screen: it holds the static crop.
    for t in [0, 300_000, 999_000] {
        let c = crop_at(&project, &id, t);
        assert_eq!((c.left, c.right), (0.0, 0.5), "at {t}");
    }
    // A static set is refused while the crop is animated; a keyed set works.
    assert!(set_crop_command(&project, &id, Some(LEFT_HALF)).is_err());

    let remove = toggle_crop_keyframe_command(&project, &id, 300_000 + 10_000).unwrap();
    history.apply(&mut project, remove).unwrap();
    assert_eq!(
        canonical(&project),
        before,
        "the toggle came back to the file"
    );

    // A time outside the clip is refused, not clamped.
    assert!(toggle_crop_keyframe_command(&project, &id, 2_000_000).is_err());
}

#[test]
fn reset_removes_the_crop_and_its_keyframes_in_one_undo_step() {
    let (mut project, id) = one_clip();
    let mut history = History::default();
    animate(&mut project, &mut history, &id);
    let animated = canonical(&project);

    let reset = set_crop_command(&project, &id, None).unwrap();
    history.apply(&mut project, reset).unwrap();
    let (_, s) = project.segment(&id).unwrap();
    assert!(s.crop.is_none() && !s.has_crop_keyframes());

    history.undo(&mut project).unwrap();
    assert_eq!(canonical(&project), animated);
}

#[test]
fn a_split_keeps_the_crop_moving_across_the_cut() {
    let (mut project, id) = one_clip();
    let mut history = History::default();
    animate(&mut project, &mut history, &id);
    let whole: Vec<Crop> = [100_000, 450_000, 600_000, 950_000]
        .iter()
        .map(|&t| crop_at(&project, &id, t))
        .collect();
    let split = split_at(&project, &id, 450_000).expect("split");
    history.apply(&mut project, split).unwrap();
    let lane = &project.tracks[0];
    assert_eq!(lane.segments.len(), 2);
    for (i, &t) in [100_000, 450_000, 600_000, 950_000].iter().enumerate() {
        let s = lane.segment_at(t).unwrap();
        let c = s.crop_at(t).unwrap_or_default();
        assert!(
            (c.right - whole[i].right).abs() < 1e-5,
            "at {t}: {} after the split, {} before",
            c.right,
            whole[i].right
        );
    }
}

#[test]
fn crop_keyframes_save_and_open_unchanged_and_old_files_stay_as_they_were() {
    let (mut project, id) = one_clip();
    let mut history = History::default();
    animate(&mut project, &mut history, &id);
    let saved = serde_json::to_string_pretty(&project).unwrap();
    assert!(
        saved.contains("\"crop_right\""),
        "the edge is named in the file"
    );
    let opened: Project = serde_json::from_str(&saved).expect("the file opens");
    assert_eq!(canonical(&opened), canonical(&project));

    // A file from before crop keyframes: a static crop and a transform
    // keyframe. It reads, and it writes back the same bytes.
    let (mut old, old_id) = one_clip();
    {
        let s = old.segment_mut(&old_id).unwrap();
        s.crop = Some(LEFT_HALF);
    }
    let rotation = chukcut_engine::modules::timeline::ops::EditCommand::AddKeyframe {
        segment_id: old_id.clone(),
        property: AnimatableProperty::Rotation,
        keyframe: chukcut_engine::modules::project::document::Keyframe {
            time: 0,
            value: 10.0,
            easing: Default::default(),
        },
    };
    history.apply(&mut old, rotation).unwrap();
    let bytes = serde_json::to_string_pretty(&old).unwrap();
    assert!(!bytes.contains("crop_"));
    let reopened: Project = serde_json::from_str(&bytes).unwrap();
    assert_eq!(serde_json::to_string_pretty(&reopened).unwrap(), bytes);
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

/// The quadrants fixture for its whole second, with the crop animated from
/// the full picture to its left half at 0.9 s.
fn quadrants() -> Option<(Project, String)> {
    let media = support::media().ok()?;
    let (mut project, _) = one_clip();
    project.tracks.clear();
    project
        .materials
        .videos
        .push(material_for("quads", &media.quadrants).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    let clip = segment("quads", 0, 1_000_000);
    let id = clip.id.clone();
    track.segments.push(clip);
    project.tracks.push(track);
    let mut history = History::default();
    animate(&mut project, &mut history, &id);
    Some((project, id))
}

/// What the crop shows at three instants, as `(time, pixel, colour, what)`.
/// At the start the whole picture fills the canvas; half way the right edge
/// is at 0.75, a 240x240 picture centred (x 40..280); at the end the left half,
/// 160x240 centred (x 80..240).
/// A timeline instant, a canvas pixel, the colour there and what it shows.
type Probe = (Micros, (u32, u32), [u8; 4], &'static str);

const EXPECTED: [Probe; 6] = [
    (
        0,
        (300, 60),
        GREEN,
        "start: the top right corner is on screen",
    ),
    (0, (20, 60), RED, "start: the picture reaches the left edge"),
    (450_000, (270, 60), GREEN, "half way: green still shows"),
    (450_000, (20, 60), BLACK, "half way: pillarboxed"),
    (
        950_000,
        (300, 60),
        BLACK,
        "end: the right half is cropped away",
    ),
    (
        950_000,
        (160, 60),
        RED,
        "end: the left half fills the middle",
    ),
];

#[test]
fn the_compositor_draws_the_crop_of_each_instant() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((project, _)) = quadrants() else {
        return;
    };
    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    for (time, (x, y), want, what) in EXPECTED {
        let frame = compositor
            .render(&project, time + 10, (320, 240), sources.as_ref())
            .expect("render");
        assert_pixel_near(frame.pixel(x, y), want, CODEC_TOLERANCE, what);
    }
}

#[test]
fn an_export_shows_the_same_animated_crop() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((project, _)) = quadrants() else {
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("crop-keyframes");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("animated-crop.mp4");
    let _ = std::fs::remove_file(&path);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: None,
    };
    let settings = resolve_settings(&project, &request).expect("settings");
    let job = ExportJob {
        job_id: "crop".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let sink = FnSink({
        let seen = Arc::new(Mutex::new(0usize));
        move |_| *seen.lock().unwrap() += 1
    });
    run_export(&job, &sink).expect("export");

    let mut decoder = VideoDecoder::open(&path).expect("open the export");
    for (time, (x, y), want, what) in EXPECTED {
        // The frame the instant falls in, sampled a little inside it.
        let frame_start = (time as f64 / (1e6 / 30.0)).floor() * (1e6 / 30.0);
        let frame = decoder
            .seek_and_decode(frame_start as Micros + 16_000)
            .expect("decode");
        // The export goes through 4:2:0, which smears colour at an edge by a
        // pixel or two; the probes are far from every edge.
        assert_pixel_near(
            pixel(&frame.data, frame.width, x, y),
            want,
            CODEC_TOLERANCE * 2,
            &format!("export, {what}"),
        );
    }
    let _ = std::fs::remove_file(&path);
}
