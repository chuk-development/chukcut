//! Templates end to end, on real files: a project made from a built-in with
//! generated clips in its slots, the time arithmetic of each fill against
//! the files' real lengths, slot order, missing media, replacing a slot as
//! one undo step, saving a project as a template and making a project from
//! that, and a preview tile when there is a GPU.
//!
//! The data and cache directories point into the target directory, so the
//! test never writes the user's placeholders, music or templates.

mod support;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use chukcut_engine::modules::project::document::{Severity, TimeRange};
use chukcut_engine::modules::template::commands::{self as templates, SaveRequest};
use chukcut_engine::modules::template::slot;
use chukcut_engine::modules::timeline::commands as timeline;
use chukcut_engine::state::AppState;

/// Point the engine's data and cache roots at the target directory, once.
fn isolate() -> PathBuf {
    static ONCE: Once = Once::new();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("templates-test");
    ONCE.call_once(|| {
        std::fs::create_dir_all(&root).unwrap();
        // Set before any thread of this test binary reads them; every test
        // calls `isolate` first.
        std::env::set_var("XDG_DATA_HOME", root.join("data"));
        std::env::set_var("XDG_CACHE_HOME", root.join("cache"));
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        chukcut_engine::modules::project::autosave::disable_for_process();
    });
    root
}

fn path(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn a_built_in_filled_with_real_clips_has_the_right_times_order_and_shapes() {
    isolate();
    let Ok(media) = support::media() else {
        eprintln!("skipped: no ffmpeg");
        return;
    };
    // Quick Cuts: six 1 s slots on a 9:16 canvas. Three files, in order.
    let files = [
        path(&media.counter),
        path(&media.solid_red_landscape),
        path(&media.solid_green_portrait),
    ];
    let applied = templates::template_build_project("quick-cuts", &files, None).unwrap();
    let project = &applied.project;
    assert_eq!(applied.filled.len(), 3);
    assert_eq!(applied.empty, vec![4, 5, 6]);

    let slots = slot::slots(project);
    assert_eq!(slots.len(), 6);
    for (n, file) in files.iter().enumerate() {
        // Slot n holds file n: the order is the slots', not the files'.
        assert_eq!(slots[n].index, n as u32 + 1);
        assert_eq!(slots[n].media_path.as_deref(), Some(file.as_str()));
        assert!(slots[n].filled);
    }
    for empty in &slots[3..] {
        assert!(!empty.filled);
        assert!(empty.media_path.is_none());
    }

    // The 4 s counter in a 1 s slot: trimmed from its start, full speed,
    // landscape cropped to the slot's portrait shape.
    let (_, first) = project.segment(&slots[0].segment_id).unwrap();
    assert_eq!(first.source_range, TimeRange::new(0, slots[0].duration));
    assert_eq!(first.speed, 1.0);
    let crop = first.crop.expect("4:3 into 9:16 is cropped");
    let kept = crop.right - crop.left;
    assert!(
        (kept - (9.0 / 16.0) / (4.0 / 3.0)).abs() < 1e-3,
        "kept {kept} of the width"
    );
    // The slot did not move or change length.
    assert_eq!(first.target_range.start, 0);

    // The placeholders of the empty slots and the music are on disk.
    for empty in &slots[3..] {
        let (_, segment) = project.segment(&empty.segment_id).unwrap();
        let image = project.materials.image(&segment.material_id).unwrap();
        assert!(Path::new(&image.path).is_file(), "{}", image.path);
    }
    let music = &project.materials.audios[0];
    let probed = chukcut_engine::modules::media::probe(&music.path).unwrap();
    assert!(probed.has_audio);
    assert!(probed.duration >= project.duration() - 1_000);

    let errors: Vec<_> = project
        .validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_short_clip_is_slowed_to_fill_a_long_slot() {
    isolate();
    let Ok(media) = support::media() else {
        eprintln!("skipped: no ffmpeg");
        return;
    };
    // Talking Points: one 9 s video slot; the counter is 4 s.
    let applied =
        templates::template_build_project("talking-points", &[path(&media.counter)], None).unwrap();
    let filled = &applied.filled[0];
    let speed = filled.slowed_to.expect("slowed");
    let (_, segment) = applied.project.segment(&filled.segment_id).unwrap();
    assert_eq!(segment.target_range.duration, 9_000_000);
    let file_length = applied.project.materials.videos[0].duration;
    assert!(segment.source_range.duration <= file_length);
    assert!(file_length - segment.source_range.duration < 34_000);
    assert!((speed as f64 - file_length as f64 / 9e6).abs() < 1e-3);
}

#[test]
fn missing_and_unsuitable_media_is_refused_by_name() {
    isolate();
    let Ok(media) = support::media() else {
        eprintln!("skipped: no ffmpeg");
        return;
    };
    let error = templates::template_build_project(
        "quick-cuts",
        &[path(&media.counter), "/nowhere/gone.mp4".into()],
        None,
    )
    .unwrap_err();
    assert!(error.contains("/nowhere/gone.mp4"), "{error}");

    // Sound only cannot fill a picture slot.
    let error = templates::template_build_project("quick-cuts", &[path(&media.audio_only)], None)
        .unwrap_err();
    assert!(error.contains("sound only"), "{error}");

    // Photo Dump takes stills only; a video is refused for its slot.
    let error =
        templates::template_build_project("photo-dump", &[path(&media.counter)], None).unwrap_err();
    assert!(error.contains("slot 1"), "{error}");
    assert!(error.contains("photo"), "{error}");

    // More files than slots.
    let many: Vec<String> = (0..3).map(|_| path(&media.counter)).collect();
    let error = templates::template_build_project("talking-points", &many, None).unwrap_err();
    assert!(error.contains("1 slot"), "{error}");
}

#[test]
fn replacing_a_slot_is_one_undo_step_and_keeps_the_slot() {
    isolate();
    let Ok(media) = support::media() else {
        eprintln!("skipped: no ffmpeg");
        return;
    };
    let state: Arc<AppState> = AppState::new();
    templates::template_new_project(&state, "travel-diary", &[], Some("Trip".into())).unwrap();
    let before = state.project.read().clone().unwrap();
    assert_eq!(before.name, "Trip");
    let second = templates::template_slots(&state).unwrap()[1].clone();
    assert!(!second.filled);

    let replaced = templates::template_replace_media(
        &state,
        &second.segment_id,
        &path(&media.counter),
        Some(500_000),
    )
    .unwrap();
    assert!(replaced.slowed_to.is_none());
    let slots = templates::template_slots(&state).unwrap();
    assert!(slots[1].filled);
    assert_eq!(slots[1].index, 2);
    let after = state.project.read().clone().unwrap();
    let (_, segment) = after.segment(&second.segment_id).unwrap();
    // The chosen start is kept; the slot keeps its place and length.
    assert_eq!(segment.source_range.start, 500_000);
    assert_eq!(
        segment.target_range,
        second_range(&before, &second.segment_id)
    );
    // Its look survived the swap.
    assert!(after.materials.color_adjust_of(segment).is_some());

    timeline::timeline_undo(&state).unwrap();
    let undone = state.project.read().clone().unwrap();
    let slots = templates::template_slots(&state).unwrap();
    assert!(!slots[1].filled);
    let (_, segment) = undone.segment(&second.segment_id).unwrap();
    assert!(undone.materials.image(&segment.material_id).is_some());
}

fn second_range(
    project: &chukcut_engine::modules::project::document::Project,
    id: &str,
) -> TimeRange {
    project.segment(id).unwrap().1.target_range
}

#[test]
fn a_project_saved_as_a_template_makes_projects_of_its_own() {
    let root = isolate();
    let Ok(media) = support::media() else {
        eprintln!("skipped: no ffmpeg");
        return;
    };
    // A project of our own: Memories, filled, then two of its clips chosen
    // as the new template's slots, the last one first.
    let state = AppState::new();
    let files = [path(&media.counter), path(&media.solid_red_landscape)];
    templates::template_new_project(&state, "memories", &files, None).unwrap();
    let slots = templates::template_slots(&state).unwrap();
    let request = SaveRequest {
        name: "Two memories".into(),
        slots: vec![slots[1].segment_id.clone(), slots[0].segment_id.clone()],
        labels: vec!["Second".into(), "First".into()],
        ..SaveRequest::default()
    };
    let info = templates::template_save(&state, &request).unwrap();
    assert!(!info.builtin);
    assert_eq!(info.slots.len(), 2);
    assert_eq!(info.slots[0].label.as_deref(), Some("Second"));
    let dir = PathBuf::from(info.path.clone().unwrap());
    assert!(dir.starts_with(root.join("data")));
    assert!(dir.join("template.json").is_file());
    assert!(templates::template_list().iter().any(|t| t.id == info.id));

    // A project from the saved template, filled with one file.
    let applied =
        templates::template_build_project(&info.id, &[path(&media.counter)], None).unwrap();
    assert_eq!(applied.filled.len(), 1);
    assert_eq!(applied.empty, vec![2]);
    // The look, the grain clip and the title came along.
    let project = &applied.project;
    assert!(!project.materials.color_adjusts.is_empty());
    assert!(!project.materials.effects.is_empty());
    assert!(!project.materials.texts.is_empty());

    templates::template_delete(&info.id).unwrap();
    assert!(!dir.exists());
    assert!(templates::template_delete("quick-cuts").is_err());
}

#[test]
fn every_built_in_draws_a_preview_tile() {
    isolate();
    if support::gpu().is_none() {
        eprintln!("skipped: no GPU");
        return;
    }
    for info in templates::template_list().into_iter().filter(|t| t.builtin) {
        let tile = templates::template_thumbnail(&info.id, 120)
            .unwrap_or_else(|e| panic!("{}: {e}", info.id));
        let image = image::open(&tile).unwrap().to_rgba8();
        let short = image.width().min(image.height());
        assert_eq!(short, 120, "{}", info.id);
        // Not a black frame: the sample pictures are in the slots.
        let lit = image
            .pixels()
            .filter(|p| p.0[0] > 40 || p.0[2] > 40)
            .count();
        assert!(
            lit > (image.width() * image.height()) as usize / 10,
            "{} tile is nearly black",
            info.id
        );
    }
}
