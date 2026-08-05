//! Edit invariants under fuzzing, and undo/redo over a long session.
//!
//! This is the highest-value test in the suite and it is worth saying why.
//! Every claim `docs/architecture/timeline-editing.md` makes about editing is a
//! property of *all* command sequences, not of the dozen sequences somebody
//! thought to write down:
//!
//! - an applied command leaves a document `validate()` finds no errors in;
//! - an applied command leaves a document that can still be **saved and
//!   reopened**, which is a stronger statement than `validate()` makes and the
//!   one a user notices when it stops being true;
//! - the structural invariants hold, checked here independently of
//!   `validate()` so that a hole in the checker cannot hide a hole in the
//!   commands;
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

use std::collections::HashSet;

use chukcut_lib::modules::project::document::{
    source_duration_for, speed_slack, AnimatableProperty, AudioMaterial, CanvasConfig, Easing,
    ImageMaterial, Keyframe, Micros, Project, Severity, TextAlign, TextMaterial, Track, TrackKind,
    TransitionKind, TransitionMaterial, VideoMaterial,
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
                [
                    "video-0", "video-1", "video-2", "audio-0", "image-0", "text-0",
                ][n % 6],
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

/// The document's structural invariants, checked here rather than asked of
/// `Project::validate()`.
///
/// Deliberate duplication. `validate()` is itself a thing that can be wrong —
/// every one of these was a gap in it at some point — and a fuzzer that only
/// asks the checker whether the checker is happy tests nothing when the checker
/// stops looking.
fn invariants(project: &Project) -> Vec<String> {
    let mut broken = Vec::new();
    let mut track_ids: HashSet<&str> = HashSet::new();
    let mut segment_ids: HashSet<&str> = HashSet::new();

    if !project.fps.is_finite() {
        broken.push(format!("project fps is {}", project.fps));
    }

    for track in &project.tracks {
        if !track_ids.insert(track.id.as_str()) {
            broken.push(format!("two tracks share the id {}", track.id));
        }
        if !track.volume.is_finite() {
            broken.push(format!("track {} volume is {}", track.id, track.volume));
        }

        for segment in &track.segments {
            let id = &segment.id;
            if !segment_ids.insert(segment.id.as_str()) {
                broken.push(format!("two segments share the id {id}"));
            }
            if segment.target_range.start < 0 {
                broken.push(format!(
                    "segment {id} starts at {} µs",
                    segment.target_range.start
                ));
            }
            if segment.target_range.duration <= 0 {
                broken.push(format!(
                    "segment {id} lasts {} µs",
                    segment.target_range.duration
                ));
            }
            if segment.source_range.start < 0 {
                broken.push(format!(
                    "segment {id} reads the source from {} µs",
                    segment.source_range.start
                ));
            }
            if segment.source_range.duration <= 0 {
                broken.push(format!(
                    "segment {id} shows {} µs of source",
                    segment.source_range.duration
                ));
            }
            if let Some(field) = segment.non_finite_field() {
                broken.push(format!("segment {id} has a non-finite {field}"));
            } else if segment.speed <= 0.0 {
                broken.push(format!("segment {id} runs at {}x", segment.speed));
            } else {
                // `source_range.duration = target_range.duration × speed`.
                let implied = source_duration_for(segment.target_range.duration, segment.speed);
                let drift = (segment.source_range.duration - implied).abs();
                if drift > speed_slack(segment.speed) {
                    broken.push(format!(
                        "segment {id} shows {} µs of source for {} µs of timeline at {}x, \
                         which should be {implied} µs",
                        segment.source_range.duration, segment.target_range.duration, segment.speed
                    ));
                }
            }

            // Transitions. A transition is centred on a cut, so the cut has to
            // exist: the clip carrying it needs the one before it on the same
            // track to end exactly where it starts. Stated from the document
            // rather than by calling `transitions::validate`, for the reason at
            // the top of this function.
            for extra in &segment.extras {
                if !project.materials.transitions.iter().any(|t| &t.id == extra) {
                    continue; // Not a transition; effects live in `extras` too.
                }
                let joined = track
                    .segments
                    .iter()
                    .position(|s| s.id == segment.id)
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|i| track.segments.get(i))
                    .is_some_and(|previous| {
                        previous.target_range.end() == segment.target_range.start
                    });
                if !joined {
                    broken.push(format!(
                        "segment {id} carries a transition with no clip ending where it starts"
                    ));
                }
            }

            // Keyframes. A track exists exactly when the property is animated,
            // it holds each property once, and its times are sorted and
            // distinct — the sampler reads neighbouring pairs by binary search
            // and quietly interpolates the wrong two otherwise.
            let mut animated: HashSet<AnimatableProperty> = HashSet::new();
            for keys in &segment.keyframes {
                if !animated.insert(keys.property) {
                    broken.push(format!("segment {id} animates {:?} twice", keys.property));
                }
                if keys.keyframes.is_empty() {
                    broken.push(format!(
                        "segment {id} has an empty {:?} track, which reads as animated",
                        keys.property
                    ));
                }
                if keys.keyframes.windows(2).any(|w| w[0].time >= w[1].time) {
                    broken.push(format!(
                        "segment {id} has {:?} keyframes out of order or sharing a time",
                        keys.property
                    ));
                }
            }
        }
    }
    broken
}

/// Prove the document can still make the trip a save makes it take.
///
/// This is the assertion that would have caught the worst of the defects: a
/// non-finite number reaches disk as `null` — `serde_json` has no other way to
/// write it — and the file never opens again, having reported a successful
/// save. Nothing else in the suite notices, because in memory the document is
/// perfectly fine.
fn reopens(project: &Project) -> Result<(), String> {
    let json = serde_json::to_string(project).map_err(|e| e.to_string())?;
    serde_json::from_str::<Project>(&json)
        .map(|_| ())
        .map_err(|e| format!("{e} — the saved file would never open again"))
}

/// The properties the generator animates. All of them, so that the canonical
/// order of a segment's keyframe tracks is exercised from every direction.
const PROPERTIES: [AnimatableProperty; 7] = [
    AnimatableProperty::PositionX,
    AnimatableProperty::PositionY,
    AnimatableProperty::ScaleX,
    AnimatableProperty::ScaleY,
    AnimatableProperty::Rotation,
    AnimatableProperty::Opacity,
    AnimatableProperty::Volume,
];

const EASINGS: [Easing; 5] = [
    Easing::Hold,
    Easing::Linear,
    Easing::EaseIn,
    Easing::EaseOut,
    Easing::EaseInOut,
];

/// One keyframe command against a random segment, or `None` when the document
/// has no segments.
///
/// Kept here rather than in the shared generator because the keyframe commands
/// need to read the segment's *current* animation to build a command that
/// nearly works — a remove naming a keyframe that was never there only tests
/// the error path.
fn keyframe_command(project: &Project, rng: &mut Lcg) -> Option<EditCommand> {
    let segments: Vec<(String, Micros)> = project
        .tracks
        .iter()
        .flat_map(|t| {
            t.segments
                .iter()
                .map(|s| (s.id.clone(), s.target_range.duration))
        })
        .collect();
    let (segment_id, duration) = rng.pick(&segments)?.clone();
    let (_, segment) = project.segment(&segment_id)?;

    // Times land on a coarse grid inside — and occasionally just outside — the
    // clip, so that collisions with existing keyframes are common.
    let grid = (duration / 8).max(1);
    let time = rng.between(-1, 9) * grid;

    // Half the time, aim at a keyframe that is really there — and carry its
    // real value and easing, because that is what the UI sends: the `before`
    // side of a command is read out of the document, and undo is only exact
    // because of it.
    let existing: Vec<(AnimatableProperty, Keyframe)> = segment
        .keyframes
        .iter()
        .flat_map(|t| t.keyframes.iter().map(move |k| (t.property, *k)))
        .collect();
    let aimed = if rng.chance(2) {
        rng.pick(&existing).copied()
    } else {
        None
    };
    let (property, at) = match aimed {
        Some((property, keyframe)) => (property, keyframe),
        None => {
            let property = *rng.pick(&PROPERTIES)?;
            // If a keyframe happens to sit at the time this picked, the command
            // has to carry *that* keyframe, not a guess at it. The UI reads the
            // `before` side out of the document, and a command claiming a
            // keyframe had a value it never had undoes to a document the user
            // never saw — which is a bug in the generator, not in the command,
            // and one this loop was blaming on `RemoveKeyframe` until it was
            // tracked down.
            let real = existing
                .iter()
                .find(|(p, k)| *p == property && k.time == time)
                .map(|(_, k)| *k);
            (
                property,
                real.unwrap_or(Keyframe {
                    time,
                    value: 0.0,
                    easing: Easing::Linear,
                }),
            )
        }
    };

    Some(match rng.below(4) {
        0 => EditCommand::AddKeyframe {
            segment_id,
            property,
            keyframe: Keyframe {
                time,
                value: rng.unit() * 2.0 - 1.0,
                easing: *rng.pick(&EASINGS)?,
            },
        },
        1 => EditCommand::RemoveKeyframe {
            segment_id,
            property,
            keyframe: at,
        },
        2 => EditCommand::MoveKeyframe {
            segment_id,
            property,
            from_time: at.time,
            to_time: time,
            before_value: at.value,
            after_value: rng.unit() * 2.0 - 1.0,
        },
        _ => EditCommand::SetKeyframeEasing {
            segment_id,
            property,
            time: at.time,
            before: at.easing,
            after: *rng.pick(&EASINGS)?,
        },
    })
}

/// The transition attached to `segment_id`, found without asking the
/// transitions module — an id in `extras` that resolves in the pool.
fn attached_transition(project: &Project, segment_id: &str) -> Option<TransitionMaterial> {
    let (_, segment) = project.segment(segment_id)?;
    segment.extras.iter().find_map(|extra| {
        project
            .materials
            .transitions
            .iter()
            .find(|t| &t.id == extra)
            .cloned()
    })
}

/// One transition command against a cut that really exists.
///
/// A transition sits at the head of a clip whose predecessor ends exactly where
/// it starts, so the generator finds those joins itself rather than offering
/// commands that can only be refused.
fn transition_command(project: &Project, rng: &mut Lcg) -> Option<EditCommand> {
    let mut joins: Vec<(String, Micros)> = Vec::new();
    for track in &project.tracks {
        for pair in track.segments.windows(2) {
            if pair[0].target_range.end() == pair[1].target_range.start {
                // The longest a centred transition can be: half of it hangs
                // over each side, so neither clip may be shorter than half.
                let limit = 2 * pair[0]
                    .target_range
                    .duration
                    .min(pair[1].target_range.duration);
                joins.push((pair[1].id.clone(), limit));
            }
        }
    }
    let (segment_id, limit) = rng.pick(&joins)?.clone();

    let kinds = [
        TransitionKind::Dissolve,
        TransitionKind::DipToColor,
        TransitionKind::Wipe,
        TransitionKind::Slide,
        TransitionKind::Zoom,
    ];
    // Occasionally longer than the clips allow, which must be refused rather
    // than clamped behind the user's back.
    let duration = if rng.chance(6) {
        limit + 100_000
    } else {
        rng.between(1, (limit / 100_000).max(1)) * 100_000
    };

    Some(
        match (attached_transition(project, &segment_id), rng.below(3)) {
            (Some(transition), 0) => EditCommand::RemoveTransition {
                segment_id,
                transition,
            },
            (Some(before), _) => {
                let mut after = before.clone();
                after.duration = duration;
                after.kind = *rng.pick(&kinds)?;
                EditCommand::SetTransition {
                    segment_id,
                    before,
                    after,
                }
            }
            (None, _) => EditCommand::AddTransition {
                segment_id,
                transition: TransitionMaterial::new(*rng.pick(&kinds)?, duration),
            },
        },
    )
}

/// Values `serde_json` cannot write.
const POISONS: [f32; 3] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];

/// Replace one float in `command` with a value that cannot be saved, returning
/// whether there was one to replace.
///
/// The UI produces these by accident, not by malice: a slider divided by a zero
/// width, a speed derived from a duration that was momentarily zero, a
/// normalized position computed before the canvas had a size. The command
/// boundary is the last place to stop them.
fn poison(command: &mut EditCommand, rng: &mut Lcg) -> bool {
    let bad = POISONS[rng.below(POISONS.len())];
    match command {
        EditCommand::SetTransform { after, .. } => {
            match rng.below(4) {
                0 => after.opacity = bad,
                1 => after.rotation = bad,
                2 => after.scale[rng.below(2)] = bad,
                _ => after.position[rng.below(2)] = bad,
            }
            true
        }
        EditCommand::SetSpeed { after, .. } => {
            *after = bad;
            true
        }
        EditCommand::SetVolume { after, .. } => {
            *after = bad;
            true
        }
        EditCommand::SetTrackFlags { after, .. } => {
            after.volume = bad;
            true
        }
        EditCommand::InsertSegment { segment, .. } => {
            if rng.chance(2) {
                segment.transform.opacity = bad;
            } else {
                segment.volume = bad;
            }
            true
        }
        EditCommand::AddKeyframe { keyframe, .. } => {
            keyframe.value = bad;
            true
        }
        EditCommand::MoveKeyframe { after_value, .. } => {
            *after_value = bad;
            true
        }
        _ => false,
    }
}

/// Apply `iterations` random edits, checking every invariant after each one.
fn fuzz(seed: u64, iterations: usize) {
    let mut project = seed_project();
    let mut fuzzer = EditFuzzer::new(seed);

    assert!(
        errors(&project).is_empty(),
        "the seed document is already invalid"
    );

    assert!(
        invariants(&project).is_empty(),
        "the seed document already breaks an invariant"
    );

    let mut applied = 0usize;
    let mut rejected = 0usize;
    let mut poisoned_count = 0usize;

    for iteration in 0..iterations {
        // One edit in five is a keyframe edit. They are generated here rather
        // than by the shared generator because they have to read the segment's
        // current animation to be worth sending.
        let generated = if fuzzer.rng.chance(5) {
            keyframe_command(&project, &mut fuzzer.rng)
        } else {
            fuzzer.next(&project)
        };
        let Some(mut command) = generated else {
            continue;
        };

        // One command in twenty carries a number that cannot be written to
        // JSON. Every one of them must be refused: a project that has taken one
        // saves successfully and then never opens again.
        let poisoned = fuzzer.rng.chance(20) && poison(&mut command, &mut fuzzer.rng);
        let command = command;

        let before = canonical(&project);

        if poisoned {
            poisoned_count += 1;
            let result = command.apply(&mut project);
            assert!(
                result.is_err(),
                "seed {seed} iteration {iteration}: {} accepted a value that cannot be saved\n{}",
                command.label(),
                describe(&command)
            );
            assert_eq!(
                canonical(&project),
                before,
                "seed {seed} iteration {iteration}: a refused {} still changed the document",
                command.label()
            );
            continue;
        }

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

                let broken = invariants(&project);
                assert!(
                    broken.is_empty(),
                    "seed {seed} iteration {iteration}: {} broke an invariant: {broken:?}\n{}",
                    command.label(),
                    describe(&command)
                );

                if let Err(why) = reopens(&project) {
                    panic!(
                        "seed {seed} iteration {iteration}: after {} the project could not be \
                         saved and reopened: {why}\n{}",
                        command.label(),
                        describe(&command)
                    );
                }

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
    assert!(
        poisoned_count > iterations / 100,
        "only {poisoned_count} of {iterations} edits carried an unsaveable number; the check \
         above is not being exercised"
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
// The cases the generator cannot reach on its own
// ---------------------------------------------------------------------------
//
// The generator builds commands that are *consistent with the document it can
// see*, which is the right default — a command naming a track that never
// existed tests the error path and nothing else. But two of the defects below
// only appear when a command is consistent with a document that has since
// moved on, which is what a real UI sends when a round trip is slow or a panel
// is stale. Those are constructed here.

#[test]
fn a_stale_track_index_never_deletes_a_different_lane() {
    // `RemoveTrack` carries both the track and the index it had. Removing
    // whatever is at the index is how a delete performed against a timeline
    // that has since gained or lost a lane silently deletes someone else's
    // work — and reports success, so nothing downstream notices either.
    let mut rng = Lcg::new(0x5A1E);

    for _ in 0..200 {
        let mut project = seed_project();
        let count = project.tracks.len();
        let victim = rng.below(count);
        let track = project.tracks[victim].clone();
        let before = canonical(&project);

        let stale = rng.below(count);
        let command = EditCommand::RemoveTrack {
            track: track.clone(),
            index: stale,
        };

        if stale == victim {
            command.apply(&mut project).expect("the honest index works");
            assert!(
                !project.tracks.iter().any(|t| t.id == track.id),
                "the lane it named is the lane that went"
            );
            assert_eq!(project.tracks.len(), count - 1);
        } else {
            assert!(
                command.apply(&mut project).is_err(),
                "index {stale} does not hold {}, so the delete must be refused",
                track.id
            );
            assert_eq!(
                canonical(&project),
                before,
                "a refused delete removed a lane anyway"
            );
        }
        assert!(invariants(&project).is_empty());
    }
}

#[test]
fn transitions_survive_the_edits_that_move_the_clips_they_join() {
    // A transition is the one thing in the document that depends on *two*
    // segments, so the sequence that matters is: place one, then move, trim,
    // split or delete a clip it joins. The weighted generator never emits a
    // transition command at all, so that sequence was unreachable until now.
    let mut project = seed_project();
    let mut fuzzer = EditFuzzer::new(0x7A11);

    let mut placed = 0usize;
    let mut structural = 0usize;

    for iteration in 0..1_500 {
        // Two in five are transition commands, because a join has to exist
        // before the rest of the mix can disturb it.
        let generated = if fuzzer.rng.chance(2) {
            transition_command(&project, &mut fuzzer.rng)
        } else {
            fuzzer.next(&project)
        };
        let Some(command) = generated else {
            continue;
        };
        // Exactly what the app does with a command before it applies it: an
        // edit that breaks a cut carries the removal of the transition that
        // was sitting on it, so that one undo brings both back.
        let command =
            chukcut_lib::modules::timeline::ops::detach_broken_transitions(&project, command);
        let is_transition = matches!(
            command,
            EditCommand::AddTransition { .. }
                | EditCommand::RemoveTransition { .. }
                | EditCommand::SetTransition { .. }
        );

        let before = canonical(&project);
        match command.apply(&mut project) {
            Err(_) => assert_eq!(
                canonical(&project),
                before,
                "iteration {iteration}: a rejected {} changed the document",
                command.label()
            ),
            Ok(()) => {
                if is_transition {
                    placed += 1;
                } else {
                    structural += 1;
                }

                // Warnings are not failures here, and the distinction is the
                // whole point: a transition longer than the clips it joins is
                // what an ordinary trim produces, and the renderer clamps the
                // window. Only `Severity::Error` means the document is wrong.
                let issues = errors(&project);
                assert!(
                    issues.is_empty(),
                    "iteration {iteration}: {} produced an invalid document: {issues:?}\n{}",
                    command.label(),
                    describe(&command)
                );
                let broken = invariants(&project);
                assert!(
                    broken.is_empty(),
                    "iteration {iteration}: {} broke an invariant: {broken:?}\n{}",
                    command.label(),
                    describe(&command)
                );

                let after = canonical(&project);
                command.invert().apply(&mut project).unwrap_or_else(|e| {
                    panic!(
                        "iteration {iteration}: {} could not be undone: {e}\n{}",
                        command.label(),
                        describe(&command)
                    )
                });
                assert_eq!(
                    canonical(&project),
                    before,
                    "iteration {iteration}: undoing {} did not restore the document\n{}",
                    command.label(),
                    describe(&command)
                );
                command.apply(&mut project).expect("redo");
                assert_eq!(canonical(&project), after);
            }
        }
    }

    assert!(
        placed > 50 && structural > 100,
        "{placed} transition edits and {structural} others is not a mix"
    );
    assert!(
        !project.materials.transitions.is_empty(),
        "the run finished with no transitions in the document at all"
    );
}

#[test]
fn speed_changes_and_splits_interleave_without_the_two_ranges_drifting() {
    // `SetSpeed` and `split_at` are the pair the invariant exists for: one
    // changes the factor between the ranges, the other divides them. Neither
    // is generated often enough by the weighted mix above to hit the other
    // many times over, so they are pushed together here.
    let mut project = seed_project();
    let mut rng = Lcg::new(0xF00D);

    let mut speeds = 0usize;
    let mut splits = 0usize;

    for _ in 0..2_000 {
        let ids: Vec<String> = project
            .tracks
            .iter()
            .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
            .collect();
        let Some(id) = rng.pick(&ids).cloned() else {
            break;
        };
        let before = canonical(&project);

        let applied = if rng.chance(2) {
            let (_, segment) = project.segment(&id).expect("just listed");
            let command = EditCommand::SetSpeed {
                segment_id: id.clone(),
                before: segment.speed,
                after: 0.25 + rng.unit() * 3.75,
            };
            let ok = command.apply(&mut project).is_ok();
            if ok {
                speeds += 1;
            }
            ok
        } else {
            let (_, segment) = project.segment(&id).expect("just listed");
            let range = segment.target_range;
            let speed = segment.speed;
            let source_start = segment.source_range.start;
            if range.duration < 200_000 {
                continue;
            }
            let at = range.start + rng.between(1, range.duration / 100_000 - 1) * 100_000;
            match chukcut_lib::modules::timeline::ops::split_at(&project, &id, at) {
                Ok(command) => {
                    if command.apply(&mut project).is_err() {
                        continue;
                    }
                    splits += 1;

                    // The property a user would notice: the frame the clip
                    // showed at the cut is the frame the second half starts on.
                    // At any speed but 1x this is where the arithmetic used to
                    // go wrong.
                    let (track, left) = project.segment(&id).expect("the left half");
                    let right = track
                        .segments
                        .iter()
                        .find(|s| s.target_range.start == at)
                        .expect("the right half");
                    assert_eq!(left.source_range.start, source_start);
                    assert_eq!(
                        left.source_range.duration,
                        source_duration_for(at - range.start, speed),
                        "the cut landed in the wrong place in the source"
                    );
                    assert_eq!(
                        right.source_range.start,
                        left.source_range.end(),
                        "the two halves do not join in the source"
                    );
                    assert_eq!(
                        right.source_time_at(at),
                        Some(left.source_range.end()),
                        "the second half does not start where the first left off"
                    );
                    true
                }
                Err(_) => continue,
            }
        };

        if !applied {
            assert_eq!(canonical(&project), before, "a refused edit changed things");
            continue;
        }

        let broken = invariants(&project);
        assert!(broken.is_empty(), "{broken:?}");
        assert!(errors(&project).is_empty(), "{:?}", errors(&project));
    }

    assert!(
        speeds > 100 && splits > 100,
        "{speeds} speeds, {splits} splits"
    );
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
        let generated = if fuzzer.rng.chance(5) {
            keyframe_command(&project, &mut fuzzer.rng)
        } else {
            fuzzer.next(&project)
        };
        let Some(command) = generated else {
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
        assert!(
            undone <= depth,
            "undo produced more steps than were applied"
        );
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
    assert_eq!(
        canonical(&project),
        initial,
        "after wandering, undo still lands home"
    );

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
