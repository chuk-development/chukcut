//! Edit invariants under fuzzing, and undo/redo over a long session.
//!
//! This is the highest-value test in the suite and it is worth saying why.
//! Every claim `docs/architecture/timeline-editing.md` makes about editing is a
//! property of *all* command sequences, not of the dozen sequences somebody
//! thought to write down:
//!
//! - an applied command leaves a document `validate()` finds no errors in;
//! - `invert()` is exact — apply then invert is the identity, byte for byte;
//! - a rejected command changes nothing at all, including a composite that
//!   failed on its third part;
//! - undoing a whole session returns the document it started from, and redoing
//!   it returns the one it ended at.
//!
//! Overlap and ordering bugs are exactly the kind that survive example-based
//! tests: they need two specific clips at two specific times, and nobody
//! writes that case down until a user hits it. A few thousand pseudo-random
//! edits find them before the user does.
//!
//! The generator is seeded, so a failure is reproducible: the panic names the
//! iteration and the command, and rerunning with the same seed replays it.

mod support;

use chukcut_lib::modules::project::document::{
    AudioMaterial, CanvasConfig, ImageMaterial, Micros, Project, Severity, TextAlign, TextMaterial,
    Track, TrackKind, VideoMaterial,
};
use chukcut_lib::modules::timeline::ops::EditCommand;
use chukcut_lib::modules::timeline::History;

use support::edits::EditFuzzer;
use support::{canonical, Lcg};

/// How many edits the default run applies. Enough to hit every pair of
/// command kinds many times over while staying inside a fast suite; the soak
/// test below runs twenty times as many.
const ITERATIONS: usize = 2_000;

/// A starting document with every material kind and a few populated lanes.
fn seed_project() -> Project {
    let mut project = Project::new(
        "fuzz",
        CanvasConfig {
            width: 1080,
            height: 1920,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );

    for i in 0..3 {
        project.materials.videos.push(VideoMaterial {
            id: format!("video-{i}"),
            path: format!("/media/clip{i}.mp4"),
            width: 1920,
            height: 1080,
            duration: 60_000_000,
            fps: 30.0,
            has_audio: true,
            rotation: 0,
        });
    }
    project.materials.audios.push(AudioMaterial {
        id: "audio-0".into(),
        path: "/media/music.wav".into(),
        duration: 60_000_000,
        sample_rate: 48_000,
        channels: 2,
    });
    project.materials.images.push(ImageMaterial {
        id: "image-0".into(),
        path: "/media/logo.png".into(),
        width: 512,
        height: 512,
    });
    project.materials.texts.push(TextMaterial {
        id: "text-0".into(),
        content: "Title".into(),
        font_family: "Inter".into(),
        font_size: 48.0,
        color: [1.0, 1.0, 1.0, 1.0],
        bold: false,
        italic: false,
        align: TextAlign::Center,
        stroke_width: 0.0,
        stroke_color: [0.0, 0.0, 0.0, 1.0],
        shadow: None,
        background: None,
    });

    for (index, kind) in [TrackKind::Video, TrackKind::Video, TrackKind::Audio]
        .into_iter()
        .enumerate()
    {
        let mut track = Track::new(kind, format!("Lane {index}"));
        track.id = format!("track-{index}");
        let mut at: Micros = 0;
        for n in 0..6 {
            let mut segment = support::segment(
                ["video-0", "video-1", "video-2", "audio-0", "image-0", "text-0"][n % 6],
                at,
                1_000_000,
            );
            segment.id = format!("seed-{index}-{n}");
            segment.render_index = index as i32;
            at += 1_500_000;
            track.segments.push(segment);
        }
        project.tracks.push(track);
    }
    project
}

/// Structural errors in `project`, which is what must always be empty.
///
/// Warnings are ignored on purpose: the fixture's media does not exist on
/// disk, and "the world is missing a file" is not an edit bug.
fn errors(project: &Project) -> Vec<String> {
    project
        .validate()
        .into_iter()
        .filter(|issue| issue.severity == Severity::Error)
        .map(|issue| issue.message)
        .collect()
}

/// A one-line description of a command, for a failure message that can be
/// acted on without rerunning anything.
fn describe(command: &EditCommand) -> String {
    serde_json::to_string(command).unwrap_or_else(|_| command.label())
}

/// Apply `iterations` random edits, checking every invariant after each one.
fn fuzz(seed: u64, iterations: usize) {
    let mut project = seed_project();
    let mut fuzzer = EditFuzzer::new(seed);

    assert!(
        errors(&project).is_empty(),
        "the seed document is already invalid"
    );

    let mut applied = 0usize;
    let mut rejected = 0usize;

    for iteration in 0..iterations {
        let Some(command) = fuzzer.next(&project) else {
            continue;
        };
        let before = canonical(&project);

        match command.apply(&mut project) {
            Err(_) => {
                rejected += 1;
                // "A failed edit leaves the document exactly as it was." A
                // partially applied composite is the case this is really
                // guarding, and the one most likely to regress.
                assert_eq!(
                    canonical(&project),
                    before,
                    "seed {seed} iteration {iteration}: a rejected {} changed the document\n{}",
                    command.label(),
                    describe(&command)
                );
            }
            Ok(()) => {
                applied += 1;
                let issues = errors(&project);
                assert!(
                    issues.is_empty(),
                    "seed {seed} iteration {iteration}: {} produced an invalid document: {issues:?}\n{}",
                    command.label(),
                    describe(&command)
                );

                let after = canonical(&project);

                command.invert().apply(&mut project).unwrap_or_else(|e| {
                    panic!(
                        "seed {seed} iteration {iteration}: {} could not be undone: {e}\n{}",
                        command.label(),
                        describe(&command)
                    )
                });
                assert_eq!(
                    canonical(&project),
                    before,
                    "seed {seed} iteration {iteration}: undoing {} did not restore the document\n{}",
                    command.label(),
                    describe(&command)
                );

                // Redo has to be exact too, and re-applying to a document that
                // is byte-identical must produce a byte-identical result — an
                // edit whose outcome depends on hidden state would show up
                // here and nowhere else.
                command.apply(&mut project).unwrap_or_else(|e| {
                    panic!(
                        "seed {seed} iteration {iteration}: {} applied, undid, then refused to redo: {e}\n{}",
                        command.label(),
                        describe(&command)
                    )
                });
                assert_eq!(
                    canonical(&project),
                    after,
                    "seed {seed} iteration {iteration}: redoing {} produced a different document\n{}",
                    command.label(),
                    describe(&command)
                );
            }
        }
    }

    // A run where almost everything was rejected would pass every assertion
    // above while testing nothing, so the shape of the run is itself checked.
    assert!(
        applied > iterations / 10,
        "only {applied} of {iterations} edits applied; the fuzzer is not editing anything"
    );
    assert!(
        rejected > iterations / 100,
        "only {rejected} of {iterations} edits were rejected; the fuzzer is not colliding"
    );
}

#[test]
fn thousands_of_random_edits_never_produce_an_invalid_document() {
    fuzz(20_250_725, ITERATIONS);
}

#[test]
fn the_same_invariants_hold_from_a_different_seed() {
    // A second seed for the price of a second run: the generator's weights
    // mean one seed can miss a command pair entirely.
    fuzz(0xDEAD_BEEF, ITERATIONS);
}

/// Run with `cargo test --test edit_fuzz -- --ignored`.
#[test]
#[ignore = "soak test: forty thousand edits across four seeds, about a minute"]
fn a_long_soak_finds_nothing_the_short_run_missed() {
    for seed in [1, 7, 0x5EED, u64::MAX / 3] {
        fuzz(seed, 10_000);
    }
}

// ---------------------------------------------------------------------------
// Undo and redo over a long session
// ---------------------------------------------------------------------------

/// Apply `count` accepted edits through `History`, returning the documents
/// before and after and how many edits actually landed.
fn long_session(seed: u64, count: usize) -> (Project, History, String, String, usize) {
    let mut project = seed_project();
    let mut history = History::new();
    let mut fuzzer = EditFuzzer::new(seed);
    let initial = canonical(&project);

    let mut depth = 0usize;
    let mut attempts = 0usize;
    while depth < count && attempts < count * 20 {
        attempts += 1;
        let Some(command) = fuzzer.next(&project) else {
            continue;
        };
        // A rejected edit must not reach the undo stack: undoing something the
        // user never saw happen is worse than not offering undo at all.
        if history.apply(&mut project, command).is_ok() {
            depth += 1;
        }
    }

    let finished = canonical(&project);
    (project, history, initial, finished, depth)
}

#[test]
fn undoing_a_long_session_completely_returns_the_document_it_started_from() {
    let (mut project, mut history, initial, finished, depth) = long_session(4242, 300);
    assert!(depth >= 250, "only {depth} edits made it onto the stack");
    assert_ne!(initial, finished, "the session did not change anything");

    let mut undone = 0usize;
    while history.can_undo() {
        history
            .undo(&mut project)
            .unwrap_or_else(|e| panic!("undo {undone} of {depth} failed: {e}"));
        undone += 1;
        assert!(undone <= depth, "undo produced more steps than were applied");
    }

    assert_eq!(undone, depth, "every applied edit is one undo step");
    assert_eq!(
        canonical(&project),
        initial,
        "a fully undone session did not return the original document"
    );
    assert!(errors(&project).is_empty());
}

#[test]
fn redoing_a_long_session_completely_returns_the_document_it_ended_at() {
    let (mut project, mut history, initial, finished, depth) = long_session(0xB0A7, 300);

    while history.can_undo() {
        history.undo(&mut project).expect("undo");
    }
    assert_eq!(canonical(&project), initial);

    let mut redone = 0usize;
    while history.can_redo() {
        history
            .redo(&mut project)
            .unwrap_or_else(|e| panic!("redo {redone} of {depth} failed: {e}"));
        redone += 1;
    }

    assert_eq!(redone, depth);
    assert_eq!(
        canonical(&project),
        finished,
        "a fully redone session did not return the document the session ended at"
    );
    assert!(errors(&project).is_empty());
}

#[test]
fn undo_and_redo_can_be_walked_back_and_forth_without_drifting() {
    // Partial undo followed by partial redo is what a user actually does, and
    // it is where an off-by-one between the two stacks shows up.
    let (mut project, mut history, initial, finished, depth) = long_session(99, 200);
    let mut rng = Lcg::new(7);

    for _ in 0..400 {
        if rng.chance(2) {
            history.undo(&mut project).expect("undo");
        } else {
            history.redo(&mut project).expect("redo");
        }
        assert!(
            errors(&project).is_empty(),
            "walking the history produced an invalid document"
        );
    }

    while history.can_undo() {
        history.undo(&mut project).expect("undo");
    }
    assert_eq!(canonical(&project), initial, "after wandering, undo still lands home");

    while history.can_redo() {
        history.redo(&mut project).expect("redo");
    }
    assert_eq!(canonical(&project), finished);
    assert!(depth > 0);
}

#[test]
fn a_rejected_edit_never_reaches_the_undo_stack() {
    let mut project = seed_project();
    let mut history = History::new();

    // Two clips cannot share a range; the second insert is refused.
    let track_id = project.tracks[0].id.clone();
    let occupied = project.tracks[0].segments[0].target_range;
    let mut intruder = support::segment("video-0", occupied.start, occupied.duration);
    intruder.id = "intruder".into();

    let before = canonical(&project);
    let result = history.apply(
        &mut project,
        EditCommand::InsertSegment {
            track_id,
            segment: intruder,
            index: 0,
        },
    );

    assert!(result.is_err(), "an occupied range must be refused");
    assert_eq!(canonical(&project), before);
    assert!(!history.can_undo(), "a refused edit is not undoable");
    assert_eq!(history.undo_label(), None);
}
