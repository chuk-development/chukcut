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
    let key = cache::key_for(path, &current).unwrap();
    for (_, dir) in cache::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let dir = cache::dir_in(&key, "CPU");
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
    let key = cache::key_for(&path, &current).unwrap();
    for (_, dir) in cache::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let job = bake::BakeJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W, H),
        range: (0, DURATION - 1),
        setting: current.clone(),
    };
    let outcome = bake::run(&job, &AtomicBool::new(false), |_| {}).expect("bake");
    eprintln!(
        "rvm: {} frames in {:.2} s on {:?}",
        outcome.frames, outcome.seconds, outcome.provider
    );
    assert_eq!(outcome.written, 30);
    // The mattes are in the directory of the provider that made them.
    let provider = outcome.provider.clone().expect("a provider");
    let dir = cache::dir_in(&key, &provider);
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
        setting: chukcut_engine::modules::project::compositing::BackgroundRemoval {
            version: "0.9-old".into(),
            ..current
        },
        ..job
    };
    let error = bake::run(&old, &AtomicBool::new(false), |_| {}).unwrap_err();
    assert!(error.contains("0.9-old"), "{error}");
}

/// Every frame in the CPU's directory and a third of them in CUDA's: the
/// clip counts as baked and is drawn from the directory with the most
/// frames, so it never mixes the two providers' mattes.
#[test]
fn the_provider_with_the_most_frames_is_the_one_drawn() {
    let Some(path) = white("providers.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    let full = fake_bake(&path, 1.0);
    let key = cache::key_for(&path, &matting::current_model()).unwrap();
    let partial = cache::dir_in(&key, "CUDA");
    for k in 0..10i64 {
        cache::write(&partial, (k * 1_000_000 + 15) / 30, 4, 4, vec![0; 16]).unwrap();
    }
    let (best, times) = cache::best(&key).expect("a directory");
    assert_eq!(best, full);
    assert_eq!(times.len(), 30);
    let providers: Vec<String> = cache::dirs_of(&key).into_iter().map(|(p, _)| p).collect();
    assert_eq!(providers, ["cpu", "cuda"]);
    // Turning it on finds the clip baked: no job.
    let on = matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    assert!(on.job.is_none());
    let coverage = matting::matting_coverage(&state, "clip".into()).unwrap();
    assert_eq!((coverage.baked, coverage.total), (30, 30));
    let _ = std::fs::remove_dir_all(&partial);
}

/// A clip whose matte lacks frames (trimmed longer, a cleaned cache) gets
/// a background bake from `matting_queue_missing`; a complete one does not.
#[test]
fn missing_frames_are_queued_and_complete_clips_are_left_alone() {
    let Some(path) = white("queue.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    let dir = fake_bake(&path, 1.0);
    matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    assert!(matting::matting_queue_missing(&state).unwrap().is_empty());
    for k in 20..30i64 {
        std::fs::remove_file(cache::frame_path(&dir, (k * 1_000_000 + 15) / 30)).unwrap();
    }
    let jobs = matting::matting_queue_missing(&state).unwrap();
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    // Asking again joins a running bake instead of starting another.
    let again = matting::matting_queue_missing(&state).unwrap();
    let running = matting::matting_status(jobs[0]).is_some_and(|s| s.finished.is_none());
    if running {
        assert_eq!(again, jobs);
    } else {
        // Failed already (no worker here): a failed bake is not retried
        // on its own after every edit.
        assert!(again.len() <= 1, "{again:?}");
    }
    for job in jobs.into_iter().chain(again) {
        matting::matting_cancel(job);
    }
}

/// "Select object" turns clicks on the canvas into points of the source
/// frame, records them with the frame's source time as one undoable edit,
/// and refuses clicks it cannot use, in words.
#[test]
fn select_object_records_source_points_and_refuses_bad_clicks() {
    let Some(path) = white("select.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    let point = |x, y, keep| matting::CanvasPoint { x, y, keep };

    let none_kept = matting::matting_select_object(
        &state,
        "clip".into(),
        500_000,
        vec![point(0.5, 0.5, false)],
        None,
    )
    .err()
    .unwrap();
    assert!(none_kept.contains("on the object"), "{none_kept}");
    let off_clip = matting::matting_select_object(
        &state,
        "clip".into(),
        5_000_000,
        vec![point(0.5, 0.5, true)],
        None,
    )
    .err()
    .unwrap();
    assert!(off_clip.contains("playhead"), "{off_clip}");
    assert_eq!(setting(&state), None);

    let response = matting::matting_select_object(
        &state,
        "clip".into(),
        500_000,
        vec![point(0.25, 0.75, true), point(0.6, 0.1, false)],
        Some(true),
    )
    .unwrap();
    if let Some(job) = response.job {
        // The model is not what this test is about.
        matting::matting_cancel(job);
    }
    let recorded = setting(&state).expect("the setting");
    assert_eq!(recorded.model, "mobilesam");
    assert!(recorded.invert);
    let prompt = recorded.prompt.expect("the clicks");
    assert_eq!(prompt.time, 500_000);
    // The clip fills the canvas, so canvas and source fractions agree.
    assert!((prompt.points[0].x - 0.25).abs() < 1e-3, "{prompt:?}");
    assert!((prompt.points[0].y - 0.75).abs() < 1e-3, "{prompt:?}");
    assert!(prompt.points[0].keep && !prompt.points[1].keep);

    // Inverting is its own edit; the selection stays.
    matting::matting_set_invert(&state, "clip".into(), false).unwrap();
    let flipped = setting(&state).unwrap();
    assert!(!flipped.invert && flipped.prompt.is_some());
    timeline_undo(&state).unwrap();
    assert!(setting(&state).unwrap().invert);
    timeline_undo(&state).unwrap();
    assert_eq!(setting(&state), None);
}

/// "Cut out instead": the same baked matte, the other side removed.
#[test]
fn an_inverted_matte_removes_what_it_kept() {
    use chukcut_engine::modules::media::MediaSourceProvider;
    use chukcut_engine::modules::render::{Compositor, SourceProvider};

    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = white("invert.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path);
    fake_bake(&path, 0.25);
    matting::matting_remove_background(&state, "clip".into(), true).unwrap();
    matting::matting_set_invert(&state, "clip".into(), true).unwrap();
    let project = state.project.read().clone().unwrap();
    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let frame = compositor
        .render(&project, 500_000, (W, H), sources.as_ref())
        .expect("render");
    assert!(frame.pixel(10, H / 2)[0] < 10, "the kept side is cut out");
    assert!(frame.pixel(W - 10, H / 2)[0] > 240, "the rest is drawn");
    let edge = (0..W)
        .find(|&x| frame.pixel(x, H / 2)[0] > 128)
        .expect("a drawn part");
    assert!(
        (edge as f32 / W as f32 - 0.25).abs() < 0.02,
        "edge at {edge}"
    );
}

/// A box moving across a grey frame, for the models that find objects:
/// 1 s at 30 fps, 320x180, a red 60x60 square from x = 40 to x = 160.
fn moving_box(name: &str) -> Option<PathBuf> {
    let path = fixture_dir().join(name);
    if path.exists() {
        return Some(path);
    }
    let tmp = path.with_extension("tmp.mp4");
    // `overlay`, not `drawbox`: drawbox's `t` is its line thickness, not
    // the time, so a box "moving" with t sat outside the frame.
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c=0x707070:size={W}x{H}:rate=30:duration=1"))
        .args([
            "-f",
            "lavfi",
            "-i",
            "color=c=0xd02020:size=60x60:rate=30:duration=1",
        ])
        .args([
            "-filter_complex",
            "[0][1]overlay=x='40+t*120':y=60",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-pix_fmt",
            "yuv420p",
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

/// Intersection over union of a matte (>= 128) with the box x..x+60,
/// 60..120 at frame size W x H.
fn box_iou(alpha: &[u8], x: f32) -> f32 {
    let (mut inter, mut union) = (0u32, 0u32);
    for py in 0..H {
        for px in 0..W {
            let inside_box = (px as f32) >= x && (px as f32) < x + 60.0 && (60..120).contains(&py);
            let inside_mask = alpha[(py * W + px) as usize] >= 128;
            inter += (inside_box && inside_mask) as u32;
            union += (inside_box || inside_mask) as u32;
        }
    }
    inter as f32 / union.max(1) as f32
}

fn installed(model: &str) -> bool {
    use chukcut_engine::modules::ml;
    use chukcut_ml_worker::registry;
    let root = ml::root();
    ml::worker::binary().is_some()
        && registry::model(model).is_some_and(|m| {
            registry::model_present(&root, m)
                && m.companion
                    .and_then(registry::model)
                    .is_none_or(|c| registry::model_present(&root, c))
        })
        && (registry::preferred_runtime(&root).is_some()
            || std::env::var_os("CHUKCUT_ORT_DYLIB").is_some())
}

/// MobileSAM, where it is installed: one click on the box in the middle
/// frame selects it, and the selection follows it to both ends of the clip
/// (VitTrack carries the prompt; SAM draws the mask).
#[test]
fn a_click_selects_the_box_and_the_selection_follows_it() {
    if !installed("mobilesam") {
        eprintln!("skipping: the ML worker, a runtime or MobileSAM is not installed");
        return;
    }
    let Some(path) = moving_box("select-moving-box.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let setting = chukcut_engine::modules::project::compositing::BackgroundRemoval {
        model: "mobilesam".into(),
        version: chukcut_engine::modules::ml::segment::model_version().into(),
        // The box is at x = 100..160 at 0.5 s: click its centre.
        prompt: Some(
            chukcut_engine::modules::project::compositing::ObjectPrompt {
                time: 500_000,
                points: vec![chukcut_engine::modules::project::compositing::PromptPoint {
                    x: 130.0 / W as f32,
                    y: 90.0 / H as f32,
                    keep: true,
                }],
            },
        ),
        invert: false,
    };
    let key = cache::key_for(&path, &setting).unwrap();
    for (_, dir) in cache::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let job = bake::BakeJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W, H),
        range: (0, DURATION - 1),
        setting,
    };
    let outcome = bake::run(&job, &AtomicBool::new(false), |_| {}).expect("bake");
    eprintln!(
        "mobilesam: {} frames in {:.2} s on {:?}",
        outcome.written, outcome.seconds, outcome.provider
    );
    assert_eq!(outcome.written, 30);
    let (dir, times) = cache::best(&key).expect("baked");
    assert!(job.missing(&times).is_empty());
    for (index, &pts) in times.iter().enumerate() {
        let (w, h, alpha) = cache::read(&dir, pts).unwrap();
        assert_eq!((w, h), (W, H));
        // ffmpeg's overlay evaluates x at the frame's time.
        let x = 40.0 + index as f32 / 30.0 * 120.0;
        let iou = box_iou(&alpha, x.floor());
        assert!(iou > 0.8, "frame {index}: IoU {iou:.2} with the box at {x}");
    }
}

/// BiRefNet, where it is installed: the box is the object. On a machine
/// where models run on the CPU it must refuse in words, not run for minutes.
#[test]
fn birefnet_cuts_out_the_object_or_refuses_on_the_cpu() {
    if !installed("birefnet-lite") {
        eprintln!("skipping: the ML worker, a runtime or BiRefNet is not installed");
        return;
    }
    let Some(path) = moving_box("objects-moving-box.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let setting = matting::setting_for(matting::BackgroundMode::Objects);
    let key = cache::key_for(&path, &setting).unwrap();
    for (_, dir) in cache::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let job = bake::BakeJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W, H),
        // Ten frames are enough to know it works.
        range: (0, 333_333),
        setting,
    };
    match bake::run(&job, &AtomicBool::new(false), |_| {}) {
        Ok(outcome) => {
            eprintln!(
                "birefnet: {} frames in {:.2} s on {:?}",
                outcome.written, outcome.seconds, outcome.provider
            );
            assert!(outcome.written >= 10);
            let (dir, times) = cache::best(&key).expect("baked");
            let (_, _, alpha) = cache::read(&dir, times[0]).unwrap();
            let iou = box_iou(&alpha, 40.0);
            assert!(iou > 0.8, "IoU {iou:.2}");
        }
        Err(error) => {
            assert!(error.contains("needs a GPU"), "{error}");
            eprintln!("skipping the matte check: {error}");
        }
    }
}
