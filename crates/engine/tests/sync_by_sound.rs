//! Sync by sound end to end (`analysis_sync_audio`, after FilmCraft's
//! Synchronize): a "camera" file and a "recorder" file that started 3.25 s
//! into the same take are placed anywhere on the timeline; one command
//! moves the recorder clip so the two play the moment together, and one
//! undo puts it back.
//!
//! The fixtures are white noise cut with ffmpeg: noise matches only at the
//! true offset, like a voice, and needs no media in git.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::analysis::commands::analysis_sync_audio;
use chukcut_engine::modules::media;
use chukcut_engine::modules::project::commands::import_material;
use chukcut_engine::modules::project::document::{
    CanvasConfig, Project, Segment, TimeRange, Track, TrackKind,
};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::state::AppState;

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .is_ok_and(|s| s.success())
}

fn import(project: &mut Project, file: &Path) -> String {
    let info = media::probe(file).expect("probe the fixture");
    import_material(project, &file.to_string_lossy(), "fixture", &info)
        .expect("import the fixture")
        .id
}

fn clip(material: &str, start: i64, duration: i64) -> Segment {
    Segment {
        id: chukcut_engine::modules::project::document::new_id(),
        material_id: material.into(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(0, duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Default::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

#[test]
fn the_recorder_moves_to_where_the_camera_heard_the_same_moment() {
    let dir = std::env::temp_dir().join(format!("chukcut-sync-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (take, camera, recorder) = (
        dir.join("take.wav"),
        dir.join("camera.wav"),
        dir.join("recorder.wav"),
    );
    let made = ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "anoisesrc=d=20:c=white:r=48000:a=0.3:s=11",
        take.to_str().unwrap(),
    ]) && ffmpeg(&[
        "-i",
        take.to_str().unwrap(),
        "-t",
        "15",
        camera.to_str().unwrap(),
    ]) && ffmpeg(&[
        "-ss",
        "3.25",
        "-i",
        take.to_str().unwrap(),
        "-t",
        "8",
        "-af",
        "volume=0.4",
        recorder.to_str().unwrap(),
    ]);
    if !made {
        eprintln!("skipping: ffmpeg could not make the fixtures");
        return;
    }

    let mut project = Project::new("sync", CanvasConfig::default(), 30.0);
    let cam = import(&mut project, &camera);
    let rec = import(&mut project, &recorder);
    let mut a1 = Track::new(TrackKind::Audio, "A1");
    let camera_clip = clip(&cam, 1_000_000, 15_000_000);
    let camera_id = camera_clip.id.clone();
    a1.segments.push(camera_clip);
    let mut a2 = Track::new(TrackKind::Audio, "A2");
    let recorder_clip = clip(&rec, 20_000_000, 8_000_000);
    let recorder_id = recorder_clip.id.clone();
    a2.segments.push(recorder_clip);
    project.tracks = vec![a1, a2];
    let state = AppState::new();
    *state.project.write() = Some(project);

    let synced = analysis_sync_audio(
        &state,
        camera_id.clone(),
        vec![recorder_id.clone()],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(synced.len(), 1);
    assert!(synced[0].confidence >= 8.0, "{synced:?}");
    let start = |state: &std::sync::Arc<AppState>| {
        state
            .with_project(|p| p.segment(&recorder_id).unwrap().1.target_range.start)
            .unwrap()
    };
    // The camera clip starts at 1 s and the recorder heard from 3.25 s into
    // the take: 4.25 s, to within a sample of the 8 kHz analysis.
    assert!(
        (start(&state) - 4_250_000).abs() <= 250,
        "{}",
        start(&state)
    );

    timeline_undo(&state).unwrap();
    assert_eq!(start(&state), 20_000_000);

    // A clip cannot be synced with something it shares nothing with.
    let err = analysis_sync_audio(&state, recorder_id.clone(), vec![], &AtomicBool::new(false))
        .unwrap_err();
    assert!(err.contains("choose the clips"));
    let _ = std::fs::remove_dir_all(&dir);
}
