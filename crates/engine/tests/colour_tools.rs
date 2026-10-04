//! Auto adjust, colour match and grade presets, end to end: decoded media,
//! the command layer, the undo stack, and the compositor's pixels.
//!
//! The colour tools fit their values on a CPU simulation of the grade
//! (`grading::pixels`). These tests check the claim that matters to a user:
//! the picture the *compositor* draws with the fitted grade has the
//! statistics the fit aimed for.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use chukcut_engine::modules::grading::auto::Measure;
use chukcut_engine::modules::grading::commands::{self as grading, AutoAdjust, ColourMatch};
use chukcut_engine::modules::grading::pixels::{LabStats, Samples};
use chukcut_engine::modules::inspector::edit::GradeEdit;
use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::render::{Compositor, SourceProvider};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::state::AppState;

const W: u32 = 320;
const H: u32 = 180;
const DURATION: Micros = 1_000_000;

fn fixture(name: &str, filter: &str) -> Option<PathBuf> {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/colour-tools-v1");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    if path.exists() {
        return Some(path);
    }
    let tmp = path.with_extension("tmp.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size={W}x{H}:rate=30:duration=1,{filter}"))
        .args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "12", "-pix_fmt", "yuv420p",
        ])
        .arg(&tmp)
        .stdout(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// The test picture as filmed well, and as filmed with a warm cast, a stop
/// under and a lifted black.
fn fixtures() -> Option<(PathBuf, PathBuf)> {
    Some((
        fixture("good.mp4", "null")?,
        fixture(
            "warm-dark.mp4",
            "colorchannelmixer=rr=1.0:gg=0.82:bb=0.55,lutyuv=y='16+val*0.55'",
        )?,
    ))
}

fn video(id: &str, path: &Path) -> VideoMaterial {
    VideoMaterial {
        id: id.into(),
        path: path.to_string_lossy().into(),
        width: W,
        height: H,
        duration: DURATION,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    }
}

fn clip(id: &str, material: &str, start: Micros) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(start, DURATION),
        source_range: TimeRange::new(0, DURATION),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// A timeline: the good clip ("ref"), then the spoiled one ("bad").
fn state(good: &Path, bad: &Path) -> Arc<AppState> {
    let mut project = Project::new("colour", CanvasConfig::default(), 30.0);
    project.canvas.width = W;
    project.canvas.height = H;
    project.materials.videos.push(video("g", good));
    project.materials.videos.push(video("b", bad));
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(clip("ref", "g", 0));
    track.segments.push(clip("bad", "b", DURATION));
    project.tracks.push(track);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

/// The compositor's frame at `t`, as samples.
fn rendered(state: &AppState, t: Micros) -> Option<Samples> {
    let ctx = chukcut_engine::modules::gpu::render_context()?;
    let project = state.project.read().clone().unwrap();
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let frame = Compositor::new(ctx)
        .render(&project, t, (W, H), sources.as_ref())
        .expect("render");
    let mut samples = Samples::default();
    samples.add_rgba(&frame.data, 1);
    Some(samples)
}

fn grade(state: &AppState, id: &str) -> GradeEdit {
    let guard = state.project.read();
    GradeEdit::of_segment(guard.as_ref().unwrap(), id).unwrap()
}

fn cast(m: &Measure) -> f32 {
    m.cast[0].hypot(m.cast[1])
}

#[test]
fn auto_adjust_balances_what_the_compositor_draws_in_one_undo_step() {
    let Some((good, bad)) = fixtures() else {
        eprintln!("skipping: ffmpeg could not generate the fixtures");
        return;
    };
    let state = state(&good, &bad);
    let done = grading::grading_auto_adjust(
        &state,
        AutoAdjust {
            segment_id: "bad".into(),
            amount: 1.0,
        },
    )
    .unwrap();
    let c = done.controls;
    assert!(c.exposure > 0.2, "{c:?}");
    assert!(c.temperature < -0.05, "a warm cast is cooled: {c:?}");
    // testsrc2 has almost no grey in it, so auto adjust trusts its cast
    // measurement only half way (`grading::auto::neutral_pool`).
    assert!(cast(&done.after) < cast(&done.before) * 0.7, "{done:?}");
    assert!(done.after.key > done.before.key * 1.3, "{done:?}");
    let g = grade(&state, "bad");
    assert_eq!(g.grade.exposure, c.exposure);
    assert_eq!(g.temperature, c.temperature);

    // The compositor's picture: brighter, less warm, than the ungraded one.
    if let Some(after) = rendered(&state, DURATION + 500_000) {
        timeline_undo(&state).unwrap();
        assert!(
            grade(&state, "bad").is_identity(),
            "one undo takes it all back"
        );
        let before = rendered(&state, DURATION + 500_000).unwrap();
        let (mb, ma) = (Measure::of(&before), Measure::of(&after));
        assert!(ma.key > mb.key * 1.3, "key {} -> {}", mb.key, ma.key);
        let (lb, la) = (LabStats::of(&before), LabStats::of(&after));
        assert!(
            la.mean[2] < lb.mean[2] - 3.0,
            "b* (yellow) {} -> {}",
            lb.mean[2],
            la.mean[2]
        );
    } else {
        eprintln!("no GPU: the compositor half is skipped");
    }
}

#[test]
fn colour_match_brings_the_rendered_clip_to_the_reference() {
    let Some((good, bad)) = fixtures() else {
        eprintln!("skipping: ffmpeg could not generate the fixtures");
        return;
    };
    let state = state(&good, &bad);
    let done = grading::grading_match(
        &state,
        ColourMatch {
            segment_id: "bad".into(),
            reference_id: "ref".into(),
            at: None,
            amount: 1.0,
        },
    )
    .unwrap();
    assert!(
        done.distance_after < done.distance_before * 0.3,
        "{} -> {}",
        done.distance_before,
        done.distance_after
    );
    // The same frame of both clips, drawn: the graded "bad" against "ref".
    let (Some(reference), Some(matched)) = (
        rendered(&state, 500_000),
        rendered(&state, DURATION + 500_000),
    ) else {
        eprintln!("no GPU: the compositor half is skipped");
        return;
    };
    let r = LabStats::of(&reference);
    let m = LabStats::of(&matched);
    timeline_undo(&state).unwrap();
    let unmatched = LabStats::of(&rendered(&state, DURATION + 500_000).unwrap());
    let (d0, d1) = (unmatched.distance(&r), m.distance(&r));
    eprintln!("rendered L*a*b* distance to the reference: {d0:.2} -> {d1:.2}");
    assert!(d1 < d0 * 0.35, "rendered distance {d0} -> {d1}");
    // The fit's prediction and the compositor agree to within a few units.
    assert!(
        (m.mean[0] - done.after.mean[0]).abs() < 3.0,
        "L* predicted {} drawn {}",
        done.after.mean[0],
        m.mean[0]
    );

    // Matching one frame of the reference, and refusals.
    grading::grading_match(
        &state,
        ColourMatch {
            segment_id: "bad".into(),
            reference_id: "ref".into(),
            at: Some(200_000),
            amount: 0.5,
        },
    )
    .unwrap();
    let error = grading::grading_match(
        &state,
        ColourMatch {
            segment_id: "bad".into(),
            reference_id: "bad".into(),
            at: None,
            amount: 1.0,
        },
    )
    .unwrap_err();
    assert!(error.contains("another clip"), "{error}");
    let error = grading::grading_match(
        &state,
        ColourMatch {
            segment_id: "bad".into(),
            reference_id: "gone".into(),
            at: None,
            amount: 1.0,
        },
    )
    .unwrap_err();
    assert!(error.contains("reference"), "{error}");
}

#[test]
fn a_saved_preset_gives_another_clip_the_same_grade() {
    let Some((good, bad)) = fixtures() else {
        eprintln!("skipping: ffmpeg could not generate the fixtures");
        return;
    };
    // Presets live in the data directory: point it at the test's own.
    let data = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-data/colour-tools")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&data);
    std::env::set_var("XDG_DATA_HOME", &data);

    let state = state(&good, &bad);
    grading::grading_auto_adjust(
        &state,
        AutoAdjust {
            segment_id: "bad".into(),
            amount: 1.0,
        },
    )
    .unwrap();
    let saved = grade(&state, "bad");
    assert_eq!(grading::grading_preset_name(), "Preset 1");
    let entry =
        grading::grading_save_preset(&state, "bad".into(), "Fix warm".into(), false).unwrap();
    assert!(Path::new(&entry.path).starts_with(&data));
    assert_eq!(grading::grading_presets(), vec![entry]);
    grading::grading_apply_preset(&state, "ref".into(), "Fix warm".into()).unwrap();
    assert_eq!(grade(&state, "ref"), saved);
    timeline_undo(&state).unwrap();
    assert!(grade(&state, "ref").is_identity());

    // An ungraded clip has nothing to save; a removed preset is gone.
    let error =
        grading::grading_save_preset(&state, "ref".into(), "Nothing".into(), false).unwrap_err();
    assert!(error.contains("no grade"), "{error}");
    grading::grading_delete_preset("Fix warm".into()).unwrap();
    assert!(grading::grading_presets().is_empty());
}
