//! "Remove object" and "Enhance quality" end to end: the compositor drawing
//! a made frame in place of the decoded one (and the decoded one until it is
//! made) in the preview and the export, and — where the ML worker and the
//! models are installed — real bakes: a moving square removed from a still
//! shot, a static box removed, a clip enhanced 2x.
//!
//! The compositor half needs no model: it writes frames of its own into the
//! cache (solid magenta) and checks that the picture is them.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::enhance::commands as cmd;
use chukcut_engine::modules::enhance::{
    self, bake::EnhanceJob, cache, jobs, Chain, ObjectRemoval, Upscale,
};
use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::matting::commands::CanvasPoint;
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig};
use chukcut_engine::modules::timeline::History;
use chukcut_engine::state::AppState;

const W: u32 = 320;
const H: u32 = 180;
/// The background, a mid blue-grey: what a removed square should become.
const BG: [u8; 3] = [0x40, 0x50, 0x60];
const SQUARE: u32 = 36;
/// Pixels the square moves per frame.
const STEP: u32 = 6;

fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/enhance-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// One second at 30 fps: a white square moving right by [`STEP`] pixels a
/// frame over a flat [`BG`] with a few dark stripes (so the picture has
/// something to continue). Its own file per test, so the caches (keyed by
/// the file) of two tests never meet.
fn square(name: &str) -> Option<PathBuf> {
    let path = fixture_dir().join(name);
    if path.exists() {
        return Some(path);
    }
    let tmp = path.with_extension("tmp.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c=0x405060:size={W}x{H}:rate=30:duration=1"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!(
            "color=c=white:size={SQUARE}x{SQUARE}:rate=30:duration=1"
        ))
        .args(["-filter_complex"])
        .arg(format!(
            "[0]drawbox=x=0:y=20:w={W}:h=6:c=0x101820:t=fill[bg];\
             [bg][1]overlay=x='60+n*{STEP}':y=80"
        ))
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

fn project_with(path: &Path) -> Project {
    let mut project = Project::new(
        "enhance",
        CanvasConfig {
            width: W,
            height: H,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project.materials.videos.push(VideoMaterial {
        id: "v".into(),
        path: path.to_string_lossy().into(),
        width: W,
        height: H,
        duration: 1_000_000,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(Segment {
        id: "clip".into(),
        material_id: "v".into(),
        target_range: TimeRange::new(0, 1_000_000),
        source_range: TimeRange::new(0, 1_000_000),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    });
    project.tracks.push(track);
    project
}

/// `project` with "Remove object" over `removal` and, with `scale`,
/// "Enhance quality", set through the document commands (no bake starts).
fn with_settings(
    mut project: Project,
    removal: Option<ObjectRemoval>,
    scale: Option<u32>,
) -> Project {
    let mut history = History::new();
    if let Some(removal) = removal {
        let (entry, command) =
            enhance::set_removal_command(&project, "clip", Some(&removal)).unwrap();
        let (id, value) = entry.unwrap();
        project.materials.extras.insert(id, value);
        history.apply(&mut project, command).unwrap();
    }
    if let Some(scale) = scale {
        let up = Upscale::new(scale).unwrap();
        let (entry, command) = enhance::set_upscale_command(&project, "clip", Some(&up)).unwrap();
        let (id, value) = entry.unwrap();
        project.materials.extras.insert(id, value);
        history.apply(&mut project, command).unwrap();
    }
    project
}

fn state_of(project: Project) -> Arc<AppState> {
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

/// Timeline frame `n` of the 30 fps grid, as every renderer samples it.
fn at(n: i64) -> Micros {
    (n as f64 * 1_000_000.0 / 30.0).round() as Micros + 10
}

fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * width + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2]]
}

fn job_of(project: &Project) -> EnhanceJob {
    jobs::job_for(project, &project.tracks[0].segments[0]).unwrap()
}

/// Remove every directory of `job`'s frames, then fill one with solid
/// magenta frames of the chain's output size for `times`.
fn fake_bake(job: &EnhanceJob, times: &[Micros]) -> PathBuf {
    let key = job.key().unwrap();
    for dir in cache::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let (_, (ow, oh)) = job.sizes();
    let dir = cache::dir_in(&key, "CPU", ow.max(oh));
    let magenta: Vec<u8> = (0..ow * oh).flat_map(|_| [255u8, 0, 255, 255]).collect();
    for &t in times {
        cache::write(&dir, t, ow, oh, &magenta).unwrap();
    }
    dir
}

fn boxed() -> ObjectRemoval {
    ObjectRemoval {
        boxes: vec![[0.1, 0.1, 0.2, 0.2]],
        ..ObjectRemoval::new()
    }
}

#[test]
fn a_clip_says_how_much_is_made_and_what_the_rest_would_take() {
    let Some(path) = square("coverage.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let project = with_settings(project_with(&path), Some(boxed()), None);
    let job = job_of(&project);
    let grid = job.frame_times(job.range.0, job.range.1);
    assert_eq!(grid.len(), 30);
    fake_bake(&job, &grid[..12]);
    let state = state_of(project);
    let cover = cmd::enhance_coverage(&state, "clip".into()).unwrap();
    assert_eq!((cover.baked, cover.total), (12, 30));
    let estimate = cmd::enhance_estimate(&state, "clip".into()).unwrap();
    assert_eq!((estimate.frames, estimate.missing), (30, 18));
    assert!(
        estimate.seconds_cpu > 5.0 * estimate.seconds_gpu,
        "{estimate:?}"
    );
    // A chain changed (a second box) is another set of frames.
    let other = with_settings(
        project_with(&path),
        Some(ObjectRemoval {
            boxes: vec![[0.1, 0.1, 0.2, 0.2], [0.5, 0.5, 0.1, 0.1]],
            ..ObjectRemoval::new()
        }),
        None,
    );
    assert_ne!(job_of(&other).key().unwrap(), job.key().unwrap());
    assert_eq!(jobs::coverage(&job_of(&other)).unwrap().baked, 0);
}

/// Until a frame is made the clip is drawn as decoded; once it is, the made
/// picture is drawn — over the whole clip, at its place, whatever its size
/// — in the preview's render and in an export, which needs no worker when
/// nothing is missing.
#[test]
fn a_made_frame_replaces_the_decoded_one_in_the_preview_and_the_export() {
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = square("compositor.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    // Enhanced 2x as well: the made frames are 640×360 and must still fill
    // exactly the 320×180 canvas.
    let project = with_settings(project_with(&path), Some(boxed()), Some(2));
    let plain = project_with(&path);
    let job = job_of(&project);
    assert_eq!(job.sizes().1, (640, 360));
    let render = |project: &Project, n: i64| {
        Compositor::new(Arc::clone(&ctx))
            .render(
                project,
                at(n),
                (W, H),
                &MediaSourceProvider::from_project(project),
            )
            .expect("render")
            .data
    };
    fake_bake(&job, &[]);
    assert_eq!(
        render(&project, 5),
        render(&plain, 5),
        "nothing made yet: the decoded frame"
    );
    let grid = job.frame_times(job.range.0, job.range.1);
    let mut made: Vec<Micros> = grid.clone();
    made.remove(10);
    fake_bake(&job, &made);
    let drawn = render(&project, 5);
    for (x, y) in [(0, 0), (W - 1, H - 1), (160, 90), (W - 1, 0)] {
        let p = pixel(&drawn, W, x, y);
        assert!(
            p[0] >= 250 && p[1] <= 4 && p[2] >= 250,
            "the made frame at {x},{y}: {p:?}"
        );
    }
    // Frame 10 is not made: the decoded picture.
    let decoded = render(&project, 10);
    assert_eq!(decoded, render(&plain, 10));

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("enhance");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("made.mp4");
    let _ = std::fs::remove_file(&out);
    fake_bake(&job, &grid);
    let request = ExportRequest {
        output_path: out.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: Some((0, 400_000)),
    };
    let settings = resolve_settings(&project, &request).expect("settings");
    let export = ExportJob {
        job_id: "enhance".into(),
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
    run_export(&export, &FnSink(|_| {})).expect("export");
    let mut decoder = VideoDecoder::open(&out).expect("open the export");
    for n in [0, 6, 11] {
        let frame = decoder.seek_and_decode(at(n) + 5_000).expect("decode");
        let p = &frame.data[((90 * frame.width + 160) * 4) as usize..][..3];
        assert!(p[0] > 200 && p[1] < 60 && p[2] > 200, "exported {n}: {p:?}");
    }
    let _ = std::fs::remove_file(&out);
}

fn installed(models: &[&str]) -> bool {
    use chukcut_ml_worker::registry;
    let root = chukcut_engine::modules::ml::root();
    chukcut_engine::modules::ml::worker::binary().is_some()
        && registry::preferred_runtime(&root).is_some()
        && models
            .iter()
            .all(|m| registry::model(m).is_some_and(|spec| registry::model_present(&root, spec)))
}

fn clear(job: &EnhanceJob) {
    for dir in cache::dirs_of(&job.key().unwrap()) {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Mean absolute difference from [`BG`] over the square's place in frame
/// `n` (where it is in the source), on a straight-RGBA render.
fn residue(data: &[u8], n: u32) -> f32 {
    let x0 = 60 + n * STEP;
    let (mut sum, mut count) = (0u32, 0u32);
    for y in 80..80 + SQUARE {
        for x in x0..(x0 + SQUARE).min(W) {
            let p = pixel(data, W, x, y);
            for c in 0..3 {
                sum += (p[c] as i32 - BG[c] as i32).unsigned_abs();
            }
            count += 3;
        }
    }
    sum as f32 / count as f32
}

/// A real removal through the worker, where it, LaMa and MobileSAM are
/// installed: one click on the square and it is gone from every frame,
/// the place it was looks like the background, and the rest of the picture
/// is untouched. Skipped (not downloaded) on a machine without them.
#[test]
fn a_clicked_square_is_removed_from_every_frame() {
    if !installed(&["lama", "mobilesam", "mobilesam-decoder"]) {
        eprintln!("skipping: the ML worker, ONNX Runtime, LaMa or MobileSAM is not installed");
        return;
    }
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = square("removal.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let plain = project_with(&path);
    let state = state_of(plain.clone());
    // Frame 15: the square at x 150..186, y 80..116; click its middle.
    let point = CanvasPoint {
        x: (150.0 + SQUARE as f32 / 2.0) / W as f32,
        y: (80.0 + SQUARE as f32 / 2.0) / H as f32,
        keep: true,
    };
    let prompt = cmd::enhance_prompt_at(&state, "clip".into(), at(15), vec![point]).unwrap();
    let removal = ObjectRemoval {
        prompt: Some(prompt),
        ..ObjectRemoval::new()
    };
    clear(&job_of(&with_settings(
        plain.clone(),
        Some(removal.clone()),
        None,
    )));
    let response = cmd::enhance_set_removal(&state, "clip".into(), Some(removal)).unwrap();
    assert!(response.edit.is_some());
    let job = response.job.expect("a bake");
    let done = cmd::enhance_wait(job, |_| {}).expect("bake");
    assert_eq!(done.written, 30, "{done:?}");
    eprintln!(
        "removed a square from 30 frames of {W}x{H} in {:.2} s on {:?} ({:?} ms a frame in the model)",
        done.seconds, done.provider, done.model_millis_per_frame
    );
    let removed = state.project.read().clone().unwrap();
    let render = |project: &Project, n: i64| {
        Compositor::new(Arc::clone(&ctx))
            .render(
                project,
                at(n),
                (W, H),
                &MediaSourceProvider::from_project(project),
            )
            .expect("render")
            .data
    };
    for n in [2u32, 15, 27] {
        let before = render(&plain, n as i64);
        let after = render(&removed, n as i64);
        assert!(residue(&before, n) > 80.0, "the square is in the source");
        let left = residue(&after, n);
        assert!(
            left < 12.0,
            "frame {n}: {left} code values from the background"
        );
        // The stripe and the far corner are as they were.
        for (x, y) in [(10, 23), (300, 170), (5, 150)] {
            let (a, b) = (pixel(&before, W, x, y), pixel(&after, W, x, y));
            assert!(
                a.iter()
                    .zip(b)
                    .all(|(a, b)| (*a as i32 - b as i32).abs() <= 3),
                "frame {n} at {x},{y}: {a:?} → {b:?}"
            );
        }
    }
}

/// A real 2x through the worker, where it and Real-ESRGAN are installed:
/// every frame made at 640×360, and the made picture is the same picture.
#[test]
fn a_clip_enhanced_2x_has_every_frame_at_twice_the_size() {
    if !installed(&["realesr-general-x4v3"]) {
        eprintln!("skipping: the ML worker, ONNX Runtime or Real-ESRGAN is not installed");
        return;
    }
    let Some(path) = square("upscale.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let mut plain = project_with(&path);
    // A third of a second is enough.
    plain.tracks[0].segments[0].source_range = TimeRange::new(0, 333_333);
    plain.tracks[0].segments[0].target_range = TimeRange::new(0, 333_333);
    let enhanced = with_settings(plain.clone(), None, Some(2));
    let job = job_of(&enhanced);
    clear(&job);
    let state = state_of(plain);
    let response = cmd::enhance_set_upscale(&state, "clip".into(), Some(2)).unwrap();
    let done = cmd::enhance_wait(response.job.expect("a bake"), |_| {}).expect("bake");
    assert_eq!(done.written, 10, "{done:?}");
    eprintln!(
        "enhanced 10 frames of {W}x{H} 2x in {:.2} s on {:?} ({:?} ms a frame in the model)",
        done.seconds, done.provider, done.model_millis_per_frame
    );
    let (dir, times) = job.best().unwrap().expect("made frames");
    assert_eq!(times.len(), 10);
    let (w, h, rgba) = cache::read(&dir, times[5], (0, 0)).unwrap();
    assert_eq!((w, h), (640, 360));
    // Frame 5's square is at x 90..126: its middle is white, the
    // background is the background, at twice the coordinates.
    let p = pixel(&rgba, w, 2 * 108, 2 * 98);
    assert!(p.iter().all(|&v| v > 230), "the square: {p:?}");
    let q = pixel(&rgba, w, 2 * 250, 2 * 150);
    assert!(
        q.iter()
            .zip(BG)
            .all(|(a, b)| (*a as i32 - b as i32).abs() <= 6),
        "the background: {q:?}"
    );
    // Off again: the clip has nothing to remake.
    cmd::enhance_set_upscale(&state, "clip".into(), None).unwrap();
    let project = state.project.read().clone().unwrap();
    assert!(Chain::of(&project.materials, &project.tracks[0].segments[0]).is_none());
}

/// `project` slowed to 0.25x (its first quarter second over the clip's
/// second) with optical flow on.
fn slowed_with_flow(mut project: Project) -> Project {
    use chukcut_engine::modules::speed::blend::{set_frame_blend_command, FrameBlend};
    let clip = &mut project.tracks[0].segments[0];
    clip.source_range = TimeRange::new(0, 250_000);
    clip.speed = 0.25;
    let (entry, command) = set_frame_blend_command(&project, "clip", FrameBlend::Flow).unwrap();
    let (key, value) = entry.unwrap();
    project.materials.extras.insert(key, value);
    History::new().apply(&mut project, command).unwrap();
    project
}

/// Optical flow on a remade clip draws in-between frames made from the
/// remade frames: their own directory (keyed by the chain), never the
/// in-between frames of the decoded clip, which still show the object.
#[test]
fn a_remade_clip_flows_between_its_remade_frames() {
    use chukcut_engine::modules::speed::flow::{self, jobs as flow_jobs, FlowSample};
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = square("flow-chain.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let project = slowed_with_flow(with_settings(project_with(&path), Some(boxed()), None));
    let segment = &project.tracks[0].segments[0];
    let chain = Chain::of(&project.materials, segment).unwrap();
    let flow_job = flow_jobs::job_for(&project, segment).unwrap();
    let remade = flow_job
        .remade
        .as_ref()
        .expect("the remade frames go along");
    assert_eq!(remade.key().unwrap(), job_of(&project).key().unwrap());
    assert!(
        remade.range.1 > job_of(&project).range.1,
        "one frame past the clip"
    );
    let plain_key = flow::key_for(&path, None).unwrap();
    let chained_key = flow::key_for(&path, Some(&chain)).unwrap();
    assert_eq!(flow_job.key().unwrap(), chained_key);
    assert_ne!(plain_key, chained_key);
    // No dash after the plain key: its directories never list as the plain
    // clip's.
    assert!(chained_key.starts_with(&plain_key) && !chained_key[plain_key.len()..].contains('-'));

    // Every remade frame made (magenta); the in-between frame of the
    // decoded clip green, the remade clip's red.
    let grid = remade.frame_times(remade.range.0, remade.range.1);
    fake_bake(remade, &grid);
    let solid = |rgb: [u8; 3]| -> Vec<u8> {
        (0..W * H)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect()
    };
    for key in [&plain_key, &chained_key] {
        for dir in flow::dirs_of(key) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    // Timeline frame 2 at 0.25x is source frame 0 and a half.
    let sample = FlowSample { index: 0, step: 32 };
    flow::write(
        &flow::dir_in(&plain_key, "CPU", W),
        sample,
        W,
        H,
        &solid([0, 255, 0]),
    )
    .unwrap();
    let render = |project: &Project| {
        Compositor::new(Arc::clone(&ctx))
            .render(
                project,
                at(2),
                (W, H),
                &MediaSourceProvider::from_project(project),
            )
            .expect("render")
            .data
    };
    let p = pixel(&render(&project), W, 160, 90);
    assert!(
        p[0] > 240 && p[1] < 16 && p[2] > 240,
        "not baked yet: the remade frames blended, never the decoded clip's flow: {p:?}"
    );
    flow::write(
        &flow::dir_in(&chained_key, "CPU", W),
        sample,
        W,
        H,
        &solid([255, 0, 0]),
    )
    .unwrap();
    let p = pixel(&render(&project), W, 160, 90);
    assert!(
        p[0] > 240 && p[1] < 16 && p[2] < 16,
        "the remade clip's own in-between frame: {p:?}"
    );
    assert_eq!(
        flow_jobs::coverage(&flow_job).unwrap().baked,
        1,
        "counted in its own directory"
    );
}

/// A real bake of a remade clip's in-between frames, where the worker and
/// RIFE are installed: they are made from the remade frames (here grey
/// levels that step up frame by frame, with no square), so the frame half
/// way shows the grey half way between and no trace of the decoded clip's
/// white square.
#[test]
fn a_remade_clip_bakes_its_in_between_frames_from_the_remade_ones() {
    use chukcut_engine::modules::speed::flow::{self, bake as flow_bake, jobs as flow_jobs};
    if !installed(&["rife"]) {
        eprintln!("skipping: the ML worker or RIFE is not installed");
        return;
    }
    let Some(path) = square("flow-chain-bake.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let project = slowed_with_flow(with_settings(project_with(&path), Some(boxed()), None));
    let segment = &project.tracks[0].segments[0];
    let flow_job = flow_jobs::job_for(&project, segment).unwrap();
    let remade = flow_job.remade.clone().unwrap();
    clear(&remade);
    let key = remade.key().unwrap();
    let dir = cache::dir_in(&key, "CPU", W);
    let grid = remade.frame_times(remade.range.0, remade.range.1);
    for (k, &t) in grid.iter().enumerate() {
        let v = 40 + 20 * k as u8;
        let grey: Vec<u8> = (0..W * H).flat_map(|_| [v, v, v, 255]).collect();
        cache::write(&dir, t, W, H, &grey).unwrap();
    }
    for dir in flow::dirs_of(&flow_job.key().unwrap()) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let cancel = AtomicBool::new(false);
    let outcome = flow_bake::run(&flow_job, &cancel, |_| {}).expect("bake");
    assert_eq!(outcome.written as usize, flow_job.samples.len());
    let (dir, _) = flow::best(&flow_job.key().unwrap()).expect("baked");
    let sample = flow::FlowSample { index: 1, step: 32 };
    let (w, h, rgba) = flow::read(&dir, sample).unwrap();
    assert_eq!((w, h), (W, H));
    // Between grey 60 and grey 80: about 70 everywhere, the square's place
    // included.
    for (x, y) in [(20, 20), (160, 90), (100, 98), (300, 170)] {
        let v = rgba[((y * W + x) * 4) as usize] as i32;
        assert!((v - 70).abs() <= 6, "at {x},{y}: {v}");
    }
}
