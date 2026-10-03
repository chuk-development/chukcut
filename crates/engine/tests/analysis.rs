//! Scene detection, stabilisation, beat detection and auto reframe end to
//! end: generated media through the decoders, the analyses, the command
//! layer and the undo stack.
//!
//! Every fixture is made by `ffmpeg` from a formula, so each test knows the
//! right answer: where the shots change, how the camera shook, where the
//! clicks are, where the subject is.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use chukcut_engine::modules::analysis::commands::{self as analysis, DetectScenes, Reframe};
use chukcut_engine::modules::analysis::frames::Walk;
use chukcut_engine::modules::analysis::stabilise;
use chukcut_engine::modules::project::document::{
    AnimatableProperty, AudioMaterial, CanvasConfig, Micros, Project, Segment, TimeRange, Track,
    TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::state::AppState;

fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/analysis-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// Run ffmpeg into `name` unless it is there already. `None` without ffmpeg.
fn generate(name: &str, args: &[&str]) -> Option<PathBuf> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = fixture_dir().join(name);
    if path.exists() {
        return Some(path);
    }
    let ext = path.extension()?.to_str()?.to_string();
    let tmp = path.with_extension(format!("tmp.{ext}"));
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args(args)
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

/// Three shots of one second each: test pattern, colour bars, fractal.
fn three_shots() -> Option<PathBuf> {
    generate(
        "three-shots.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=30:duration=1",
            "-f",
            "lavfi",
            "-i",
            "smptebars=size=320x180:rate=30:duration=1",
            "-f",
            "lavfi",
            "-i",
            "mandelbrot=size=320x180:rate=30",
            "-filter_complex",
            "[2]trim=duration=1[m];[0][1][m]concat=n=3:v=1:a=0",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

/// 3 s of a 1120×630 window shaking over a still 1280×720 test pattern
/// that an orange disc crosses: the window sits at
/// x = 80 + 40·sin(2π·1.5t), y = 45 + 25·sin(2π·1.1t + 1). The background is
/// still on purpose — `testsrc2`'s own sweeping line puts most of the
/// frame's corners on a moving thing — and the disc is the subject walking
/// through that the camera estimate has to ignore.
fn shaky() -> Option<PathBuf> {
    generate(
        "shaky.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=3,trim=end_frame=1,\
             loop=loop=89:size=1,setpts=N/30/TB",
            "-f",
            "lavfi",
            "-i",
            "color=c=orange:size=120x120:rate=30:duration=3,format=rgba,\
             geq=r='250':g='140':b='20':a='if(lte(hypot(X-59.5,Y-59.5),60),255,0)'",
            "-filter_complex",
            "[0][1]overlay=x='100+300*t':y=300:eval=frame,\
             crop=w=1120:h=630:x='80+40*sin(2*PI*1.5*t)':y='45+25*sin(2*PI*1.1*t+1)'",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

fn shake_truth(seconds: f64) -> (f64, f64) {
    let x = |t: f64| 80.0 + 40.0 * (2.0 * std::f64::consts::PI * 1.5 * t).sin();
    let y = |t: f64| 45.0 + 25.0 * (2.0 * std::f64::consts::PI * 1.1 * t + 1.0).sin();
    // The window moving right moves the picture in it left.
    (
        -(x(seconds) - x(0.0)) / 1120.0,
        -(y(seconds) - y(0.0)) / 630.0,
    )
}

/// 10 s of clicks every half second from 0.25 s (120 BPM) over a low hum.
fn clicks() -> Option<PathBuf> {
    generate(
        "clicks-120.wav",
        &[
            "-f", "lavfi", "-i",
            "aevalsrc='0.05*sin(2*PI*110*t)+0.8*gte(t,0.25)*sin(2*PI*1500*t)*exp(-150*mod(t-0.25,0.5))':s=44100:d=10",
            "-c:a", "pcm_s16le",
        ],
    )
}

/// 4 s, 1280×720: a light disc of radius 50 moving from x = 140 to x = 1140
/// over a dark, still background.
fn moving_subject() -> Option<PathBuf> {
    generate(
        "moving-subject.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=0x202428:size=1280x720:rate=30:duration=4",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:size=100x100:rate=30:duration=4,format=rgba,\
             geq=r='235':g='200':b='170':a='if(lte(hypot(X-49.5,Y-49.5),50),255,0)'",
            "-filter_complex",
            "[0][1]overlay=x='90+250*t':y=310:eval=frame",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

fn segment(id: &str, material: &str, duration: Micros) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(0, duration),
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

fn video_state(path: &std::path::Path, size: (u32, u32), duration: Micros) -> Arc<AppState> {
    let mut project = Project::new("analysis", CanvasConfig::default(), 30.0);
    project.canvas.width = size.0;
    project.canvas.height = size.1;
    project.materials.videos.push(VideoMaterial {
        id: "v".into(),
        path: path.to_string_lossy().into(),
        width: size.0,
        height: size.1,
        duration,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment("clip", "v", duration));
    project.tracks.push(track);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

fn starts(state: &AppState) -> Vec<Micros> {
    state
        .with_project(|p| {
            p.tracks[0]
                .segments
                .iter()
                .map(|s| s.target_range.start)
                .collect()
        })
        .unwrap()
}

#[test]
fn three_shots_are_found_and_split_in_one_step() {
    let Some(path) = three_shots() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = video_state(&path, (320, 180), 3_000_000);
    let job = analysis::analysis_detect_scenes(
        &state,
        DetectScenes {
            segment_id: "clip".into(),
            sensitivity: 0.5,
            split: true,
        },
        None,
    )
    .unwrap();
    let message = analysis::analysis_wait(job).unwrap();
    assert_eq!(message, "Split at 2 scene changes");
    let cuts = starts(&state);
    assert_eq!(cuts.len(), 3, "{cuts:?}");
    assert!((cuts[1] - 1_000_000).abs() <= 34_000, "{cuts:?}");
    assert!((cuts[2] - 2_000_000).abs() <= 34_000, "{cuts:?}");
    // The marks stay on the pieces.
    let marks = state
        .with_project(|p| {
            analysis::analysis_of(p, &p.tracks[0].segments[0].id)
                .scenes
                .len()
        })
        .unwrap();
    assert_eq!(marks, 0, "the first piece holds no cut inside it");
    // One undo takes the splits and the marks away together.
    timeline_undo(&state).unwrap();
    assert_eq!(starts(&state).len(), 1);
    assert!(state
        .with_project(|p| analysis::analysis_of(p, "clip").scenes.is_empty())
        .unwrap());
}

#[test]
fn camera_shake_is_measured_and_cancelled() {
    let Some(path) = shaky() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let walk = Walk {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        range: TimeRange::new(0, 3_000_000),
        height: stabilise::ANALYSIS_HEIGHT,
        max_rate: None,
    };
    let camera = stabilise::measure(&walk, "v", None).unwrap();
    assert!(
        camera.samples.len() >= 85,
        "{} frames",
        camera.samples.len()
    );
    let mut worst = 0.0f64;
    for s in &camera.samples {
        let (tx, ty) = shake_truth(s.t as f64 / 1e6);
        worst = worst
            .max((s.x as f64 - tx).abs())
            .max((s.y as f64 - ty).abs());
    }
    // Four pixels at 1120 wide, over three seconds of accumulated motion.
    assert!(worst < 0.0045, "the path is off by {worst}");

    // Through the command layer: the clip ends up stabilised, and the crop
    // window it is drawn through moves against the shake.
    let state = video_state(&path, (1120, 630), 3_000_000);
    let job = analysis::analysis_stabilise(&state, "clip".into(), 1.0, None, None).unwrap();
    let message = analysis::analysis_wait(job).unwrap();
    assert!(message.starts_with("Stabilised"), "{message}");
    let project = state.project.read().clone().unwrap();
    let clip = project.segment("clip").unwrap().1.clone();
    let mean_x =
        camera.samples.iter().map(|s| s.x as f64).sum::<f64>() / camera.samples.len() as f64;
    for t in [200_000i64, 700_000, 1_300_000, 2_100_000] {
        let drawn = stabilise::resolve(&project, std::borrow::Cow::Borrowed(&clip), t);
        let crop = drawn.crop.expect("a stabilised clip is cropped");
        let centre = (crop.left + crop.right) as f64 * 0.5;
        let (tx, _) = shake_truth(t as f64 / 1e6);
        // Picture drift plus window shift is the same at every instant.
        let expected = 0.5 + (tx - mean_x);
        assert!(
            (centre - expected).abs() < 0.006,
            "at {t}: {centre} vs {expected}"
        );
        assert!(crop.left >= 0.0 && crop.right <= 1.0);
    }
    timeline_undo(&state).unwrap();
    assert!(state
        .with_project(|p| analysis::analysis_of(p, "clip").stabilise.is_none())
        .unwrap());
}

#[test]
fn beats_drive_auto_cut_and_snap() {
    let (Some(music), Some(video)) = (clicks(), three_shots()) else {
        eprintln!("skipping: ffmpeg could not generate the fixtures");
        return;
    };
    let state = video_state(&video, (320, 180), 3_000_000);
    state
        .with_project_mut(|p| {
            p.materials.audios.push(AudioMaterial {
                id: "music".into(),
                path: music.to_string_lossy().into(),
                duration: 10_000_000,
                sample_rate: 44_100,
                channels: 1,
            });
            let mut track = Track::new(TrackKind::Audio, "Music");
            track.segments.push(segment("song", "music", 10_000_000));
            p.tracks.push(track);
        })
        .unwrap();
    let job = analysis::analysis_detect_beats(&state, "song".into(), None).unwrap();
    let message = analysis::analysis_wait(job).unwrap();
    assert!(message.starts_with("120 BPM"), "{message}");
    let beats = state
        .with_project(|p| analysis::analysis_of(p, "song").beats)
        .unwrap();
    let on_grid = beats
        .iter()
        .filter(|b| {
            let k = ((**b as f64 / 1e6 - 0.25) / 0.5).round();
            (**b as f64 / 1e6 - (0.25 + 0.5 * k)).abs() < 0.03
        })
        .count();
    assert!(on_grid as f64 >= beats.len() as f64 * 0.9, "{beats:?}");

    // Every other beat inside the three-second clip: 0.25, 1.25, 2.25 — or
    // the other half of the grid — two or three cuts either way.
    analysis::analysis_auto_cut_to_beat(&state, vec!["clip".into()], 2).unwrap();
    let cuts = starts(&state);
    assert!(cuts.len() >= 3, "{cuts:?}");
    for cut in &cuts[1..] {
        let k = ((*cut as f64 / 1e6 - 0.25) / 0.5).round();
        assert!(
            (*cut as f64 / 1e6 - (0.25 + 0.5 * k)).abs() < 0.05,
            "{cuts:?}"
        );
    }
    timeline_undo(&state).unwrap();
    assert_eq!(starts(&state).len(), 1);
}

#[test]
fn a_moving_subject_is_kept_in_a_vertical_frame() {
    let Some(path) = moving_subject() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = video_state(&path, (1280, 720), 4_000_000);
    let job = analysis::analysis_reframe(
        &state,
        Reframe {
            segment_ids: vec!["clip".into()],
            ratio: Some((9, 16)),
        },
        None,
    )
    .unwrap();
    let message = analysis::analysis_wait(job).unwrap();
    assert!(message.starts_with("Reframed to 720×1280"), "{message}");
    let project = state.project.read().clone().unwrap();
    assert_eq!((project.canvas.width, project.canvas.height), (720, 1280));
    let clip = project.segment("clip").unwrap().1.clone();
    let scale = clip.transform.scale[0];
    let qw = 720.0 * scale; // the quad's width on the canvas
    let track = clip
        .keyframes
        .iter()
        .find(|k| k.property == AnimatableProperty::PositionX)
        .expect("the window moves");
    let window = 720.0 / qw;
    for t in [500_000i64, 1_500_000, 2_500_000, 3_500_000] {
        let position = track.sample(t).unwrap();
        // Position (canvas half-widths) back to the window centre in the
        // frame: x_px = (0.5 - u)·qw.
        let u = 0.5 - position * 360.0 / qw;
        let subject = (140.0 + 250.0 * t as f32 / 1e6) / 1280.0;
        assert!(
            (u - subject).abs() < window * 0.5,
            "at {t}: window at {u}, subject at {subject}"
        );
    }
    timeline_undo(&state).unwrap();
    state
        .with_project(|p| {
            assert_eq!((p.canvas.width, p.canvas.height), (1280, 720));
            assert!(p.segment("clip").unwrap().1.keyframes.is_empty());
        })
        .unwrap();
}

/// 1 s, 320×180: the left half black, the right half white.
fn half_and_half() -> Option<PathBuf> {
    generate(
        "half-and-half.mp4",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=160x180:rate=30:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:size=160x180:rate=30:duration=1",
            "-filter_complex",
            "[0][1]hstack",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "10",
            "-pix_fmt",
            "yuv420p",
        ],
    )
}

/// The stabilised picture comes out of the compositor itself, in the preview
/// and in the export alike: the crop window moves against the shake there,
/// not only in the arithmetic.
#[test]
fn the_compositor_draws_a_stabilised_clip_through_its_window() {
    use chukcut_engine::modules::analysis::store::{self, CameraPath, PathSample, Stabilise};
    use chukcut_engine::modules::media::MediaSourceProvider;
    use chukcut_engine::modules::render::{Compositor, SourceProvider};

    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let Some(path) = half_and_half() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let state = video_state(&path, (320, 180), 1_000_000);
    let mut project = state.project.read().clone().unwrap();
    // The picture "drifted" right by a tenth of the frame at 0.5 s and back:
    // tripod strength puts the window 0.1 to the right of centre there,
    // against the drift (the mean of the path is 0.1/3).
    let samples = [(0, 0.0), (500_000, 0.1), (999_999, 0.0)]
        .map(|(t, x)| PathSample {
            t,
            x,
            y: 0.0,
            a: 0.0,
            cut: false,
        })
        .to_vec();
    let (motion_id, motion) = store::new_entry(&CameraPath {
        media_id: "v".into(),
        analysed: TimeRange::new(0, 1_000_000),
        aspect: 16.0 / 9.0,
        samples,
    });
    let (stabilise_id, stabilise) = store::new_entry(&Stabilise {
        motion_id: motion_id.clone(),
        enabled: true,
        strength: 1.0,
        crop: Some(0.3),
    });
    project.materials.extras.insert(motion_id, motion);
    project
        .materials
        .extras
        .insert(stabilise_id.clone(), stabilise);
    project.tracks[0].segments[0].extras.push(stabilise_id);

    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let frame = compositor
        .render(&project, 500_000, (320, 180), sources.as_ref())
        .expect("render");
    // The window is 0.7 wide, centred at 0.5 - (0.1/3 - 0.1) = 0.567: it
    // shows source 0.217..0.917, so the black-white edge (0.5) lands at
    // (0.5 - 0.217) / 0.7 = 40 % of the width instead of 50 %.
    let edge = (0..320u32)
        .find(|&x| frame.pixel(x, 90)[0] > 128)
        .expect("a white half");
    assert!((edge as f32 / 320.0 - 0.405).abs() < 0.02, "edge at {edge}");

    // Off, it is the plain picture again.
    let mut plain = project.clone();
    plain.tracks[0].segments[0].extras.clear();
    let frame = compositor
        .render(&plain, 500_000, (320, 180), sources.as_ref())
        .expect("render");
    let edge = (0..320u32)
        .find(|&x| frame.pixel(x, 90)[0] > 128)
        .expect("a white half");
    assert!((edge as f32 / 320.0 - 0.5).abs() < 0.02, "edge at {edge}");

    // The export path draws the same window.
    let Ok(export) = compositor.render_nv12(&project, 500_000, (320, 180), sources.as_ref()) else {
        eprintln!("skipping the export half: no RGBA to NV12 pass on this device");
        return;
    };
    let luma = export.y();
    let stride = export.y_stride;
    let edge = (0..320usize)
        .find(|&x| luma[90 * stride + x] > 128)
        .expect("a white half in the export");
    assert!(
        (edge as f32 / 320.0 - 0.405).abs() < 0.02,
        "export edge at {edge}"
    );
}
