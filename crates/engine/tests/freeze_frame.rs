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

#[test]
fn a_still_gets_a_name_the_library_can_show() {
    use chukcut_engine::modules::timeline::freeze::{freeze_output_path_for, FreezeSource};
    let path = freeze_output_path_for(&FreezeSource {
        path: "/footage/beach take.mp4".into(),
        source_time: 62_150_000,
    });
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("beach take frame 1m02.150s "),
        "named {name}"
    );
    assert!(name.ends_with(".png"));
    let again = freeze_output_path_for(&FreezeSource {
        path: "/footage/beach take.mp4".into(),
        source_time: 62_150_000,
    });
    assert_ne!(path, again, "two freezes of one frame are two files");
}

#[test]
fn unused_stills_are_deleted_and_reachable_ones_kept() {
    use chukcut_engine::modules::project::document::ImageMaterial;
    use chukcut_engine::modules::timeline::freeze::{note_created, note_saved, sweep_unused};

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("freeze_sweep");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = |name: &str| {
        let path = dir.join(name);
        std::fs::write(&path, b"png").unwrap();
        note_created(&path);
        path
    };
    let in_document = file("in document.png");
    let in_saved = file("in saved.png");
    let orphan = file("orphan.png");

    let mut project =
        chukcut_engine::modules::project::document::Project::new("sweep", Default::default(), 30.0);
    project.materials.images.push(ImageMaterial {
        id: "still".into(),
        path: in_document.to_string_lossy().into_owned(),
        width: 1,
        height: 1,
    });
    // A project saved earlier in the session that still names the second.
    let saved = dir.join("earlier.chukcut");
    std::fs::write(
        &saved,
        format!(
            "{{\"path\": {}}}",
            serde_json::to_string(&in_saved.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    note_saved(&saved);

    sweep_unused(Some(&project), None);
    assert!(in_document.exists(), "the open document uses it");
    assert!(in_saved.exists(), "a saved project uses it");
    assert!(!orphan.exists(), "nothing reaches it");

    // On close the document is gone: only the saved file keeps its still.
    sweep_unused(None, None);
    assert!(!in_document.exists());
    assert!(in_saved.exists());
}
