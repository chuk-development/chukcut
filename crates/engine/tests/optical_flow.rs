//! "Optical flow (AI)" end to end: the in-between frames a slowed clip
//! needs, the compositor drawing a baked frame (and the plain blend until it
//! is there) in the preview and the export, and — where the ML worker and
//! RIFE are installed — a real bake that puts a moving square where it is
//! half-way between two frames.
//!
//! The compositor half needs no model: it writes frames of its own into the
//! cache (solid red) and checks that the picture is them.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig};
use chukcut_engine::modules::speed::blend::FrameBlend;
use chukcut_engine::modules::speed::commands as speed;
use chukcut_engine::modules::speed::flow::{self, jobs, FlowSample};
use chukcut_engine::state::AppState;

const W: u32 = 320;
const H: u32 = 180;
/// Pixels the square moves per source frame.
const STEP: u32 = 24;
const SQUARE: u32 = 40;

fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/flow-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// One second at 30 fps: a white square moving right by [`STEP`] pixels a
/// frame over dark grey. Its own file
/// per test, so the caches (keyed by the file) of two tests never meet.
fn square(name: &str) -> Option<PathBuf> {
    let path = fixture_dir().join(name);
    if path.exists() {
        return Some(path);
    }
    let tmp = path.with_extension("tmp.mp4");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("color=c=0x303030:size={W}x{H}:rate=30:duration=1"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!(
            "color=c=white:size={SQUARE}x{SQUARE}:rate=30:duration=1"
        ))
        .args(["-filter_complex"])
        .arg(format!("[0][1]overlay=x='20+n*{STEP}':y=70"))
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

/// The first quarter second of `path` at 0.25x: one second on the timeline,
/// with optical flow on.
fn state_with(path: &std::path::Path, mode: FrameBlend) -> Arc<AppState> {
    let mut project = Project::new(
        "flow",
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
        source_range: TimeRange::new(0, 250_000),
        render_index: 0,
        speed: 0.25,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    });
    project.tracks.push(track);
    let state = AppState::new();
    *state.project.write() = Some(project);
    if mode != FrameBlend::None {
        speed::speed_set_frame_blend(&state, "clip".into(), mode).expect("frame blend");
    }
    state
}

fn project_of(state: &AppState) -> Project {
    state.project.read().clone().unwrap()
}

/// Timeline frame `n` of the 30 fps grid, as every renderer samples it.
fn at(n: i64) -> Micros {
    (n as f64 * 1_000_000.0 / 30.0).round() as Micros + 10
}

/// Remove every directory of `path`'s frames, then fill one with solid red
/// frames for `samples`.
fn fake_bake(path: &std::path::Path, samples: impl IntoIterator<Item = FlowSample>) -> PathBuf {
    let key = flow::key_for(path, None).unwrap();
    for dir in flow::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let dir = flow::dir_in(&key, "CPU", W);
    let red: Vec<u8> = (0..W * H).flat_map(|_| [255u8, 0, 0, 255]).collect();
    for sample in samples {
        flow::write(&dir, sample, W, H, &red).unwrap();
    }
    dir
}

fn pixel(data: &[u8], x: u32, y: u32) -> [u8; 3] {
    let i = ((y * W + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2]]
}

#[test]
fn a_slowed_clip_needs_three_frames_between_each_pair_and_says_so() {
    let Some(path) = square("coverage.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = state_with(&path, FrameBlend::Flow);
    let project = project_of(&state);
    let job = jobs::job_for(&project, &project.tracks[0].segments[0]).unwrap();
    // 30 timeline frames over source frames 0..7.5: three new frames after
    // each source frame shown.
    assert_eq!(job.samples.len(), 22, "{:?}", job.samples);
    assert!(job.samples.iter().all(|s| [16, 32, 48].contains(&s.step)));
    fake_bake(&path, job.samples.iter().copied().take(10));
    let coverage = speed::speed_flow_coverage(&state, "clip".into()).unwrap();
    assert_eq!((coverage.baked, coverage.total), (10, 22));
    // Off, the clip needs nothing and the coverage is refused in words.
    speed::speed_set_frame_blend(&state, "clip".into(), FrameBlend::None).unwrap();
    let error = speed::speed_flow_coverage(&state, "clip".into()).unwrap_err();
    assert!(error.contains("optical flow"), "{error}");
}

/// Until a frame is baked the clip is drawn exactly as a frame blend; once
/// it is, the baked picture is drawn, in the preview's render and in an
/// export, which needs no worker when nothing is missing.
#[test]
fn a_baked_frame_replaces_the_blend_in_the_preview_and_the_export() {
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = square("compositor.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let flowed = project_of(&state_with(&path, FrameBlend::Flow));
    let blended = project_of(&state_with(&path, FrameBlend::Blend));
    fake_bake(&path, []);
    // Timeline frame 6 is source frame 1.5: half-way from frame 1 to 2.
    let render = |project: &Project| {
        Compositor::new(Arc::clone(&ctx))
            .render(
                project,
                at(6),
                (W, H),
                &MediaSourceProvider::from_project(project),
            )
            .expect("render")
            .data
    };
    assert_eq!(
        render(&flowed),
        render(&blended),
        "nothing baked yet: the plain blend"
    );

    let job = jobs::job_for(&flowed, &flowed.tracks[0].segments[0]).unwrap();
    assert!(job.samples.contains(&FlowSample { index: 1, step: 32 }));
    fake_bake(&path, job.samples.iter().copied());
    let drawn = render(&flowed);
    let red = pixel(&drawn, 160, 20);
    assert!(
        red[0] >= 250 && red[1] <= 4 && red[2] <= 4,
        "the baked frame: {red:?}"
    );
    // A source frame itself is never replaced.
    let on_frame = Compositor::new(Arc::clone(&ctx))
        .render(
            &flowed,
            at(4),
            (W, H),
            &MediaSourceProvider::from_project(&flowed),
        )
        .unwrap()
        .data;
    let grey = pixel(&on_frame, 300, 20);
    assert!(grey[0] < 80 && grey[1] < 80, "source frame 1: {grey:?}");

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("optical-flow");
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("flow.mp4");
    let _ = std::fs::remove_file(&out);
    let request = ExportRequest {
        output_path: out.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: Some((0, 400_000)),
    };
    let settings = resolve_settings(&flowed, &request).expect("settings");
    let export = ExportJob {
        job_id: "flow".into(),
        project: flowed.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(&flowed)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    run_export(&export, &FnSink(|_| {})).expect("export");
    let mut decoder = VideoDecoder::open(&out).expect("open the export");
    let frame = decoder.seek_and_decode(at(6) + 5_000).expect("decode");
    let p = &frame.data[((20 * frame.width + 160) * 4) as usize..][..3];
    assert!(p[0] > 200 && p[1] < 60 && p[2] < 60, "exported: {p:?}");
    let frame = decoder.seek_and_decode(at(8) + 5_000).expect("decode");
    let p = &frame.data[((20 * frame.width + 300) * 4) as usize..][..3];
    assert!(p[0] < 80, "a source frame in the export: {p:?}");
    let _ = std::fs::remove_file(&out);
}

/// The square's left edge on the middle row of a straight-RGBA picture, by
/// where the row turns bright, and how many pixels are neither bright nor
/// dark (a ghost of a blend).
fn edge_and_ghost(data: &[u8], width: u32) -> (Option<u32>, usize) {
    let y = 70 + SQUARE / 2;
    let row: Vec<u8> = (0..width)
        .map(|x| data[((y * width + x) * 4) as usize])
        .collect();
    // A half-and-half mix of white over the dark grey is about 190 (the mix
    // is in linear light), the square itself 255, the background 48.
    let edge = row.iter().position(|&v| v > 235).map(|x| x as u32);
    let ghost = row.iter().filter(|&&v| v > 110 && v < 225).count();
    (edge, ghost)
}

/// A real bake through the worker, where it and RIFE are installed: the
/// in-between frame shows one square, half-way, where the blend shows two
/// at half strength. Skipped (not downloaded) on a machine without them.
#[test]
fn rife_puts_the_square_half_way_where_the_blend_shows_two() {
    use chukcut_ml_worker::registry;
    let root = chukcut_engine::modules::ml::root();
    let spec = registry::model("rife").expect("the registry lists RIFE");
    if chukcut_engine::modules::ml::worker::binary().is_none()
        || !registry::model_present(&root, spec)
        || registry::preferred_runtime(&root).is_none()
    {
        eprintln!("skipping: the ML worker, ONNX Runtime or RIFE is not installed");
        return;
    }
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = square("rife.mp4") else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let key = flow::key_for(&path, None).unwrap();
    for dir in flow::dirs_of(&key) {
        let _ = std::fs::remove_dir_all(&dir);
    }
    let state = state_with(&path, FrameBlend::Flow);
    let job = speed::speed_flow_bake(&state, "clip".into())
        .unwrap()
        .expect("a bake");
    let done = speed::speed_flow_wait(job, |_| {}).expect("bake");
    assert_eq!(done.written, 22, "{done:?}");
    eprintln!(
        "RIFE baked 22 frames of {W}x{H} in {:.2} s on {:?} ({:?} ms a frame in the model)",
        done.seconds, done.provider, done.model_millis_per_frame
    );
    assert!(speed::speed_flow_bake(&state, "clip".into())
        .unwrap()
        .is_none());

    let flowed = project_of(&state);
    let blended = project_of(&state_with(&path, FrameBlend::Blend));
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
    // Timeline frame 6 is source frame 1.5: the square's edge half-way
    // between where frames 1 and 2 (timeline frames 4 and 8) have it.
    let (one, _) = edge_and_ghost(&render(&flowed, 4), W);
    let (two, _) = edge_and_ghost(&render(&flowed, 8), W);
    let (one, two) = (one.expect("frame 1"), two.expect("frame 2"));
    assert_eq!(two - one, STEP);
    let truth = one + STEP / 2;
    let (edge, ghost) = edge_and_ghost(&render(&flowed, 6), W);
    let edge = edge.expect("a square");
    assert!(
        (edge as i64 - truth as i64).abs() <= 2,
        "flow edge {edge}, truth {truth}"
    );
    assert!(ghost <= 4, "one square, no ghost: {ghost} grey pixels");
    let (_, blend_ghost) = edge_and_ghost(&render(&blended, 6), W);
    assert!(
        blend_ghost >= 2 * STEP as usize - 8,
        "the blend shows two squares at half strength: {blend_ghost} grey pixels"
    );
}
