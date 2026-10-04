//! A clip's matte as a mask for its own grade and effects ("grade only the
//! person", "blur only the background"), without a second copy of the clip:
//! through the command layer and the undo stack, and through the compositor
//! in the preview's RGBA render and the export's NV12 render.
//!
//! No model is needed: the tests write a matte of their own into the cache
//! (left half the subject, right half the rest) and check that the grade or
//! the effect lands on exactly that half.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use chukcut_engine::modules::fx::catalog::GAUSSIAN_BLUR;
use chukcut_engine::modules::matting::{cache, commands as matting};
use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::project::compositing::{BackgroundRemoval, MatteTarget};
use chukcut_engine::modules::project::document::{
    CanvasConfig, ColorAdjustMaterial, Micros, Project, Segment, TimeRange, Track, TrackKind,
    Transform, VideoMaterial,
};
use chukcut_engine::modules::project::{EffectMaterial, EffectValue};
use chukcut_engine::modules::render::{Compositor, SourceProvider};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::state::AppState;

const W: u32 = 320;
const H: u32 = 180;
const DURATION: Micros = 1_000_000;

fn fixture_dir() -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/matte-masks-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// One second of an 8-pixel checkerboard, 320x180, 30 fps: detail a blur
/// visibly removes, and a mid level a grade visibly moves. Its own file per
/// test, so the matte caches (keyed by the file) of two tests never meet.
fn checkers(name: &str) -> Option<PathBuf> {
    let path = fixture_dir().join(name);
    if path.exists() {
        return Some(path);
    }
    let tmp = path.with_extension("tmp.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!(
            "color=c=gray:size={W}x{H}:rate=30:duration=1,\
             geq=lum='if(mod(floor(X/8)+floor(Y/8),2),180,70)':cb=128:cr=128"
        ))
        .args([
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "8", "-pix_fmt", "yuv420p",
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

fn state_with(path: &std::path::Path, grade: bool, blur: bool) -> Arc<AppState> {
    let mut project = Project::new("masks", CanvasConfig::default(), 30.0);
    project.canvas.width = W;
    project.canvas.height = H;
    project.materials.videos.push(VideoMaterial {
        id: "v".into(),
        path: path.to_string_lossy().into(),
        width: W,
        height: H,
        duration: DURATION,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    let mut clip = Segment {
        id: "clip".into(),
        material_id: "v".into(),
        target_range: TimeRange::new(0, DURATION),
        source_range: TimeRange::new(0, DURATION),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    };
    if grade {
        let mut adjust = ColorAdjustMaterial::identity();
        adjust.brightness = 0.3;
        clip.extras.push(adjust.id.clone());
        project.materials.color_adjusts.push(adjust);
    }
    if blur {
        let mut effect = EffectMaterial::new(GAUSSIAN_BLUR);
        effect
            .params
            .insert("radius".into(), EffectValue::Number(40.0));
        clip.extras.push(effect.id.clone());
        project.materials.effects.push(effect);
    }
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(clip);
    project.tracks.push(track);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

/// The people matte of every frame: the subject left of the middle.
fn fake_bake(path: &std::path::Path) {
    let current = matting::current_model();
    let key = cache::key_for(path, &current).unwrap();
    for (_, dir) in cache::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let dir = cache::dir_in(&key, "CPU");
    let (mw, mh) = (160u32, 90u32);
    let alpha: Vec<u8> = (0..mw * mh)
        .map(|i| if i % mw < mw / 2 { 255 } else { 0 })
        .collect();
    for k in 0..30i64 {
        cache::write(&dir, (k * 1_000_000 + 15) / 30, mw, mh, alpha.clone()).unwrap();
    }
}

fn setting(state: &AppState) -> Option<BackgroundRemoval> {
    state
        .with_project(|p| {
            let (_, s) = p.segment("clip")?;
            p.materials.compositing_of(s)?.background.clone()
        })
        .unwrap()
}

fn render(project: &Project) -> Option<Vec<u8>> {
    let ctx = chukcut_engine::modules::gpu::render_context()?;
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(project));
    Some(
        Compositor::new(ctx)
            .render(project, 500_000, (W, H), sources.as_ref())
            .expect("render")
            .data,
    )
}

/// Mean and variance of the red channel over a block of rows 40..140 and
/// the columns given: the level and how much checkerboard is left.
fn stats(data: &[u8], columns: std::ops::Range<u32>) -> (f64, f64) {
    let values: Vec<f64> = (40..140u32)
        .flat_map(|y| columns.clone().map(move |x| (x, y)))
        .map(|(x, y)| data[((y * W + x) * 4) as usize] as f64)
        .collect();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;
    (mean, var)
}

const LEFT: std::ops::Range<u32> = 32..128;
const RIGHT: std::ops::Range<u32> = 192..288;

#[test]
fn the_targets_are_one_undoable_edit_each_and_bring_an_uncut_matte() {
    let Some(path) = checkers("edit.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path, true, true);
    fake_bake(&path);
    let response = matting::matting_set_target(
        &state,
        "clip".into(),
        matting::MattePart::Grade,
        MatteTarget::Subject,
    )
    .unwrap();
    assert!(response.job.is_none(), "every frame was baked already");
    let s = setting(&state).expect("a matte");
    assert_eq!((s.model.as_str(), s.cut), ("rvm", false), "{s:?}");
    assert_eq!(
        (s.grade.clone(), s.effects.clone()),
        (MatteTarget::Subject, MatteTarget::Whole)
    );

    matting::matting_set_target(
        &state,
        "clip".into(),
        matting::MattePart::Effects,
        MatteTarget::Background,
    )
    .unwrap();
    // Remove background on and off keeps the targets and the matte.
    matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    let s = setting(&state).unwrap();
    assert!(s.cut);
    assert_eq!(
        (s.grade.clone(), s.effects.clone()),
        (MatteTarget::Subject, MatteTarget::Background)
    );
    matting::matting_remove_background(&state, "clip".into(), false).unwrap();
    let s = setting(&state).unwrap();
    assert!(!s.cut, "off keeps the matte for the grade and the effects");

    // Both back to the whole clip: a matte that does nothing is removed.
    for part in [matting::MattePart::Grade, matting::MattePart::Effects] {
        matting::matting_set_target(&state, "clip".into(), part, MatteTarget::Whole).unwrap();
    }
    assert_eq!(setting(&state), None);

    timeline_undo(&state).unwrap();
    assert_eq!(setting(&state).unwrap().effects, MatteTarget::Background);
    let Err(error) = matting::matting_set_target(
        &state,
        "clip".into(),
        matting::MattePart::Grade,
        MatteTarget::Other("half".into()),
    ) else {
        panic!("an unknown target is refused");
    };
    assert!(error.contains("whole, subject or background"), "{error}");
}

#[test]
fn the_grade_lands_only_on_the_subject_or_only_on_the_rest() {
    let Some(path) = checkers("grade.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    fake_bake(&path);
    let plain = state_with(&path, false, false)
        .project
        .read()
        .clone()
        .unwrap();
    let Some(ungraded) = render(&plain) else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let state = state_with(&path, true, false);
    let whole = render(&state.project.read().clone().unwrap()).unwrap();
    let lifted = stats(&whole, LEFT).0 - stats(&ungraded, LEFT).0;
    assert!(lifted > 30.0, "the grade brightens: {lifted}");

    for (target, graded, kept) in [
        (MatteTarget::Subject, LEFT, RIGHT),
        (MatteTarget::Background, RIGHT, LEFT),
    ] {
        matting::matting_set_target(
            &state,
            "clip".into(),
            matting::MattePart::Grade,
            target.clone(),
        )
        .unwrap();
        let project = state.project.read().clone().unwrap();
        let frame = render(&project).unwrap();
        let (g, u) = (
            stats(&frame, graded.clone()).0,
            stats(&ungraded, graded.clone()).0,
        );
        assert!(
            (g - u - lifted).abs() < 2.0,
            "{target}: graded side {g} against {u}"
        );
        let (k, u) = (
            stats(&frame, kept.clone()).0,
            stats(&ungraded, kept.clone()).0,
        );
        assert!(
            (k - u).abs() < 1.0,
            "{target}: ungraded side {k} against {u}"
        );
        // The whole picture shows: the matte does not cut.
        assert!(frame[((90 * W + 300) * 4 + 3) as usize] == 255);

        // The export's NV12 path agrees.
        let ctx = chukcut_engine::modules::gpu::render_context().unwrap();
        let sources = MediaSourceProvider::from_project(&project);
        let Ok(export) = Compositor::new(ctx).render_nv12(&project, 500_000, (W, H), &sources)
        else {
            eprintln!("skipping the export half: no RGBA to NV12 pass on this device");
            continue;
        };
        let luma = |x: u32| {
            let row = 90 * export.y_stride;
            (x - 8..x + 8)
                .map(|x| export.y()[row + x as usize] as f64)
                .sum::<f64>()
                / 16.0
        };
        let (left, right) = (luma(80), luma(240));
        match target {
            MatteTarget::Subject => assert!(left > right + 15.0, "export: {left} {right}"),
            _ => assert!(right > left + 15.0, "export: {left} {right}"),
        }
    }
}

#[test]
fn an_effect_lands_only_on_the_rest_or_only_on_the_subject() {
    let Some(path) = checkers("effects.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    fake_bake(&path);
    let plain = state_with(&path, false, false)
        .project
        .read()
        .clone()
        .unwrap();
    let Some(sharp) = render(&plain) else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let state = state_with(&path, false, true);
    let blurred = render(&state.project.read().clone().unwrap()).unwrap();
    let (detail, smooth) = (stats(&sharp, RIGHT).1, stats(&blurred, RIGHT).1);
    assert!(
        smooth < detail / 10.0,
        "the blur removes the checkers: {smooth} of {detail}"
    );

    for (target, soft, crisp) in [
        (MatteTarget::Background, RIGHT, LEFT),
        (MatteTarget::Subject, LEFT, RIGHT),
    ] {
        matting::matting_set_target(
            &state,
            "clip".into(),
            matting::MattePart::Effects,
            target.clone(),
        )
        .unwrap();
        let frame = render(&state.project.read().clone().unwrap()).unwrap();
        let blurred_side = stats(&frame, soft.clone()).1;
        let sharp_side = stats(&frame, crisp.clone()).1;
        assert!(
            blurred_side < detail / 10.0,
            "{target}: the effect's side is blurred ({blurred_side} of {detail})"
        );
        assert!(
            (sharp_side - stats(&sharp, crisp.clone()).1).abs() < detail * 0.02,
            "{target}: the other side is untouched ({sharp_side} of {detail})"
        );
        // The blurred side was blurred over the whole picture, not a cut-out
        // of it: no dark seam where the subject would have left a hole.
        let seam = stats(&frame, 150..170).0;
        assert!(
            seam > 90.0,
            "{target}: no dark halo at the matte edge ({seam})"
        );
    }
}
