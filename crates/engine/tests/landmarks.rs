//! Face landmarks, retouch and face-follow, end to end, where the ML worker
//! and the face mesh are installed.
//!
//! The face is a public-domain picture: NASA's official portrait of
//! astronaut Terrence W. Wilcutt (jsc2003e41874, a work of the US
//! government), fetched once into `target/test-media` and checked against
//! its SHA-256. The clip pans across it, so the face moves 40 px a second
//! to the left of a 720×900 frame — a motion the landmarks must follow.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::analysis::commands::analysis_wait;
use chukcut_engine::modules::landmarks::commands::{
    landmarks_analyse, landmarks_coverage, landmarks_faces, landmarks_follow_face,
    landmarks_set_retouch, FollowFace, RetouchSetting,
};
use chukcut_engine::modules::landmarks::shape::{key, Anchor};
use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::project::document::{
    CanvasConfig, ImageMaterial, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_engine::modules::render::{Compositor, SourceProvider};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::modules::tracking::FollowMode;
use chukcut_engine::state::AppState;

const W: u32 = 720;
const H: u32 = 900;
const PORTRAIT_URL: &str =
    "https://images-assets.nasa.gov/image/jsc2003e41874/jsc2003e41874~medium.jpg";
const PORTRAIT_SHA256: &str = "b5321a6f012748377fc4a1d25854427b2a46bbb95021cce9c697ae62d2d656fe";

fn media_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/faces-v1");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256(path: &Path) -> Option<String> {
    use sha2::{Digest as _, Sha256};
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// The portrait, downloaded once (neutral User-Agent, pinned digest), and
/// the panning clip made from it. `None` without network or ffmpeg.
fn fixture() -> Option<(PathBuf, PathBuf)> {
    static MAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _making = MAKING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = media_dir();
    let portrait = dir.join("portrait.jpg");
    if sha256(&portrait).as_deref() != Some(PORTRAIT_SHA256) {
        let part = dir.join("portrait.part");
        let ok = Command::new("curl")
            .args(["-sfL", "-A", "chukcut/0.1", "-o"])
            .arg(&part)
            .arg(PORTRAIT_URL)
            .status()
            .is_ok_and(|s| s.success());
        if !ok || sha256(&part).as_deref() != Some(PORTRAIT_SHA256) {
            let _ = std::fs::remove_file(&part);
            return None;
        }
        std::fs::rename(&part, &portrait).ok()?;
    }
    let clip = dir.join("pan.mp4");
    if !clip.exists() {
        let tmp = dir.join("pan.tmp.mp4");
        let ok = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y", "-loop", "1", "-i"])
            .arg(&portrait)
            .args([
                "-vf",
                "crop=720:900:x='150+40*t':y='60+20*t',format=yuv420p",
                "-t",
                "2",
                "-r",
                "30",
                "-c:v",
                "libx264",
                "-crf",
                "14",
            ])
            .arg(&tmp)
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return None;
        }
        std::fs::rename(&tmp, &clip).ok()?;
    }
    // A plain sticker for face-follow: a white square.
    let sticker = dir.join("square.png");
    if !sticker.exists() {
        image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]))
            .save(&sticker)
            .ok()?;
    }
    Some((clip, sticker))
}

fn ready() -> bool {
    use chukcut_engine::modules::ml;
    use chukcut_ml_worker::registry;
    let root = ml::root();
    ml::worker::binary().is_some()
        && registry::model_present(&root, registry::model("facemesh").unwrap())
        && registry::model_present(&root, registry::model("yunet").unwrap())
        && (registry::preferred_runtime(&root).is_some()
            || std::env::var_os("CHUKCUT_ORT_DYLIB").is_some())
}

fn state(clip: &Path, sticker: &Path) -> Arc<AppState> {
    let mut project = Project::new("faces", CanvasConfig::default(), 30.0);
    project.canvas.width = W;
    project.canvas.height = H;
    project.materials.videos.push(VideoMaterial {
        id: "v".into(),
        path: clip.to_string_lossy().into(),
        width: W,
        height: H,
        duration: 2_000_000,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    project.materials.images.push(ImageMaterial {
        id: "s".into(),
        path: sticker.to_string_lossy().into(),
        width: 64,
        height: 64,
    });
    let segment = |id: &str, material: &str| Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(0, 2_000_000),
        source_range: TimeRange::new(0, 2_000_000),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    };
    let mut video = Track::new(TrackKind::Video, "V1");
    video.segments.push(segment("face", "v"));
    let mut overlay = Track::new(TrackKind::Video, "V2");
    overlay.segments.push(segment("sticker", "s"));
    project.tracks.push(video);
    project.tracks.push(overlay);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

fn render(state: &AppState, t: Micros) -> Option<Vec<u8>> {
    let ctx = chukcut_engine::modules::gpu::render_context()?;
    let mut project = state.project.read().clone().unwrap();
    // The face clip alone.
    project.tracks.truncate(1);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    Some(
        Compositor::new(ctx)
            .render(&project, t, (W, H), sources.as_ref())
            .expect("render")
            .data,
    )
}

/// Mean absolute difference of two RGBA frames over a pixel rectangle.
fn diff(a: &[u8], b: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> f64 {
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * W + x) * 4) as usize;
            for c in 0..3 {
                sum += (a[i + c] as i32 - b[i + c] as i32).unsigned_abs() as u64;
            }
            n += 3;
        }
    }
    sum as f64 / n.max(1) as f64
}

#[test]
fn faces_are_found_followed_retouched_and_followed_by_a_sticker() {
    if !ready() {
        eprintln!("skipping: the ML worker, a runtime or the face mesh is not installed");
        return;
    }
    let Some((clip, sticker)) = fixture() else {
        eprintln!(
            "skipping: the public-domain portrait could not be fetched, or ffmpeg is missing"
        );
        return;
    };
    // Keep the landmark track out of the user's cache; the models stay.
    let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("landmarks-cache");
    let _ = std::fs::remove_dir_all(cache.join("chukcut/landmarks"));
    std::fs::create_dir_all(cache.join("chukcut")).unwrap();
    let link = cache.join("chukcut/ml");
    if !link.exists() {
        std::os::unix::fs::symlink(chukcut_engine::modules::ml::root(), &link).unwrap();
    }
    std::env::set_var("XDG_CACHE_HOME", &cache);

    let state = state(&clip, &sticker);

    // 1. Landmarks: every frame, one face, following the pan.
    let started = std::time::Instant::now();
    let job = landmarks_analyse(&state, "face".into(), None).unwrap();
    let said = analysis_wait(job).unwrap();
    eprintln!("{said} in {:.2} s", started.elapsed().as_secs_f32());
    let coverage = landmarks_coverage(&state, "face".into()).unwrap();
    assert!(coverage.done(), "{coverage:?}");
    let at = |t: Micros| {
        let faces = landmarks_faces(&state, "face".into(), t, false).unwrap();
        assert_eq!(faces.len(), 1, "one face at {t}");
        faces.into_iter().next().unwrap()
    };
    let (first, later) = (at(0), at(1_500_000));
    let k = &first.key_points;
    assert!(k[key::R_EYE_OUT][0] < k[key::L_EYE_OUT][0], "eyes in order");
    assert!(k[key::FOREHEAD][1] < k[key::NOSE][1] && k[key::NOSE][1] < k[key::CHIN][1]);
    let moved = (later.bbox[0] - first.bbox[0]) * W as f32;
    assert!(
        (moved + 60.0).abs() < 8.0,
        "the face moved {moved} px, the pan 60"
    );

    // 2. Retouch changes the face and leaves the rest alone.
    if let Some(plain) = render(&state, 1_000_000) {
        landmarks_set_retouch(&state, "face".into(), RetouchSetting::preset("soft")).unwrap();
        let retouched = render(&state, 1_000_000).unwrap();
        let face = |b: [f32; 4]| {
            (
                (b[0] * W as f32) as u32,
                (b[1] * H as f32) as u32,
                ((b[0] + b[2]) * W as f32) as u32,
                ((b[1] + b[3]) * H as f32) as u32,
            )
        };
        let (x0, y0, x1, y1) = face(at(1_000_000).bbox);
        let inside = diff(&plain, &retouched, x0, y0, x1, y1);
        // The top right corner is backdrop, far from the face.
        let outside = diff(&plain, &retouched, 560, 10, 710, 120);
        eprintln!("retouch: {inside:.2} inside the face, {outside:.3} outside");
        assert!(inside > 0.5, "the face changed: {inside}");
        assert!(outside < 0.05, "the backdrop stayed: {outside}");
        timeline_undo(&state).unwrap();
        assert_eq!(
            render(&state, 1_000_000).unwrap(),
            plain,
            "undo restores it"
        );
    } else {
        eprintln!("no GPU: the retouch half is skipped");
    }

    // 3. A sticker follows the face, as one undo step.
    let never = AtomicBool::new(false);
    landmarks_follow_face(
        &state,
        FollowFace {
            overlay_id: "sticker".into(),
            target_segment_id: "face".into(),
            anchor: Anchor::Forehead,
            mode: FollowMode::Position,
            face: 0,
            at: 0,
        },
        &never,
    )
    .unwrap();
    let position = |t: Micros| {
        let project = state.project.read().clone().unwrap();
        let (_, s) = project.segment("sticker").unwrap();
        chukcut_engine::modules::tracking::follow::followed_transform(&project, s, t)
            .expect("the sticker follows")
            .position
    };
    let (p0, p1) = (position(0), position(1_500_000));
    // Canvas units: ±1 at the edges, so 60 px of a 720 frame is 0.167.
    let dx = p1[0] - p0[0];
    assert!(
        (dx + 0.167).abs() < 0.03,
        "the sticker moved {dx}, the face -0.167"
    );
    assert!(
        p0[0].abs() < 1e-3 && p0[1].abs() < 1e-3,
        "attaching moves nothing: {p0:?}"
    );
    timeline_undo(&state).unwrap();
    let project = state.project.read().clone().unwrap();
    let (_, s) = project.segment("sticker").unwrap();
    assert!(project.materials.follow_of(s).is_none());
}
