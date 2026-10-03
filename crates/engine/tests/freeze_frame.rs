//! Freeze frame end to end on a real file: the decoded still is the frame the
//! playhead was on, and the edit built from it is one exact undo step.
//!
//! The pure half (`timeline::freeze::freeze_frame_edit`) has its unit tests
//! next to it; this file covers the decode, which needs media.

mod support;

use chukcut_engine::modules::project::Severity;
use chukcut_engine::modules::timeline::freeze::{extract_frame, freeze_frame_edit, freeze_source};
use chukcut_engine::modules::timeline::History;
use support::{counter_frame_time, read_counter_rgba, COUNTER_HEIGHT, COUNTER_WIDTH};

#[test]
fn the_still_is_the_frame_under_the_playhead() {
    let media = match support::media() {
        Ok(media) => media,
        Err(reason) => {
            eprintln!("{}: skipped, {reason}", support::test_name());
            return;
        }
    };
    let material = support::material_for("counter", &media.counter).unwrap();
    let duration = counter_frame_time(120);
    let mut project = support::single_clip_project((320, 240), 30.0, material, duration);
    let clip = project.tracks[0].segments[0].id.clone();

    // Frame 45, sampled half a frame in as the preview does.
    let at = counter_frame_time(45) + counter_frame_time(1) / 2;
    let source = freeze_source(&project, &clip, at).unwrap();
    let output = media.dir.join("freeze_frame_45.png");
    let image = extract_frame(&source, &output).unwrap();
    assert_eq!((image.width, image.height), (COUNTER_WIDTH, COUNTER_HEIGHT));

    let png = image::open(&output).unwrap().to_rgba8();
    assert_eq!(
        read_counter_rgba(png.as_raw(), png.width(), png.height()),
        Some(45)
    );

    let before = serde_json::to_string(&project).unwrap();
    let command = freeze_frame_edit(&project, &clip, at, 3_000_000, image).unwrap();
    let mut history = History::new();
    history.apply(&mut project, command).unwrap();
    let lane = &project.tracks[0].segments;
    assert_eq!(lane.len(), 3);
    assert_eq!(lane[1].target_range.start, at);
    assert_eq!(lane[2].target_range.start, at + 3_000_000);
    assert!(project
        .validate()
        .iter()
        .all(|issue| issue.severity != Severity::Error));

    history.undo(&mut project).unwrap();
    assert_eq!(serde_json::to_string(&project).unwrap(), before);
}
