//! Body landmarks, following a body part, and auto reframe on a body when
//! no face shows — end to end, where the ML worker and the models are
//! installed.
//!
//! The person is a public-domain picture: NASA's full-length portrait of
//! Mercury astronaut L. Gordon Cooper in his spacesuit (S63-01755, a work
//! of the US government), fetched once into `target/test-media` and checked
//! against its SHA-256. The clip slides him across a 1280×720 frame at
//! 150 px a second — a motion the keypoints must follow. A second clip hides
//! his head under a grey box: no face for the face detector, a body for the
//! person detector.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::analysis::commands::{
    analysis_reframe, analysis_wait, Reframe, SubjectCue,
};
use chukcut_engine::modules::body::commands::{
    body_analyse, body_coverage, body_follow, body_people, FollowBody, PersonInfo,
};
use chukcut_engine::modules::body::shape::BodyPart;
use chukcut_engine::modules::project::document::{
    AnimatableProperty, CanvasConfig, ImageMaterial, Micros, Project, Segment, TimeRange, Track,
    TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::modules::tracking::FollowMode;
use chukcut_engine::state::AppState;

const W: u32 = 1280;
const H: u32 = 720;
/// The person's left edge at `t` seconds, and how fast it moves.
const START_X: f32 = 200.0;
const SPEED: f32 = 150.0;
const PORTRAIT_URL: &str = "https://images-assets.nasa.gov/image/S63-01755/S63-01755~medium.jpg";
const PORTRAIT_SHA256: &str = "2d956cdd737294876f6605ed579f08bd8ce00271fcae43051e5050b68151ded9";

fn media_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/body-v1");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256(path: &Path) -> Option<String> {
    use sha2::{Digest as _, Sha256};
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(&bytes)))
}

/// A 2 s clip of the portrait, 640 px tall, sliding right over grey.
/// `hide_head` covers his head first.
fn clip(dir: &Path, portrait: &Path, name: &str, hide_head: bool) -> Option<PathBuf> {
    let path = dir.join(name);
    if path.exists() {
        return Some(path);
    }
    let head = if hide_head {
        // The helmet-less head of the 944×1280 picture, and some margin.
        "drawbox=x=330:y=0:w=290:h=300:color=0x808080:t=fill,"
    } else {
        ""
    };
    let filter = format!(
        "[0]{head}scale=-2:640,format=rgba[p];\
         [1][p]overlay=x='{START_X}+{SPEED}*t':y=40:shortest=1,format=yuv420p"
    );
    let tmp = dir.join(format!("{name}.tmp.mp4"));
    let ok = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-loop", "1", "-i"])
        .arg(portrait)
        .args([
            "-f",
            "lavfi",
            "-i",
            &format!("color=c=0x606060:size={W}x{H}:rate=30:duration=2"),
            "-filter_complex",
            &filter,
            "-t",
            "2",
            "-r",
            "30",
            "-c:v",
            "libx264",
            "-crf",
            "16",
        ])
        .arg(&tmp)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return None;
    }
    std::fs::rename(&tmp, &path).ok()?;
    Some(path)
}

/// The portrait (downloaded once, neutral User-Agent, pinned digest), the
/// two clips and a sticker. `None` without network or ffmpeg.
fn fixture() -> Option<(PathBuf, PathBuf, PathBuf)> {
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
    let walk = clip(&dir, &portrait, "walk.mp4", false)?;
    let headless = clip(&dir, &portrait, "headless.mp4", true)?;
    let sticker = dir.join("square.png");
    if !sticker.exists() {
        image::RgbaImage::from_pixel(64, 64, image::Rgba([255, 255, 255, 255]))
            .save(&sticker)
            .ok()?;
    }
    Some((walk, headless, sticker))
}

fn ready(with_faces: bool) -> bool {
    use chukcut_engine::modules::ml;
    use chukcut_ml_worker::registry;
    let root = ml::root();
    let present = |id: &str| registry::model_present(&root, registry::model(id).unwrap());
    ml::worker::binary().is_some()
        && present("rtmpose-m")
        && present("yolox-tiny-human")
        && (!with_faces || present("yunet"))
        && (registry::preferred_runtime(&root).is_some()
            || std::env::var_os("CHUKCUT_ORT_DYLIB").is_some())
}

/// Keep the body track out of the user's cache; the models stay.
fn private_cache(name: &str) {
    let cache = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(cache.join("chukcut/landmarks"));
    std::fs::create_dir_all(cache.join("chukcut")).unwrap();
    let link = cache.join("chukcut/ml");
    if !link.exists() {
        std::os::unix::fs::symlink(chukcut_engine::modules::ml::root(), &link).unwrap();
    }
    std::env::set_var("XDG_CACHE_HOME", &cache);
}

fn state(clip: &Path, sticker: &Path) -> Arc<AppState> {
    let mut project = Project::new("body", CanvasConfig::default(), 30.0);
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
    video.segments.push(segment("person", "v"));
    let mut overlay = Track::new(TrackKind::Video, "V2");
    overlay.segments.push(segment("sticker", "s"));
    project.tracks.push(video);
    project.tracks.push(overlay);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

fn point(person: &PersonInfo, name: &str) -> [f32; 3] {
    let k = person
        .keypoints
        .iter()
        .find(|k| k.name == name)
        .unwrap_or_else(|| panic!("{name}"));
    [k.x, k.y, k.confidence]
}

#[test]
fn a_body_is_found_followed_and_followed_by_a_sticker() {
    if !ready(false) {
        eprintln!("skipping: the ML worker, a runtime or the body models are not installed");
        return;
    }
    let Some((walk, _, sticker)) = fixture() else {
        eprintln!(
            "skipping: the public-domain portrait could not be fetched, or ffmpeg is missing"
        );
        return;
    };
    private_cache("body-cache");
    let state = state(&walk, &sticker);

    // 1. Keypoints: every frame, one person, upright, facing the camera,
    // sliding right.
    let started = std::time::Instant::now();
    let job = body_analyse(&state, "person".into(), None).unwrap();
    let said = analysis_wait(job).unwrap();
    eprintln!("{said} in {:.2} s", started.elapsed().as_secs_f32());
    let coverage = body_coverage(&state, "person".into()).unwrap();
    assert!(coverage.done(), "{coverage:?}");
    let at = |t: Micros| {
        let people = body_people(&state, "person".into(), t).unwrap();
        assert_eq!(people.len(), 1, "one person at {t}: {people:?}");
        people.into_iter().next().unwrap()
    };
    let (first, later) = (at(0), at(1_500_000));
    assert_eq!(first.id, 0);
    for name in [
        "nose",
        "left_shoulder",
        "right_shoulder",
        "left_hip",
        "right_hip",
    ] {
        assert!(point(&first, name)[2] > 0.3, "{name} seen: {first:?}");
    }
    let (nose, hip, ankle) = (
        point(&first, "nose"),
        point(&first, "left_hip"),
        point(&first, "left_ankle"),
    );
    assert!(nose[1] < hip[1] && hip[1] < ankle[1], "upright: {first:?}");
    // Facing the camera: his left shoulder is on the picture's right.
    assert!(point(&first, "left_shoulder")[0] > point(&first, "right_shoulder")[0]);
    let hips = |p: &PersonInfo| (point(p, "left_hip")[0] + point(p, "right_hip")[0]) / 2.0;
    let moved = (hips(&later) - hips(&first)) * W as f32;
    eprintln!("the hips moved {moved:.1} px in 1.5 s");
    assert!(
        (moved - SPEED * 1.5).abs() < 12.0,
        "the hips moved {moved} px, the clip {}",
        SPEED * 1.5
    );

    // 2. A sticker follows his left hand (theirs), as one undo step.
    let never = AtomicBool::new(false);
    body_follow(
        &state,
        FollowBody {
            overlay_id: "sticker".into(),
            target_segment_id: "person".into(),
            part: BodyPart::LeftHand,
            mode: FollowMode::Position,
            person: 0,
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
    // Canvas units: ±1 at the edges, so 225 px of a 1280 frame is 0.352.
    let dx = p1[0] - p0[0];
    let expected = SPEED * 1.5 / W as f32 * 2.0;
    assert!(
        (dx - expected).abs() < 0.04,
        "the sticker moved {dx}, the hand {expected}"
    );
    assert!(
        p0[0].abs() < 1e-3 && p0[1].abs() < 1e-3,
        "attaching moves nothing: {p0:?}"
    );
    // Another person, who is not there, is refused in words.
    let Err(error) = body_follow(
        &state,
        FollowBody {
            overlay_id: "sticker".into(),
            target_segment_id: "person".into(),
            part: BodyPart::Hips,
            mode: FollowMode::Position,
            person: 2,
            at: 0,
        },
        &never,
    ) else {
        panic!("a person who is not there was followed");
    };
    assert!(error.contains("person 3"), "{error}");
    timeline_undo(&state).unwrap();
    let project = state.project.read().clone().unwrap();
    let (_, s) = project.segment("sticker").unwrap();
    assert!(project.materials.follow_of(s).is_none());
}

#[test]
fn auto_reframe_holds_a_body_when_no_face_shows() {
    if !ready(true) {
        eprintln!("skipping: the ML worker, a runtime, YuNet or the body models are not installed");
        return;
    }
    let Some((_, headless, sticker)) = fixture() else {
        eprintln!(
            "skipping: the public-domain portrait could not be fetched, or ffmpeg is missing"
        );
        return;
    };
    private_cache("body-reframe-cache");
    let state = state(&headless, &sticker);
    let job = analysis_reframe(
        &state,
        Reframe {
            segment_ids: vec!["person".into()],
            ratio: Some((9, 16)),
            subject: SubjectCue::Auto,
        },
        None,
    )
    .unwrap();
    let message = analysis_wait(job).unwrap();
    eprintln!("{message}");
    assert!(message.contains("people in"), "{message}");
    let project = state.project.read().clone().unwrap();
    let clip = project.segment("person").unwrap().1.clone();
    let qw = 720.0 * clip.transform.scale[0];
    let track = clip
        .keyframes
        .iter()
        .find(|k| k.property == AnimatableProperty::PositionX)
        .expect("the window moves");
    let window = 720.0 / qw;
    // His middle: the picture is 640 px tall, so about 472 wide.
    for t in [500_000i64, 1_000_000, 1_500_000] {
        let u = 0.5 - track.sample(t).unwrap() * 360.0 / qw;
        let subject = (START_X + 236.0 + SPEED * t as f32 / 1e6) / W as f32;
        assert!(
            (u - subject).abs() < window * 0.35,
            "at {t}: window at {u}, him at {subject}"
        );
    }
}
