//! An opened project bakes what its clips lack in the background
//! (`modules::prepare`): what it finds, in every timeline and inside
//! compound clips, and a run that mixes a compound clip's sound down, or
//! stops when asked.
//!
//! The cache and data roots point into the target directory, so nothing
//! from an earlier run of the app is found and nothing is left behind.

mod support;

use std::path::Path;
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use chukcut_engine::modules::audiofx::commands::audiofx_add;
use chukcut_engine::modules::matting::commands::current_model;
use chukcut_engine::modules::prepare::commands::{
    prepare_missing, prepare_start, prepare_status, prepare_stop, PrepareStatus,
};
use chukcut_engine::modules::project::compositing::CompositingMaterial;
use chukcut_engine::modules::project::document::{
    new_id, AudioMaterial, CanvasConfig, Micros, Project, Segment, Track, TrackKind,
};
use chukcut_engine::modules::sequence::{bounce, build};
use chukcut_engine::state::AppState;

use support::{material_for, segment};

const S: Micros = 1_000_000;

fn isolate() {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("prepare-test");
    ONCE.call_once(|| {
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        chukcut_engine::modules::project::autosave::disable_for_process();
    });
}

/// `segment` with Remove background on, people model, nothing baked.
fn matted(project: &mut Project, mut clip: Segment) -> Segment {
    let mut material = CompositingMaterial::new();
    material.id = new_id();
    material.background = Some(current_model());
    clip.extras.push(material.id.clone());
    project.materials.compositing.push(material);
    clip
}

/// A sine on its own lane, put into a compound clip with a reverb on it:
/// its mix-down is missing. Contents of this instant's own, so no earlier
/// run's mix-down is found. Answers the state and the compound clip's id.
fn with_processed_compound(mut project: Project, audio: &Path) -> (Arc<AppState>, String) {
    let sine = chukcut_engine::modules::media::probe(audio).unwrap();
    project.materials.audios.push(AudioMaterial {
        id: "sine".into(),
        path: audio.to_string_lossy().into_owned(),
        duration: sine.duration,
        sample_rate: 48_000,
        channels: 1,
    });
    let mut lane = Track::new(TrackKind::Audio, "Audio 1");
    let mut clip = segment("sine", 0, S);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    clip.volume = 0.5 + (nanos % 100_000) as f32 / 1e6;
    lane.segments.push(clip);
    project.tracks.push(lane);
    let made = build::create_compound(&project, &["seg-sine-0".to_string()], None).unwrap();
    made.command.apply(&mut project).unwrap();
    let state = AppState::new();
    *state.project.write() = Some(project);
    audiofx_add(&state, made.segment_id.clone(), "reverb".into()).unwrap();
    (state, made.segment_id)
}

fn canvas() -> CanvasConfig {
    CanvasConfig {
        width: 320,
        height: 240,
        background: [0.0, 0.0, 0.0, 1.0],
    }
}

#[test]
fn what_an_opened_project_lacks_is_counted_in_every_sequence() {
    isolate();
    let media = require_media!();
    let mut project = Project::new("prepare", canvas(), 30.0);
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).unwrap());
    // One second matted on the open timeline.
    let main_clip = matted(&mut project, segment("counter", 0, S));
    let mut main = Track::new(TrackKind::Video, "Video 1");
    main.segments.push(main_clip);
    project.tracks.push(main);
    // Half a second matted on a second timeline, which is parked.
    let main_id = project.sequence.id.clone();
    let (new, _) = build::new_timeline(&project, Some("Second".into())).unwrap();
    new.apply(&mut project).unwrap();
    let other = matted(&mut project, segment("counter", 0, S / 2));
    let lane = project.tracks[0].id.clone();
    chukcut_engine::modules::timeline::ops::EditCommand::InsertSegment {
        track_id: lane,
        segment: other,
        index: 0,
    }
    .apply(&mut project)
    .unwrap();
    build::switch(&project, &main_id)
        .unwrap()
        .apply(&mut project)
        .unwrap();
    // A compound clip whose own reverb needs a mix-down.
    let (state, _) = with_processed_compound(project, &media.audio_only);
    let project = state.project.read().clone().unwrap();

    let missing = prepare_missing(&project);
    assert_eq!(missing.sounds, 1);
    // 30 + 15 frames at 30 fps, give or take the frame at each end.
    assert!(
        (44..=47).contains(&missing.frames),
        "{} frames",
        missing.frames
    );
    assert_eq!(
        missing.sentence(),
        format!(
            "Preparing {} frames and the sound of 1 compound clip",
            missing.frames
        )
    );
}

/// Face landmarks a retouch reads and an isolated voice are part of what
/// an opened project lacks — inside a compound clip and on a timeline that
/// is not the open one — and the queues the app runs after an edit find
/// them there too.
#[test]
fn landmarks_and_voices_are_counted_inside_compounds_and_parked_timelines() {
    use chukcut_engine::modules::landmarks::commands::{self as landmarks, RetouchSetting};
    use chukcut_engine::modules::voice::commands::{
        voice_isolation_missing, voice_set_isolation, IsolationSetting,
    };
    use chukcut_engine::modules::voice::isolate::Keep;
    isolate();
    let media = require_media!();
    let mut project = Project::new("faces and voices", canvas(), 30.0);
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).unwrap());
    let sine = chukcut_engine::modules::media::probe(&media.audio_only).unwrap();
    project.materials.audios.push(AudioMaterial {
        id: "sine".into(),
        path: media.audio_only.to_string_lossy().into_owned(),
        duration: sine.duration,
        sample_rate: 48_000,
        channels: 1,
    });
    let mut video = Track::new(TrackKind::Video, "Video 1");
    video.segments.push(segment("counter", 0, S));
    let mut audio = Track::new(TrackKind::Audio, "Audio 1");
    audio.segments.push(segment("sine", 0, S));
    project.tracks.push(video);
    project.tracks.push(audio);
    let state = AppState::new();
    *state.project.write() = Some(project);
    landmarks::landmarks_set_retouch(
        &state,
        "seg-counter-0".into(),
        RetouchSetting::preset("natural"),
    )
    .unwrap();
    voice_set_isolation(
        &state,
        "seg-sine-0".into(),
        Some(IsolationSetting {
            strength: 1.0,
            keep: Keep::Voice,
        }),
    )
    .unwrap();
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().unwrap();
        // The retouched clip goes into a compound clip ...
        let made = build::create_compound(project, &["seg-counter-0".to_string()], None).unwrap();
        made.command.apply(project).unwrap();
        // ... and a second timeline is opened, so all of it is parked.
        let (new, _) = build::new_timeline(project, Some("Second".into())).unwrap();
        new.apply(project).unwrap();
        assert!(project.segment("seg-sine-0").is_none(), "parked");
    }
    let project = state.project.read().clone().unwrap();

    let missing = landmarks::missing(&project);
    assert_eq!(missing.len(), 1, "the clip inside the compound clip");
    assert_eq!(missing[0].0, "seg-counter-0");
    assert_eq!(
        voice_isolation_missing(&state),
        vec!["seg-sine-0".to_string()]
    );
    let status = prepare_missing(&project);
    assert_eq!(status.voices, 1);
    // A second of 30 fps faces, give or take the frame at each end.
    assert!((29..=32).contains(&status.frames), "{status:?}");
    assert_eq!(
        status.sentence(),
        format!("Preparing {} frames and 1 voice", status.frames)
    );
}

/// One preparation runs at a time in a process: a test that starts one
/// would stop another's.
static RUNS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Wait for the current run to end.
fn finished() -> PrepareStatus {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = prepare_status().expect("a run");
        if status.finished || Instant::now() > deadline {
            return status;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn an_opened_project_mixes_its_compound_clips_down_or_stops_when_asked() {
    isolate();
    let media = require_media!();
    let _run = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let (state, outer) =
        with_processed_compound(Project::new("sound", canvas(), 30.0), &media.audio_only);
    let project = state.project.read().clone().unwrap();
    let sequence = project.segment(&outer).unwrap().1.material_id.clone();
    let (mix_down, _) = bounce::source_of(&project, &sequence).unwrap();
    assert!(!mix_down.is_file());

    // Stopped during the wait after opening: nothing is made.
    let first = prepare_start(&state);
    prepare_stop();
    let stopped = finished();
    assert_eq!(stopped.run, first);
    assert!(stopped.finished && stopped.stopped);
    assert_eq!(stopped.sounds_done, 0);
    assert!(!mix_down.is_file());

    // Left alone, the mix-down is made and counted.
    let second = prepare_start(&state);
    assert_ne!(second, first);
    let done = finished();
    assert_eq!(done.run, second);
    assert!(done.finished && !done.stopped, "{done:?}");
    assert_eq!((done.sounds, done.sounds_done), (1, 1), "{done:?}");
    assert!(done.failures.is_empty(), "{:?}", done.failures);
    assert!(mix_down.is_file());
    assert!(!done.busy());

    // Opened again: nothing left to do.
    assert_eq!(prepare_missing(&project).sounds, 0);
}

/// Where the face mesh and HTDemucs are installed: an opened project finds
/// the faces and isolates the voice its parked clips lack, and counts them.
#[test]
fn an_opened_project_finds_faces_and_isolates_voices_it_lacks() {
    use chukcut_engine::modules::landmarks::commands::{self as landmarks, RetouchSetting};
    use chukcut_engine::modules::ml;
    use chukcut_engine::modules::voice::commands::{
        voice_isolation_missing, voice_set_isolation, IsolationSetting,
    };
    use chukcut_engine::modules::voice::isolate::Keep;
    use chukcut_ml_worker::registry;
    isolate();
    let media = require_media!();
    // The models live in the user's ML cache, which the isolated cache
    // root links to.
    let user_ml = std::env::var_os("HOME")
        .map(|h| Path::new(&h).join(".cache/chukcut/ml"))
        .filter(|p| p.is_dir());
    let Some(user_ml) = user_ml else {
        eprintln!("skipping: no ML cache");
        return;
    };
    let link = ml::root();
    if !link.exists() {
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&user_ml, &link).unwrap();
    }
    let present = |id: &str| registry::model_present(&link, registry::model(id).unwrap());
    if ml::worker::binary().is_none()
        || !["facemesh", "yunet", "htdemucs-vocals"]
            .iter()
            .all(|id| present(id))
        || registry::preferred_runtime(&link).is_none()
    {
        eprintln!("skipping: the ML worker, a runtime, the face mesh or HTDemucs is missing");
        return;
    }
    let _run = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let mut project = Project::new("prepare faces and voices", canvas(), 30.0);
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).unwrap());
    // Audio of this instant's own, so no earlier run's isolation is found.
    let sine = chukcut_engine::modules::media::probe(&media.audio_only).unwrap();
    project.materials.audios.push(AudioMaterial {
        id: "sine".into(),
        path: media.audio_only.to_string_lossy().into_owned(),
        duration: sine.duration,
        sample_rate: 48_000,
        channels: 1,
    });
    let mut video = Track::new(TrackKind::Video, "Video 1");
    video.segments.push(segment("counter", 0, S));
    let mut audio = Track::new(TrackKind::Audio, "Audio 1");
    audio.segments.push(segment("sine", 0, S));
    project.tracks.push(video);
    project.tracks.push(audio);
    let state = AppState::new();
    *state.project.write() = Some(project);
    landmarks::landmarks_set_retouch(
        &state,
        "seg-counter-0".into(),
        RetouchSetting::preset("soft"),
    )
    .unwrap();
    // A strength of this run's own: the mix is keyed by it.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    voice_set_isolation(
        &state,
        "seg-sine-0".into(),
        Some(IsolationSetting {
            strength: 0.05 * (1 + nanos % 18) as f32,
            keep: Keep::Background,
        }),
    )
    .unwrap();
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().unwrap();
        let made = build::create_compound(project, &["seg-counter-0".to_string()], None).unwrap();
        made.command.apply(project).unwrap();
        let (new, _) = build::new_timeline(project, Some("Second".into())).unwrap();
        new.apply(project).unwrap();
    }
    let _ = std::fs::remove_dir_all(chukcut_engine::modules::landmarks::track::root());
    let wanted = prepare_missing(&state.project.read().clone().unwrap());
    assert!(wanted.frames > 0 && wanted.voices <= 1, "{wanted:?}");
    let run = prepare_start(&state);
    let deadline = Instant::now() + Duration::from_secs(180);
    let done = loop {
        let status = prepare_status().expect("a run");
        if status.finished || Instant::now() > deadline {
            break status;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(done.run, run);
    assert!(done.finished && !done.stopped, "{done:?}");
    assert!(done.failures.is_empty(), "{:?}", done.failures);
    assert_eq!(done.frames_done, done.frames, "{done:?}");
    assert_eq!(done.voices_done, done.voices, "{done:?}");
    let project = state.project.read().clone().unwrap();
    assert!(landmarks::missing(&project).is_empty());
    assert!(voice_isolation_missing(&state).is_empty());
}
