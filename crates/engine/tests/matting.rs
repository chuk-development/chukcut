//! "Remove background" end to end: the setting through the command layer and
//! the undo stack, a baked matte through the compositor in the preview and
//! the export paths, and — where the ML worker and RVM are installed — a
//! real bake through the worker.
//!
//! The compositor half does not need a model: it writes a matte of its own
//! into the cache (left half kept, right half removed) and checks that the
//! picture comes out cut exactly there.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::matting::{bake, cache, commands as matting};
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::state::AppState;

const W: u32 = 320;
const H: u32 = 180;
const DURATION: Micros = 1_000_000;

fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/matting-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// One second of plain white, 320x180, 30 fps. Its own file per test, so
/// the matte caches (keyed by the file) of two tests never meet.
fn white(name: &str) -> Option<PathBuf> {
    let path = fixture_dir().join(name);
    if path.exists() {
        return Some(path);
    }
    let tmp = path.with_extension("tmp.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c=white:size={W}x{H}:rate=30:duration=1"))
        .args([
            "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p",
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

fn state_with(path: &std::path::Path) -> Arc<AppState> {
    let mut project = Project::new("matting", CanvasConfig::default(), 30.0);
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
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(Segment {
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
    });
    project.tracks.push(track);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

/// The clip's matte setting, as the document has it.
fn setting(
    state: &AppState,
) -> Option<chukcut_engine::modules::project::compositing::BackgroundRemoval> {
    state
        .with_project(|p| {
            let (_, s) = p.segment("clip")?;
            p.materials.compositing_of(s)?.background.clone()
        })
        .unwrap()
}

/// Write a matte for every frame of the fixture: opaque left of `cut`
/// (a fraction of the width), transparent right of it. Stands in for a
/// model, so the test knows the answer.
fn fake_bake(path: &std::path::Path, cut: f32) -> PathBuf {
    let current = matting::current_model();
    let dir = cache::dir_for(path, &current.model, &current.version).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let (mw, mh) = (160u32, 90u32);
    let alpha: Vec<u8> = (0..mw * mh)
        .map(|i| {
            if ((i % mw) as f32) < cut * mw as f32 {
                255
            } else {
                0
            }
        })
        .collect();
    // The fixture's frames are at multiples of 1/30 s, which the decoder
    // reports as these microseconds.
    for k in 0..30i64 {
        let pts = (k * 1_000_000 + 15) / 30;
        cache::write(&dir, pts, mw, mh, alpha.clone()).unwrap();
    }
    dir
}

#[test]
fn the_setting_is_one_undoable_edit_and_only_on_video() {
    let Some(path) = white("setting.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    let dir = fake_bake(&path, 1.0);
    // Everything is baked already, so turning it on starts no job.
    let on = matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    assert!(on.job.is_none());
    assert_eq!(setting(&state), Some(matting::current_model()));
    let coverage = matting::matting_coverage(&state, "clip".into()).unwrap();
    assert_eq!(coverage.baked, coverage.total);
    assert_eq!(coverage.total, 30);

    timeline_undo(&state).unwrap();
    assert_eq!(setting(&state), None);

    matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    matting::matting_remove_background(&state, "clip".into(), false).unwrap();
    assert_eq!(setting(&state), None);
    // Off keeps the baked frames for an undo.
    assert_eq!(cache::list(&dir).len(), 30);

    let error = matting::matting_remove_background(&state, "nope".into(), true)
        .err()
        .unwrap();
    assert!(error.contains("no longer on the timeline"), "{error}");
}

#[test]
fn a_hole_in_the_bake_is_found_and_reported() {
    let Some(path) = white("hole.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    let dir = fake_bake(&path, 1.0);
    std::fs::remove_file(cache::frame_path(&dir, (10 * 1_000_000 + 15) / 30)).unwrap();
    let mut project = state.project.read().clone().unwrap();
    let mut material = chukcut_engine::modules::project::compositing::CompositingMaterial::new();
    material.background = Some(matting::current_model());
    project.tracks[0].segments[0]
        .extras
        .push(material.id.clone());
    project.materials.compositing.push(material);
    let job = matting::job_for(&project, &project.tracks[0].segments[0]).unwrap();
    let missing = job.missing(&cache::list(&dir));
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!((missing[0] - 333_333).abs() < 40_000, "{missing:?}");
}

/// The baked matte cuts the clip in the preview's render and in the
/// export's NV12 render: the left half white, the right half the black
/// canvas, with the edge where the matte has it.
#[test]
fn the_compositor_cuts_the_clip_with_its_baked_matte() {
    use chukcut_engine::modules::media::MediaSourceProvider;
    use chukcut_engine::modules::render::{Compositor, SourceProvider};

    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = white("cut.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    fake_bake(&path, 0.25);
    matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    let project = state.project.read().clone().unwrap();

    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let frame = compositor
        .render(&project, 500_000, (W, H), sources.as_ref())
        .expect("render");
    let edge = (0..W)
        .find(|&x| frame.pixel(x, H / 2)[0] < 128)
        .expect("a removed part");
    assert!(
        (edge as f32 / W as f32 - 0.25).abs() < 0.02,
        "edge at {edge}"
    );
    assert!(frame.pixel(10, 10)[0] > 240);
    assert!(frame.pixel(W - 10, H - 10)[0] < 10);

    // Off, the whole clip is drawn.
    let mut plain = project.clone();
    plain.tracks[0].segments[0].extras.clear();
    let frame = compositor
        .render(&plain, 500_000, (W, H), sources.as_ref())
        .expect("render");
    assert!(frame.pixel(W - 10, H / 2)[0] > 240);

    let Ok(export) = compositor.render_nv12(&project, 500_000, (W, H), sources.as_ref()) else {
        eprintln!("skipping the export half: no RGBA to NV12 pass on this device");
        return;
    };
    let luma = export.y();
    let stride = export.y_stride;
    let row = (H / 2) as usize * stride;
    let edge = (0..W as usize)
        .find(|&x| luma[row + x] < 128)
        .expect("a removed part in the export");
    assert!(
        (edge as f32 / W as f32 - 0.25).abs() < 0.02,
        "export edge at {edge}"
    );
}

/// The model itself, where it is installed: a bake through the worker
/// writes one matte per frame of the clip, and a second bake has nothing
/// left to do.
#[test]
fn rvm_bakes_a_matte_for_every_frame() {
    use chukcut_engine::modules::ml;
    use chukcut_ml_worker::registry;
    let root = ml::root();
    let ready = ml::worker::binary().is_some()
        && registry::model_present(&root, registry::model("rvm").unwrap())
        && (registry::preferred_runtime(&root).is_some()
            || std::env::var_os("CHUKCUT_ORT_DYLIB").is_some());
    if !ready {
        eprintln!("skipping: the ML worker, a runtime or RVM is not installed");
        return;
    }
    let Some(path) = white("rvm.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let current = matting::current_model();
    let dir = cache::dir_for(&path, &current.model, &current.version).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    ml::matte::Matter::prepare(&|_, _| {}, &AtomicBool::new(false)).expect("prepare");
    let job = bake::BakeJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W, H),
        range: (0, DURATION - 1),
        model: current.model.clone(),
        version: current.version.clone(),
    };
    let outcome = bake::run(&job, &AtomicBool::new(false), |_| {}).expect("bake");
    eprintln!(
        "rvm: {} frames in {:.2} s on {:?}",
        outcome.frames, outcome.seconds, outcome.provider
    );
    assert_eq!(outcome.written, 30);
    assert!(job.missing(&cache::list(&dir)).is_empty());
    let (w, h, alpha) = cache::read(&dir, cache::list(&dir)[0]).unwrap();
    assert_eq!((w, h), (W, H));
    assert_eq!(alpha.len(), (W * H) as usize);
    // A blank white frame has no person in it.
    let mean = alpha.iter().map(|&a| a as f32).sum::<f32>() / alpha.len() as f32;
    assert!(mean < 64.0, "mean alpha {mean}");

    let again = bake::run(&job, &AtomicBool::new(false), |_| {}).expect("bake");
    assert_eq!((again.written, again.frames), (0, 0));

    // A model version this build does not have is refused in words.
    let old = bake::BakeJob {
        version: "0.9-old".into(),
        ..job
    };
    let error = bake::run(&old, &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(error.contains("0.9-old"), "{error}");
}
