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
