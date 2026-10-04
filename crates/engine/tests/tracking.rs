//! Motion tracking end to end: a real encoded clip through the decoder and
//! the tracker, and a follower through the compositor in both the preview and
//! the export path.
//!
//! The clip is generated: a red disc moving on a known path over `testsrc2`
//! (whose colour bars, gradients and counter make a busy, partly moving
//! background). The path is a formula, so every tracked frame has an exact
//! answer to compare against.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::project::document::{
    CanvasConfig, ImageMaterial, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_engine::modules::render::{
    Compositor, CompositorConfig, SolidColorProvider, SolidSource,
};
use chukcut_engine::modules::tracking::edit;
use chukcut_engine::modules::tracking::follow::{followed_transform, source_to_canvas};
use chukcut_engine::modules::tracking::job::{run, Direction, TrackJob};
use chukcut_engine::modules::tracking::{
    FollowMode, TrackSample, TrackSettings, TrackerKind, TrackingMaterial,
};

const W: f32 = 1280.0;
const H: f32 = 720.0;
const RADIUS: f32 = 38.0;

/// Centre of the disc at `seconds`, as a distance in pixels from the frame's
/// top-left corner. Must match the overlay expression in [`moving_disc`]: the
/// overlay is 80 px square and the disc is centred on its pixel 39.5, whose
/// centre is 40 px in.
fn truth(seconds: f64) -> (f32, f32) {
    let x = 100.0 + 280.0 * seconds + 40.0;
    let y = 320.0 + 150.0 * (1.7 * seconds).sin() + 40.0;
    (x as f32, y as f32)
}

fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/tracking-v1");
    std::fs::create_dir_all(&dir).expect("create the fixture directory");
    dir
}

/// 4 s, 1280×720, 30 fps: a red disc of radius 38 px moving 280 px/s right
/// and swinging ±150 px vertically, over testsrc2. `None` without ffmpeg.
fn moving_disc() -> Option<PathBuf> {
    // The tests run in parallel and must not encode the same file at once.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = fixture_dir().join("moving-disc.mp4");
    if path.exists() {
        return Some(path);
    }
    let disc = format!(
        "color=c=red:size=80x80:rate=30:duration=4,format=rgba,\
         geq=r='230':g='30':b='40':a='if(lte(hypot(X-39.5,Y-39.5),{RADIUS}),255,0)'"
    );
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=4",
        ])
        .args(["-f", "lavfi", "-i", &disc])
        .args([
            "-filter_complex",
            "[0][1]overlay=x='100+280*t':y='320+150*sin(1.7*t)':eval=frame",
        ])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"])
        .args(["-pix_fmt", "yuv420p", "-t", "4"])
        .arg(path.with_extension("tmp.mp4"))
        .stdout(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::rename(path.with_extension("tmp.mp4"), &path).ok()?;
    Some(path)
}

/// The owner's case: a ball thrown across the frame. 2 s, 1280×720, 30 fps; a
/// disc of radius 22 px on a parabola, about 30 px per frame at full size
/// (15 at the analysis size), with the motion blur of a 180° shutter: drawn
/// at 120 fps, two sub-frames averaged, every fourth kept. It leaves the
/// frame at about 1.38 s.
fn thrown_ball() -> Option<PathBuf> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = fixture_dir().join("thrown-ball.mp4");
    if path.exists() {
        return Some(path);
    }
    let disc = "color=c=orange:size=48x48:rate=30:duration=2,format=rgba,\
                geq=r='250':g='140':b='20':a='if(lte(hypot(X-23.5,Y-23.5),22),255,0)'";
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=120:duration=2",
        ])
        .args(["-f", "lavfi", "-i", disc])
        .args([
            "-filter_complex",
            "[0][1]overlay=x='40+900*t':y='560-900*t+450*t*t':eval=frame,\
             tmix=frames=2,select='not(mod(n\\,4))',setpts=N/30/TB",
        ])
        .args(["-r", "30"])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"])
        .args(["-pix_fmt", "yuv420p", "-t", "2"])
        .arg(path.with_extension("tmp.mp4"))
        .stdout(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::rename(path.with_extension("tmp.mp4"), &path).ok()?;
    Some(path)
}

/// Centre of the thrown ball at `seconds`: the middle of the blur, half a
/// sub-frame (1/240 s) before the frame's own instant.
fn thrown_truth(seconds: f64) -> (f32, f32) {
    let t = (seconds - 1.0 / 240.0).max(0.0);
    let x = 40.0 + 900.0 * t + 24.0;
    let y = 560.0 - 900.0 * t + 450.0 * t * t + 24.0;
    (x as f32, y as f32)
}

#[test]
fn a_thrown_ball_is_tracked() {
    let Some(path) = thrown_ball() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let (cx, cy) = thrown_truth(0.0);
    let side = 52.0;
    let job = TrackJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W as u32, H as u32),
        start: 0,
        init: [cx / W, cy / H, side / W, side / H],
        init_angle: 0.0,
        // Until just before it leaves the frame.
        range: (0, 1_300_000),
        direction: Direction::Forward,
        analysis_size: 640,
        tracker: TrackerKind::Klt,
    };
    let outcome = run(&job, &AtomicBool::new(false), |_| {}).expect("track");
    if std::env::var_os("TRACKING_DEBUG").is_some() {
        for s in &outcome.samples {
            let (tx, ty) = thrown_truth(s.t as f64 / 1e6);
            eprintln!(
                "{:>8} got ({:7.1},{:7.1}) want ({:7.1},{:7.1}) c {:.2} f {}",
                s.t,
                s.x * W,
                s.y * H,
                tx,
                ty,
                s.c,
                s.f
            );
        }
    }
    let mut worst = 0.0f32;
    let mut sum = 0.0f32;
    for s in &outcome.samples {
        let (tx, ty) = thrown_truth(s.t as f64 / 1e6);
        let e = ((s.x * W - tx).powi(2) + (s.y * H - ty).powi(2)).sqrt();
        worst = worst.max(e);
        sum += e;
    }
    let mean = sum / outcome.samples.len() as f32;
    let lost = outcome.samples.iter().filter(|s| s.is_lost()).count();
    eprintln!(
        "thrown: {} frames, mean {mean:.2} px, worst {worst:.2} px, {lost} lost",
        outcome.samples.len()
    );
    assert!(
        outcome.samples.len() >= 38,
        "{} frames",
        outcome.samples.len()
    );
    assert_eq!(lost, 0);
    assert!(mean < 4.0, "mean error {mean} px");
    assert!(worst < 10.0, "worst error {worst} px");
}

/// VitTrack (tracker T2) on the same throw. Runs only where the ML worker,
/// a runtime pack and the model are already installed: a test must not
/// download 9 MB behind someone's back. `cargo build -p chukcut-ml-worker`
/// and one auto reframe or `chukcut-cli` ML install set that up.
#[test]
fn a_thrown_ball_is_held_by_vittrack() {
    if !vittrack_ready() {
        return;
    }
    let Some(path) = thrown_ball() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let (cx, cy) = thrown_truth(0.0);
    let side = 52.0;
    let job = TrackJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W as u32, H as u32),
        start: 0,
        init: [cx / W, cy / H, side / W, side / H],
        init_angle: 0.0,
        range: (0, 1_300_000),
        direction: Direction::Forward,
        analysis_size: 640,
        tracker: TrackerKind::VitTrack,
    };
    let started = std::time::Instant::now();
    let outcome = run(&job, &AtomicBool::new(false), |_| {}).expect("track");
    let seconds = started.elapsed().as_secs_f32();
    let (mut worst, mut sum) = (0.0f32, 0.0f32);
    for s in &outcome.samples {
        let (tx, ty) = thrown_truth(s.t as f64 / 1e6);
        let e = ((s.x * W - tx).powi(2) + (s.y * H - ty).powi(2)).sqrt();
        worst = worst.max(e);
        sum += e;
    }
    let n = outcome.samples.len();
    let mean = sum / n as f32;
    let lost = outcome.samples.iter().filter(|s| s.is_lost()).count();
    eprintln!(
        "vittrack thrown: {n} frames in {seconds:.2} s, mean {mean:.2} px, worst {worst:.2} px, {lost} lost"
    );
    assert!(n >= 38, "{n} frames");
    assert_eq!(lost, 0);
    // VitTrack boxes are whole pixels at the 640 px analysis size (2 px at
    // full size) and it has no sub-pixel flow, so it is looser than KLT on
    // this clean throw; it earns its place on real motion blur.
    assert!(mean < 6.0, "mean error {mean} px");
    assert!(worst < 14.0, "worst error {worst} px");
}

/// Whether the ML worker, a runtime pack and VitTrack are installed, and
/// VitTrack is loaded. A test must not download behind someone's back, so
/// without them the VitTrack tests skip with a note.
fn vittrack_ready() -> bool {
    use chukcut_engine::modules::ml;
    use chukcut_ml_worker::registry;
    let root = ml::root();
    let ready = ml::worker::binary().is_some()
        && registry::model_present(&root, registry::model("vittrack").unwrap())
        && (registry::preferred_runtime(&root).is_some()
            || std::env::var_os("CHUKCUT_ORT_DYLIB").is_some());
    if !ready {
        eprintln!("skipping: the ML worker, a runtime or VitTrack is not installed");
        return false;
    }
    ml::tracker::VitTracker::prepare(&|_, _| {}, &AtomicBool::new(false)).expect("prepare");
    true
}

/// Where the ball of [`hide_and_return`] is at `seconds`: its centre in
/// pixels, and whether it is wholly in view (not behind the bar, not cut by
/// the frame's edge).
fn hide_and_return_truth(seconds: f64) -> ((f32, f32), bool) {
    let (x, y) = if seconds < 3.3 {
        (100.0 + 400.0 * seconds, 300.0)
    } else {
        (-48.0 + 400.0 * (seconds - 3.3), 500.0)
    };
    let behind_bar = x + 48.0 > 560.0 && x < 760.0;
    let in_frame = x >= 0.0 && x + 48.0 <= W as f64;
    (
        ((x + 24.0) as f32, (y + 24.0) as f32),
        in_frame && !behind_bar,
    )
}

/// The two cases a box tracker that only looks near the last box cannot
/// survive. 4.5 s, 1280×720, 30 fps; an orange ball of radius 22 px moving
/// right at 400 px/s over testsrc2:
///
/// - from 1.03 s to 1.65 s it passes behind a grey bar (x 560–760) and comes
///   out 200 px from where it went in;
/// - at 2.83 s it leaves the frame on the right, and at 3.3 s it comes back
///   in from the left, 200 px lower.
fn hide_and_return() -> Option<PathBuf> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = fixture_dir().join("hide-and-return.mp4");
    if path.exists() {
        return Some(path);
    }
    let disc = "color=c=orange:size=48x48:rate=30:duration=4.5,format=rgba,\
                geq=r='250':g='140':b='20':a='if(lte(hypot(X-23.5,Y-23.5),22),255,0)'";
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1280x720:rate=30:duration=4.5",
        ])
        .args(["-f", "lavfi", "-i", disc])
        .args([
            "-filter_complex",
            "[0][1]overlay=x='if(lt(t,3.3),100+400*t,-48+400*(t-3.3))':\
             y='if(lt(t,3.3),300,500)':eval=frame,\
             drawbox=x=560:y=0:w=200:h=720:color=gray:t=fill",
        ])
        .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"])
        .args(["-pix_fmt", "yuv420p", "-t", "4.5"])
        .arg(path.with_extension("tmp.mp4"))
        .stdout(Stdio::null())
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    std::fs::rename(path.with_extension("tmp.mp4"), &path).ok()?;
    Some(path)
}

/// Tracker T2's reason to exist beyond speed: the ball goes behind a bar and
/// comes out further on, then leaves the frame and comes back elsewhere, and
/// VitTrack finds it again both times. While it is hidden the frames are
/// marked lost, not glued to the bar or the background.
#[test]
fn vittrack_finds_the_ball_again_after_occlusion_and_leaving_the_frame() {
    if !vittrack_ready() {
        return;
    }
    let Some(path) = hide_and_return() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let ((cx, cy), _) = hide_and_return_truth(0.0);
    let side = 52.0;
    let job = TrackJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W as u32, H as u32),
        start: 0,
        init: [cx / W, cy / H, side / W, side / H],
        init_angle: 0.0,
        range: (0, 4_466_666),
        direction: Direction::Forward,
        analysis_size: 640,
        tracker: TrackerKind::VitTrack,
    };
    let started = std::time::Instant::now();
    let outcome = run(&job, &AtomicBool::new(false), |_| {}).expect("track");
    let seconds = started.elapsed().as_secs_f32();

    // Visible frames, split by the phase they are in; the first 0.2 s after
    // each reappearance are the tracker's to spend finding the ball again.
    let (mut held, mut refound_bar, mut refound_edge) = (vec![], vec![], vec![]);
    let (mut hidden, mut hidden_lost) = (0, 0);
    for s in &outcome.samples {
        let t = s.t as f64 / 1e6;
        let ((tx, ty), visible) = hide_and_return_truth(t);
        let error = ((s.x * W - tx).powi(2) + (s.y * H - ty).powi(2)).sqrt();
        if !visible {
            hidden += 1;
            hidden_lost += s.is_lost() as usize;
            continue;
        }
        if t < 1.0 {
            held.push(error);
        } else if (1.85..2.8).contains(&t) {
            refound_bar.push(error);
        } else if t > 3.62 {
            refound_edge.push(error);
        }
    }
    let worst = |v: &[f32]| v.iter().copied().fold(0.0f32, f32::max);
    let missed = |v: &[f32]| v.iter().filter(|e| **e > 12.0).count();
    if std::env::var_os("TRACKING_DEBUG").is_some() {
        for s in &outcome.samples {
            let ((tx, ty), visible) = hide_and_return_truth(s.t as f64 / 1e6);
            eprintln!(
                "{:>8} truth ({tx:6.1},{ty:6.1}) {visible:5} got ({:6.1},{:6.1}) c {:.2} lost {}",
                s.t,
                s.x * W,
                s.y * H,
                s.c,
                s.is_lost()
            );
        }
    }
    eprintln!(
        "hide and return: {} frames in {seconds:.2} s; worst {:.1} / {:.1} / {:.1} px; \
         {hidden_lost} of {hidden} hidden frames lost",
        outcome.samples.len(),
        worst(&held),
        worst(&refound_bar),
        worst(&refound_edge),
    );
    assert!(outcome.samples.len() >= 130);
    assert!(!refound_bar.is_empty() && !refound_edge.is_empty());
    assert_eq!(missed(&held), 0, "lost before the bar");
    assert_eq!(missed(&refound_bar), 0, "not found again after the bar");
    assert_eq!(missed(&refound_edge), 0, "not found again after leaving");
    // Most hidden frames are marked lost rather than tracking something
    // else. Not all: the first frame or two behind the bar still see the
    // ball's last sliver.
    assert!(
        hidden_lost * 10 >= hidden * 7,
        "{hidden_lost} of {hidden} hidden frames lost"
    );
}

fn job(path: &std::path::Path, start: Micros, direction: Direction) -> TrackJob {
    let (cx, cy) = truth(start as f64 / 1e6);
    let side = 2.0 * RADIUS + 6.0;
    TrackJob {
        path: path.to_string_lossy().into(),
        fps: 30.0,
        source_size: (W as u32, H as u32),
        start,
        init: [cx / W, cy / H, side / W, side / H],
        init_angle: 0.0,
        range: (0, 3_966_666),
        direction,
        analysis_size: 640,
        tracker: TrackerKind::Klt,
    }
}

/// Mean and worst centre error, full-resolution pixels, and lost frames.
fn errors(samples: &[TrackSample]) -> (f32, f32, usize) {
    let mut sum = 0.0;
    let mut worst = 0.0f32;
    for s in samples {
        let (tx, ty) = truth(s.t as f64 / 1e6);
        let e = ((s.x * W - tx).powi(2) + (s.y * H - ty).powi(2)).sqrt();
        sum += e;
        worst = worst.max(e);
    }
    let lost = samples.iter().filter(|s| s.is_lost()).count();
    (sum / samples.len().max(1) as f32, worst, lost)
}

#[test]
fn a_moving_disc_is_tracked_through_a_real_clip() {
    let Some(path) = moving_disc() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let started = std::time::Instant::now();
    let outcome = run(
        &job(&path, 0, Direction::Forward),
        &AtomicBool::new(false),
        |_| {},
    )
    .expect("track");
    let seconds = started.elapsed().as_secs_f64();
    if std::env::var_os("TRACKING_DEBUG").is_some() {
        for s in &outcome.samples {
            let (tx, ty) = truth(s.t as f64 / 1e6);
            eprintln!(
                "{:>8} got ({:7.1},{:7.1}) want ({:7.1},{:7.1}) size {:5.1} c {:.2} f {}",
                s.t,
                s.x * W,
                s.y * H,
                tx,
                ty,
                s.w * W,
                s.c,
                s.f
            );
        }
    }
    let (mean, worst, lost) = errors(&outcome.samples);
    eprintln!(
        "forward: {} frames in {seconds:.2} s ({:.0} fps) at {:?}; centre error mean {mean:.2} px, \
         worst {worst:.2} px (1280×720), {lost} lost",
        outcome.samples.len(),
        outcome.samples.len() as f64 / seconds,
        outcome.frame_size,
    );
    assert!(
        outcome.samples.len() >= 115,
        "{} frames",
        outcome.samples.len()
    );
    assert_eq!(lost, 0);
    assert!(mean < 3.0, "mean error {mean} px");
    assert!(worst < 8.0, "worst error {worst} px");
    // A disc does not turn: whatever the flow makes of its round edge must
    // not add up to a visible rotation.
    let turned = outcome
        .samples
        .iter()
        .map(|s| s.a.abs())
        .fold(0.0f32, f32::max);
    eprintln!("largest rotation {turned:.2}°");
    assert!(turned < 5.0, "the disc turned by {turned}°");
    // A disc does not grow: the scale stays put.
    let first = outcome.samples[0];
    for s in &outcome.samples {
        let ratio = (s.w * s.h / (first.w * first.h)).sqrt();
        assert!(
            (ratio - 1.0).abs() < 0.15,
            "scale drifted to {ratio} at {}",
            s.t
        );
    }
}

#[test]
fn tracking_both_ways_from_the_middle_covers_the_clip() {
    let Some(path) = moving_disc() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let outcome = run(
        &job(&path, 2_000_000, Direction::Both),
        &AtomicBool::new(false),
        |_| {},
    )
    .expect("track");
    let (mean, worst, lost) = errors(&outcome.samples);
    eprintln!(
        "both: {} frames, mean {mean:.2} px, worst {worst:.2} px, {lost} lost",
        outcome.samples.len()
    );
    assert!(outcome.samples.first().unwrap().t < 50_000);
    assert!(outcome.samples.last().unwrap().t > 3_900_000);
    assert!(outcome.samples.windows(2).all(|w| w[0].t < w[1].t));
    assert_eq!(lost, 0);
    assert!(mean < 3.0, "mean error {mean} px");
    assert!(worst < 8.0, "worst error {worst} px");
}

#[test]
fn a_cancelled_run_keeps_what_it_had() {
    let Some(path) = moving_disc() else {
        eprintln!("skipping: ffmpeg could not generate the fixture");
        return;
    };
    let cancel = AtomicBool::new(false);
    let outcome = run(&job(&path, 0, Direction::Forward), &cancel, |p| {
        if p.done >= 10 {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })
    .expect("track");
    assert!(outcome.cancelled);
    assert!(
        (10..=12).contains(&outcome.samples.len()),
        "{}",
        outcome.samples.len()
    );
}

// --- the follower on the GPU -----------------------------------------------------

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

/// A grey 640×480 "video" under a red 32×32 "sticker" that follows a track
/// moving the object from (0.2, 0.3) to (0.8, 0.7) of the frame over 2 s.
/// The video is drawn at half size in the canvas's top-right quarter, so the
/// follower is only right if the clip's transform is applied.
fn follower_project() -> Project {
    let mut p = Project::new(
        "t",
        CanvasConfig {
            width: 640,
            height: 480,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    p.materials.videos.push(VideoMaterial {
        id: "video".into(),
        path: "/nonexistent/video.mp4".into(),
        width: 640,
        height: 480,
        duration: 2_000_000,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    p.materials.images.push(ImageMaterial {
        id: "sticker".into(),
        path: "/nonexistent/sticker.png".into(),
        width: 32,
        height: 32,
    });
    let samples = (0..=60)
        .map(|i| {
            let f = i as f32 / 60.0;
            TrackSample {
                t: i * 33_333,
                x: 0.2 + 0.6 * f,
                y: 0.3 + 0.4 * f,
                w: 0.1,
                h: 0.1,
                a: 0.0,
                c: 1.0,
                f: 0,
            }
        })
        .collect();
    let track = TrackingMaterial::new("video".into(), TrackSettings::default(), samples);
    let track_id = track.id.clone();
    edit::add_track(track).apply(&mut p).unwrap();

    let mut video = Track::new(TrackKind::Video, "V1");
    let mut clip = segment("v", "video", 2_000_000);
    clip.transform.scale = [0.5, 0.5];
    clip.transform.position = [0.5, 0.5];
    video.segments.push(clip);
    let mut overlay = Track::new(TrackKind::Video, "V2");
    let mut sticker = segment("s", "sticker", 2_000_000);
    sticker.render_index = 1;
    sticker.transform.scale = [0.1, 0.1];
    overlay.segments.push(sticker);
    p.tracks.push(video);
    p.tracks.push(overlay);

    // Put the sticker on the object at 0 s, then attach: it must not move.
    let on_object = source_to_canvas(&p, &p.tracks[0].segments[0], 0, [0.2, 0.3]).unwrap();
    p.tracks[1].segments[0].transform.position = on_object;
    edit::attach(&p, "s", &track_id, "v", FollowMode::Position, 0)
        .unwrap()
        .apply(&mut p)
        .unwrap();
    p
}

/// Centre of the pixels that are clearly red, in canvas-normalised units.
fn red_centre(width: u32, height: u32, red: impl Fn(u32, u32) -> bool) -> Option<[f32; 2]> {
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0u32);
    for y in 0..height {
        for x in 0..width {
            if red(x, y) {
                sx += x as f64 + 0.5;
                sy += y as f64 + 0.5;
                n += 1;
            }
        }
    }
    (n > 0).then(|| {
        let (cx, cy) = (sx / n as f64, sy / n as f64);
        [
            (cx / width as f64 * 2.0 - 1.0) as f32,
            (1.0 - cy / height as f64 * 2.0) as f32,
        ]
    })
}

#[test]
fn a_follower_is_drawn_on_the_object_in_preview_and_export() {
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter");
        return;
    };
    let compositor = Compositor::with_config(
        ctx,
        CompositorConfig {
            format: wgpu::TextureFormat::Rgba8Unorm,
            ..Default::default()
        },
    );
    let provider = SolidColorProvider::new()
        .with("video", SolidSource::new([0.5, 0.5, 0.5, 1.0], 640, 480))
        .with("sticker", SolidSource::new([1.0, 0.0, 0.0, 1.0], 32, 32));
    let project = follower_project();
    let sticker = project.segment("s").unwrap().1.clone();

    for time in [0, 500_000, 1_000_000, 1_900_000] {
        let expected = followed_transform(&project, &sticker, time)
            .unwrap()
            .position;
        let on_object = source_to_canvas(&project, &project.tracks[0].segments[0], time, {
            let pose = project.materials.trackings[0].pose_at(time).unwrap();
            [pose.x, pose.y]
        })
        .unwrap();
        assert!(
            (expected[0] - on_object[0]).abs() < 1e-4 && (expected[1] - on_object[1]).abs() < 1e-4,
            "the follower left the object: {expected:?} against {on_object:?}"
        );

        let preview = compositor
            .render(&project, time, (640, 480), &provider)
            .expect("preview");
        let seen = red_centre(640, 480, |x, y| {
            let [r, g, b, _] = preview.pixel(x, y);
            r > 200 && g < 60 && b < 60
        })
        .expect("the sticker is drawn");
        assert!(
            (seen[0] - expected[0]).abs() < 0.01 && (seen[1] - expected[1]).abs() < 0.01,
            "at {time}: drawn at {seen:?}, expected {expected:?}"
        );

        let Ok(export) = compositor.render_nv12(&project, time, (640, 480), &provider) else {
            eprintln!("skipping the export half: no RGBA to NV12 pass on this device");
            continue;
        };
        // Red is the only thing with a high Cr in the frame.
        let (uv, uv_stride) = (export.uv(), export.uv_stride);
        let exported = red_centre(640, 480, |x, y| {
            let i = (y as usize / 2) * uv_stride + (x as usize / 2) * 2;
            uv[i + 1] > 200
        })
        .expect("the sticker is in the export");
        assert!(
            (exported[0] - seen[0]).abs() < 0.01 && (exported[1] - seen[1]).abs() < 0.01,
            "at {time}: export {exported:?} against preview {seen:?}"
        );
    }
}

#[test]
fn an_old_project_without_tracking_opens_and_saves_unchanged() {
    let mut p = Project::new("t", CanvasConfig::default(), 30.0);
    p.tracks.push(Track::new(TrackKind::Video, "V1"));
    let json = serde_json::to_string_pretty(&p).unwrap();
    assert!(!json.contains("trackings") && !json.contains("follows"));
    let back: Project = serde_json::from_str(&json).unwrap();
    assert!(back.materials.trackings.is_empty() && back.materials.follows.is_empty());
    assert_eq!(serde_json::to_string_pretty(&back).unwrap(), json);
}
