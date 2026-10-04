//! Sequences without a GPU: the document, the edits, the time arithmetic.
//! Rendering through nesting is pinned in `tests/compound.rs`.

use super::*;
use crate::modules::project::{
    AnimatableProperty, AudioMaterial, CanvasConfig, Keyframe, KeyframeTrack, Micros, Project,
    Segment, Severity, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use crate::modules::timeline::ops::{link, EditCommand};
use crate::modules::timeline::History;

const S: Micros = 1_000_000;

fn segment(id: &str, material: &str, start: Micros, duration: Micros, source: Micros) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(source, duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// Two clips on the main lane, an overlay above, music below.
fn project() -> Project {
    let mut p = Project::new("p", CanvasConfig::default(), 30.0);
    p.materials.videos.push(VideoMaterial {
        id: "take".into(),
        path: "/nonexistent/take.mp4".into(),
        width: 1920,
        height: 1080,
        duration: 60 * S,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    p.materials.audios.push(AudioMaterial {
        id: "music".into(),
        path: "/nonexistent/music.mp3".into(),
        duration: 60 * S,
        sample_rate: 48_000,
        channels: 2,
    });
    let mut main = Track::new(TrackKind::Video, "Video 1");
    main.id = "v1".into();
    main.segments = vec![
        segment("a", "take", 0, 2 * S, 0),
        segment("b", "take", 2 * S, 3 * S, 10 * S),
        segment("c", "take", 5 * S, 2 * S, 20 * S),
    ];
    let mut overlay = Track::new(TrackKind::Video, "Video 2");
    overlay.id = "v2".into();
    overlay.segments = vec![segment("o", "take", 3 * S, S, 30 * S)];
    let mut music = Track::new(TrackKind::Audio, "Audio 1");
    music.id = "a1".into();
    music.segments = vec![segment("m", "music", S, 4 * S, 0)];
    p.tracks = vec![main, overlay, music];
    for (i, t) in p.tracks.iter_mut().enumerate() {
        for s in &mut t.segments {
            s.render_index = i as i32;
        }
    }
    p
}

fn json(p: &Project) -> serde_json::Value {
    serde_json::to_value(p).unwrap()
}

fn errors(p: &Project) -> Vec<String> {
    p.validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message)
        .collect()
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// Apply through the history, which is what the app does.
fn apply(history: &mut History, p: &mut Project, command: EditCommand) {
    history.apply(p, command).expect("the edit applies");
    assert!(errors(p).is_empty(), "{:?}", errors(p));
}

/// `apply`, with the command built before the document is borrowed for it.
macro_rules! step {
    ($h:ident, $p:ident, $command:expr $(,)?) => {{
        let command = $command;
        apply(&mut $h, &mut $p, command);
    }};
}

#[test]
fn a_project_with_one_timeline_saves_exactly_as_before() {
    let p = project();
    let text = serde_json::to_string_pretty(&p).unwrap();
    assert!(!text.contains("\"sequence"), "no sequence keys: {text}");
    let back: Project = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_string_pretty(&back).unwrap(), text);
    assert!(back.sequence.is_default());
}

#[test]
fn creating_a_compound_clip_replaces_the_clips_and_undoes_exactly() {
    let mut p = project();
    let before = json(&p);
    let made = build::create_compound(&p, &ids(&["b", "o"]), None).unwrap();
    let mut h = History::new();
    step!(h, p, made.command);

    // b (2..5 s) and o (3..4 s) became one clip covering 2..5 s on the main
    // lane, where b was.
    let (lane, compound) = p.segment(&made.segment_id).unwrap();
    assert_eq!(lane.id, "v1");
    assert_eq!(compound.target_range, TimeRange::new(2 * S, 3 * S));
    assert_eq!(compound.source_range, TimeRange::new(0, 3 * S));
    assert!(p.segment("b").is_none() && p.segment("o").is_none());
    let seq = p.materials.sequence(&made.sequence_id).unwrap();
    assert_eq!(seq.kind, SequenceKind::Compound);
    assert_eq!(seq.tracks.len(), 2);
    assert_eq!(
        seq.tracks[0].segments[0].target_range,
        TimeRange::new(0, 3 * S)
    );
    assert_eq!(seq.tracks[1].segments[0].target_range, TimeRange::new(S, S));
    assert_eq!(p.duration(), 7 * S);

    h.undo(&mut p).unwrap();
    assert_eq!(json(&p), before, "one undo restores the document exactly");
    h.redo(&mut p).unwrap();
    assert!(p.segment(&made.segment_id).is_some());
}

#[test]
fn a_linked_partner_goes_into_the_compound_with_its_clip() {
    let mut p = project();
    let mut h = History::new();
    step!(h, p, link(&p, &ids(&["b", "m"])).unwrap());
    let made = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    step!(h, p, made.command);
    assert!(p.segment("m").is_none(), "the linked sound moved too");
    let seq = p.materials.sequence(&made.sequence_id).unwrap();
    assert_eq!(seq.tracks.len(), 2);
    // The compound spans both: music 1..5 s, b 2..5 s.
    assert_eq!(
        p.segment(&made.segment_id).unwrap().1.target_range,
        TimeRange::new(S, 4 * S)
    );
}

#[test]
fn when_the_clips_lane_is_still_busy_the_compound_takes_a_free_one() {
    let mut p = project();
    // a (0..2) and c (5..7) on the main lane with b left behind between them:
    // the compound's range 0..7 collides with b.
    let made = build::create_compound(&p, &ids(&["a", "c"]), None).unwrap();
    let mut h = History::new();
    step!(h, p, made.command);
    let (lane, _) = p.segment(&made.segment_id).unwrap();
    assert_ne!(lane.id, "v1");
    assert_eq!(lane.kind, TrackKind::Video);
}

#[test]
fn opening_and_closing_a_compound_clip_is_undoable_navigation() {
    let mut p = project();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["b", "o"]), None).unwrap();
    step!(h, p, made.command);
    let outside = json(&p);

    step!(h, p, build::open(&p, &made.segment_id).unwrap());
    assert_eq!(p.sequence.id, made.sequence_id);
    assert_eq!(p.sequence.path, vec![MAIN_SEQUENCE_ID.to_string()]);
    assert_eq!(
        p.tracks.len(),
        2,
        "the compound's lanes are the ones edited"
    );
    assert_eq!(root_id(&p), MAIN_SEQUENCE_ID);
    assert_eq!(breadcrumbs(&p).len(), 2);

    // An edit inside, then out again.
    let inner = p.tracks[1].segments[0].clone();
    step!(
        h,
        p,
        EditCommand::MoveSegment {
            segment_id: inner.id.clone(),
            from_track: p.tracks[1].id.clone(),
            to_track: p.tracks[1].id.clone(),
            from_start: inner.target_range.start,
            to_start: 2 * S,
        },
    );
    step!(h, p, build::close(&p).unwrap());
    assert_eq!(p.sequence.id, MAIN_SEQUENCE_ID);
    assert!(p.sequence.is_default(), "back out, nothing left to write");

    // Undo walks back through the close, the move and the open.
    h.undo(&mut p).unwrap();
    assert_eq!(p.sequence.id, made.sequence_id);
    h.undo(&mut p).unwrap();
    h.undo(&mut p).unwrap();
    assert_eq!(json(&p), outside);
}

#[test]
fn a_compound_clip_cannot_contain_itself() {
    let mut p = project();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    step!(h, p, made.command);
    let outer = build::create_compound(&p, &ids(&[made.segment_id.as_str()]), None).unwrap();
    step!(h, p, outer.command);

    // Inside the inner compound, a clip of the inner or the outer one would
    // make a loop: refused at the boundary every clip passes.
    step!(h, p, build::open(&p, &outer.segment_id).unwrap());
    step!(h, p, build::open(&p, &made.segment_id).unwrap());
    assert_eq!(p.sequence.path.len(), 2);
    let lane = p.tracks[0].id.clone();
    for material in [&made.sequence_id, &outer.sequence_id] {
        let paste = EditCommand::InsertSegment {
            track_id: lane.clone(),
            segment: segment("loop", material, 10 * S, S, 0),
            index: 1,
        };
        let refused = h.apply(&mut p, paste).unwrap_err();
        assert!(refused.contains("cannot contain itself"), "{refused}");
    }
    // A sequence edit that parks a self-referencing sequence is refused too.
    let mut selfish = Sequence {
        id: "x".into(),
        name: "x".into(),
        kind: SequenceKind::Compound,
        tracks: vec![Track::new(TrackKind::Video, "V")],
        markers: Vec::new(),
    };
    selfish.tracks[0].segments.push(segment("s", "x", 0, S, 0));
    let refused = SequenceEdit::Add {
        sequence: selfish,
        index: 0,
        transitions: Vec::new(),
        links: Vec::new(),
    }
    .apply(&mut p)
    .unwrap_err();
    assert!(refused.contains("cannot contain itself"));
}

#[test]
fn a_hand_made_loop_is_a_validation_error() {
    let mut p = project();
    let made = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    made.command.apply(&mut p).unwrap();
    // Point the compound's own clip at itself behind the commands' backs.
    let seq = p
        .materials
        .sequences
        .iter_mut()
        .find(|s| s.id == made.sequence_id)
        .unwrap();
    seq.tracks[0].segments[0].material_id = made.sequence_id.clone();
    assert!(errors(&p).iter().any(|e| e.contains("contains itself")));
}

#[test]
fn nesting_deeper_than_the_limit_is_refused() {
    let mut p = project();
    let mut h = History::new();
    let mut clip = "b".to_string();
    for _ in 0..MAX_DEPTH {
        let made = build::create_compound(&p, &[clip.clone()], None).unwrap();
        step!(h, p, made.command);
        clip = made.segment_id;
    }
    assert_eq!(
        depth_of(&p, MAIN_SEQUENCE_ID),
        Some(MAX_DEPTH),
        "eight compound clips, one inside the next"
    );
    let refused = build::create_compound(&p, &[clip], None).unwrap_err();
    assert!(refused.contains("nest at most"), "{refused}");
}

#[test]
fn flattening_a_compound_clip_puts_its_clips_back_where_they_were() {
    let original = project();
    let mut p = original.clone();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["b", "o"]), None).unwrap();
    step!(h, p, made.command);
    let compounded = json(&p);
    step!(h, p, build::flatten(&p, &made.segment_id).unwrap());

    assert!(p.materials.sequences.is_empty(), "the unused sequence went");
    for id in ["b", "o"] {
        let (lane, s) = p.segment(id).expect("same ids again");
        let (_, was) = original.segment(id).unwrap();
        assert_eq!(s.target_range, was.target_range, "{id}");
        assert_eq!(s.source_range, was.source_range, "{id}");
        assert_eq!(lane.kind, TrackKind::Video);
    }
    // The main-lane clip went home; the overlay stacked above it.
    assert_eq!(p.segment("b").unwrap().0.id, "v1");
    let lanes: Vec<&str> = p.tracks.iter().map(|t| t.id.as_str()).collect();
    let o_lane = p.segment("o").unwrap().0.id.as_str();
    let at = lanes.iter().position(|l| *l == o_lane).unwrap();
    assert_eq!(at, 1, "directly above the main lane: {lanes:?}");

    h.undo(&mut p).unwrap();
    assert_eq!(json(&p), compounded);
}

#[test]
fn flattening_a_trimmed_compound_clip_cuts_the_clips_at_its_edges() {
    let mut p = project();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["a", "b"]), None).unwrap();
    step!(h, p, made.command);
    // Show nested 1..4 s of the 0..5 s compound, at 10 s.
    let (lane, compound) = p.segment(&made.segment_id).unwrap();
    let lane = lane.id.clone();
    let compound = compound.clone();
    apply(
        &mut h,
        &mut p,
        EditCommand::TrimSegment {
            segment_id: compound.id.clone(),
            before_target: compound.target_range,
            before_source: compound.source_range,
            after_target: TimeRange::new(S, 3 * S),
            after_source: TimeRange::new(S, 3 * S),
        },
    );
    apply(
        &mut h,
        &mut p,
        EditCommand::MoveSegment {
            segment_id: compound.id.clone(),
            from_track: lane.clone(),
            to_track: lane.clone(),
            from_start: S,
            to_start: 10 * S,
        },
    );
    step!(h, p, build::flatten(&p, &compound.id).unwrap());
    // a was 0..2 s reading 0..2 s: now 10..11 s reading 1..2 s.
    let a = p.segment("a").unwrap().1;
    assert_eq!(a.target_range, TimeRange::new(10 * S, S));
    assert_eq!(a.source_range, TimeRange::new(S, S));
    // b was 2..5 s reading 10..13 s: now 11..13 s reading 10..12 s.
    let b = p.segment("b").unwrap().1;
    assert_eq!(b.target_range, TimeRange::new(11 * S, 2 * S));
    assert_eq!(b.source_range, TimeRange::new(10 * S, 2 * S));
}

#[test]
fn a_shared_compound_flattens_into_copies_and_stays() {
    let mut p = project();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    step!(h, p, made.command);
    // A second compound clip of the same sequence, as a paste makes.
    let mut copy = p.segment(&made.segment_id).unwrap().1.clone();
    copy.id = "copy".into();
    copy.target_range.start = 20 * S;
    apply(
        &mut h,
        &mut p,
        EditCommand::InsertSegment {
            track_id: "v1".into(),
            segment: copy,
            index: 3,
        },
    );
    step!(h, p, build::flatten(&p, "copy").unwrap());
    assert!(p.materials.sequence(&made.sequence_id).is_some());
    assert!(p.segment("b").is_none(), "a copy, not the original id");
    let lane = &p.tracks[0];
    let copied = lane
        .segments
        .iter()
        .find(|s| s.target_range.start == 20 * S)
        .unwrap();
    assert_eq!(copied.source_range, TimeRange::new(10 * S, 3 * S));
}

#[test]
fn timelines_keep_their_order_however_often_you_switch() {
    let mut p = project();
    let original = json(&p);
    let mut h = History::new();
    let (cmd, second) = build::new_timeline(&p, None).unwrap();
    step!(h, p, cmd);
    assert_eq!(p.sequence.id, second, "a new timeline opens");
    assert_eq!(p.tracks.len(), 2);
    let (cmd, third) = build::new_timeline(&p, Some("Shorts".into())).unwrap();
    step!(h, p, cmd);
    let order = |p: &Project| -> Vec<String> { timelines(p).into_iter().map(|t| t.id).collect() };
    let want = vec![MAIN_SEQUENCE_ID.to_string(), second.clone(), third.clone()];
    assert_eq!(order(&p), want);
    assert_eq!(timelines(&p)[1].name, "Timeline 02");

    for to in [MAIN_SEQUENCE_ID, &second, &third, &second, MAIN_SEQUENCE_ID] {
        step!(h, p, build::switch(&p, to).unwrap());
        assert_eq!(order(&p), want);
        assert_eq!(p.sequence.id, to);
    }
    assert_eq!(p.tracks.len(), 3, "the main timeline's own lanes");
    assert_eq!(p.duration(), 7 * S);

    // Undo every switch and both new timelines: the original document.
    while h.can_undo() {
        h.undo(&mut p).unwrap();
    }
    assert_eq!(json(&p), original);
}

#[test]
fn renaming_deleting_and_duplicating_timelines() {
    let mut p = project();
    let original = json(&p);
    let mut h = History::new();
    step!(h, p, link(&p, &ids(&["a", "m"])).unwrap());
    step!(
        h,
        p,
        build::rename(&p, MAIN_SEQUENCE_ID, "Long cut").unwrap()
    );
    assert_eq!(p.sequence.name, "Long cut");

    let (cmd, copy) = build::duplicate_timeline(&p, MAIN_SEQUENCE_ID).unwrap();
    step!(h, p, cmd);
    assert_eq!(p.sequence.id, MAIN_SEQUENCE_ID, "a duplicate does not open");
    let dup = p.materials.sequence(&copy).unwrap().clone();
    assert_eq!(dup.name, "Long cut copy");
    let originals: Vec<&str> = p
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter().map(|s| s.id.as_str()))
        .collect();
    for s in dup.tracks.iter().flat_map(|t| t.segments.iter()) {
        assert!(!originals.contains(&s.id.as_str()), "new clip ids");
    }
    // The copy's linked pair has its own registered group.
    let group_a = p.link_group_of("a").unwrap().clone();
    let copied_groups: Vec<&String> = dup
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .filter_map(|s| p.materials.link_of(s))
        .collect();
    assert_eq!(copied_groups.len(), 2);
    assert!(copied_groups.iter().all(|g| **g != group_a));

    // Delete the open timeline: its neighbour opens.
    step!(h, p, build::delete_timeline(&p, MAIN_SEQUENCE_ID).unwrap());
    assert_eq!(p.sequence.id, copy);
    assert_eq!(timelines(&p).len(), 1);
    let refused = build::delete_timeline(&p, &copy).unwrap_err();
    assert!(refused.contains("at least one"));

    while h.can_undo() {
        h.undo(&mut p).unwrap();
    }
    assert_eq!(json(&p), original);
}

#[test]
fn the_export_sees_the_whole_timeline_from_inside_a_compound_clip() {
    let mut p = project();
    let made = build::create_compound(&p, &ids(&["b", "o"]), None).unwrap();
    made.command.apply(&mut p).unwrap();
    build::open(&p, &made.segment_id)
        .unwrap()
        .apply(&mut p)
        .unwrap();
    assert_eq!(p.duration(), 3 * S, "the compound's own length");
    let root = export_root(p.clone());
    assert_eq!(root.sequence.id, MAIN_SEQUENCE_ID);
    assert!(root.sequence.path.is_empty());
    assert_eq!(root.duration(), 7 * S);
    assert!(errors(&root).is_empty());
}

#[test]
fn sound_inside_a_compound_lands_where_it_plays_on_the_timeline() {
    let mut p = project();
    // A volume ramp on the music, then the music into a compound clip.
    p.tracks[2].segments[0].keyframes.push(KeyframeTrack {
        property: AnimatableProperty::Volume,
        keyframes: vec![
            Keyframe {
                time: 0,
                value: 0.0,
                easing: Default::default(),
            },
            Keyframe {
                time: 2 * S,
                value: 1.0,
                easing: Default::default(),
            },
        ],
    });
    p.tracks[2].volume = 0.5;
    let made = build::create_compound(&p, &ids(&["m"]), None).unwrap();
    made.command.apply(&mut p).unwrap();
    // The compound (1..5 s) shows nested 1..3 s at double speed from 10 s:
    // the music's 1..3 s plays in 10..11 s.
    let lane_volume = 0.8;
    let compound = p.segment_mut(&made.segment_id).unwrap();
    compound.target_range = TimeRange::new(10 * S, S);
    compound.source_range = TimeRange::new(S, 2 * S);
    compound.speed = 2.0;
    compound.volume = 0.5;
    let at = p
        .tracks
        .iter()
        .position(|t| t.segments.iter().any(|s| s.id == made.segment_id))
        .unwrap();
    p.tracks[at].volume = lane_volume;

    let flat = audio::flatten_audio(&p);
    let heard: Vec<&Segment> = flat
        .tracks
        .iter()
        .filter(|t| t.id.starts_with(&made.segment_id))
        .flat_map(|t| t.segments.iter())
        .collect();
    assert_eq!(heard.len(), 1);
    let m = heard[0];
    assert_eq!(m.target_range, TimeRange::new(10 * S, S));
    assert_eq!(m.source_range, TimeRange::new(S, 2 * S));
    assert_eq!(m.speed, 2.0);
    assert_eq!(m.volume, 0.5);
    let lane = flat
        .tracks
        .iter()
        .find(|t| t.id.starts_with(&made.segment_id))
        .unwrap();
    assert!((lane.volume - 0.5 * lane_volume).abs() < 1e-6);
    // The ramp moved with the cut (1 s in) and the speed (halved).
    let keys = &m.keyframes[0].keyframes;
    assert_eq!(keys[0].time, -S / 2);
    assert_eq!(keys[1].time, S / 2);

    // The preview mixer plans the same clip.
    let planned = crate::modules::audio::mixer::plan(&p);
    let mine = planned
        .iter()
        .find(|s| s.segment_id.starts_with(&made.segment_id))
        .expect("planned");
    assert_eq!(mine.target, TimeRange::new(10 * S, S));
    assert_eq!(mine.source_start, S);
    assert!((mine.speed - 2.0).abs() < 1e-9);
}

#[test]
fn a_project_without_compound_clips_is_not_copied_for_the_mixer() {
    let p = project();
    assert!(matches!(
        audio::flatten_audio(&p),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn deleting_a_timeline_takes_the_compound_clips_only_it_showed() {
    let mut p = project();
    let original = json(&p);
    let mut h = History::new();
    // A compound inside a compound on the main timeline, and a second
    // compound clip that is cut away and waits to be pasted.
    let inner = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    step!(h, p, inner.command);
    let outer = build::create_compound(&p, &ids(&[&inner.segment_id, "c"]), None).unwrap();
    step!(h, p, outer.command);
    let waiting = build::create_compound(&p, &ids(&["o"]), None).unwrap();
    step!(h, p, waiting.command);
    let (lane, clip) = p.segment(&waiting.segment_id).unwrap();
    let (lane, clip) = (lane.id.clone(), clip.clone());
    let index = p.track(&lane).unwrap().segments.len() - 1;
    apply(
        &mut h,
        &mut p,
        EditCommand::RemoveSegment {
            track_id: lane,
            segment: clip,
            index,
        },
    );
    let (cmd, _) = build::new_timeline(&p, None).unwrap();
    step!(h, p, cmd);
    let before = json(&p);

    step!(h, p, build::delete_timeline(&p, MAIN_SEQUENCE_ID).unwrap());
    assert!(p.materials.sequence(&outer.sequence_id).is_none());
    assert!(p.materials.sequence(&inner.sequence_id).is_none());
    assert!(
        p.materials.sequence(&waiting.sequence_id).is_some(),
        "a cut compound clip can still be pasted"
    );

    h.undo(&mut p).unwrap();
    assert_eq!(json(&p), before);
    while h.can_undo() {
        h.undo(&mut p).unwrap();
    }
    assert_eq!(json(&p), original);
}

#[test]
fn a_compound_used_elsewhere_survives_its_timeline() {
    let mut p = project();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    step!(h, p, made.command);
    let clip = p.segment(&made.segment_id).unwrap().1.clone();
    let (cmd, second) = build::new_timeline(&p, None).unwrap();
    step!(h, p, cmd);
    // The same compound clip on the second timeline, as a paste puts it.
    let mut copy = clip;
    copy.id = "pasted".into();
    copy.target_range.start = 0;
    let lane = p.tracks[0].id.clone();
    apply(
        &mut h,
        &mut p,
        EditCommand::InsertSegment {
            track_id: lane,
            segment: copy,
            index: 0,
        },
    );
    step!(h, p, build::delete_timeline(&p, MAIN_SEQUENCE_ID).unwrap());
    assert_eq!(p.sequence.id, second);
    assert!(p.materials.sequence(&made.sequence_id).is_some());
}

/// The music (1..5 s) in a compound clip, the compound clip moved to 10 s.
fn music_compound() -> (Project, String) {
    let mut p = project();
    let made = build::create_compound(&p, &ids(&["m"]), None).unwrap();
    made.command.apply(&mut p).unwrap();
    let compound = p.segment_mut(&made.segment_id).unwrap();
    compound.target_range.start = 10 * S;
    (p, made.segment_id)
}

fn heard(p: &Project, compound: &str) -> Vec<Segment> {
    audio::flatten_audio(p)
        .tracks
        .iter()
        .filter(|t| t.id.starts_with(compound))
        .flat_map(|t| t.segments.iter().cloned())
        .collect()
}

fn volume_keys(values: &[(Micros, f32)]) -> KeyframeTrack {
    KeyframeTrack {
        property: AnimatableProperty::Volume,
        keyframes: values
            .iter()
            .map(|&(time, value)| Keyframe {
                time,
                value,
                easing: Default::default(),
            })
            .collect(),
    }
}

#[test]
fn a_compound_clips_own_volume_keyframes_reach_its_sound() {
    let (mut p, compound) = music_compound();
    // A fade over the compound clip's first second; music starts 0 s inside.
    p.segment_mut(&compound)
        .unwrap()
        .keyframes
        .push(volume_keys(&[(0, 0.0), (S, 1.0)]));
    let m = &heard(&p, &compound)[0];
    assert_eq!(m.target_range.start, 10 * S);
    let keys = &m.keyframes[0];
    assert_eq!(keys.sample(0), Some(0.0));
    assert_eq!(keys.sample(S / 2), Some(0.5));
    assert_eq!(keys.sample(2 * S), Some(1.0));

    // With a ramp of the music's own as well, the two multiply.
    let inner = p.materials.sequences[0].tracks[0].segments[0]
        .keyframes
        .clone();
    assert!(inner.is_empty());
    p.materials.sequences[0].tracks[0].segments[0]
        .keyframes
        .push(volume_keys(&[(0, 1.0), (4 * S, 0.0)]));
    let m = &heard(&p, &compound)[0];
    let product = |t: Micros| m.keyframes[0].sample(t).unwrap();
    for t in [0, S / 4, S / 2, S, 2 * S, 3 * S] {
        let own = (t as f32 / S as f32).min(1.0);
        let music = 1.0 - t as f32 / (4 * S) as f32;
        assert!(
            (product(t) - own * music).abs() < 0.01,
            "at {t}: {} against {}",
            product(t),
            own * music
        );
    }
}

#[test]
fn a_speed_curve_on_a_compound_clip_reaches_its_sound() {
    use crate::modules::project::{SpeedCurveMaterial, SpeedPoint};
    let (mut p, compound) = music_compound();
    // Half speed for the first second of the contents, double after.
    let points = vec![
        SpeedPoint {
            source: 0,
            speed: 0.5,
        },
        SpeedPoint {
            source: 2 * S,
            speed: 2.0,
        },
    ];
    let source = p.segment(&compound).unwrap().1.source_range;
    let length = crate::modules::project::speed::curve_target_duration(&points, source);
    p.materials.speed_curves.push(SpeedCurveMaterial {
        id: "ramp".into(),
        preset: None,
        points,
    });
    let clip = p.segment_mut(&compound).unwrap();
    clip.extras.push("ramp".into());
    clip.target_range.duration = length;
    assert!(errors(&p).is_empty(), "{:?}", errors(&p));

    let flat = audio::flatten_audio(&p);
    let m = flat
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .find(|s| s.id.starts_with(&compound))
        .unwrap();
    // The whole compound clip, which is the whole music clip.
    assert_eq!(m.target_range, TimeRange::new(10 * S, length));
    assert_eq!(m.source_range, TimeRange::new(0, 4 * S));
    let curve = flat
        .materials
        .speed_curve_of(m)
        .expect("the music is ramped");
    // The music starts at 0 inside and reads its file from 0 at 1x: the
    // compound clip's curve, point for point.
    assert_eq!(curve.points[0].source, 0);
    assert_eq!(curve.points[1].source, 2 * S);
    assert_eq!(curve.points[1].speed, 2.0);
    let map = flat.materials.time_map(m);
    assert_eq!(map.offset_of(4 * S), length);
}

#[test]
fn a_compound_clip_at_another_speed_flattens_at_that_speed() {
    let mut p = project();
    let mut h = History::new();
    let made = build::create_compound(&p, &ids(&["b", "o"]), None).unwrap();
    step!(h, p, made.command);
    // 2..5 s at double speed: 2..3.5 s.
    let compound = p.segment_mut(&made.segment_id).unwrap();
    compound.speed = 2.0;
    compound.target_range.duration = 3 * S / 2;
    assert!(errors(&p).is_empty(), "{:?}", errors(&p));
    step!(h, p, build::flatten(&p, &made.segment_id).unwrap());
    // b was 2..5 s reading 10..13 s; o was 3..4 s reading 30..31 s.
    let b = p.segment("b").unwrap().1;
    assert_eq!(b.target_range, TimeRange::new(2 * S, 3 * S / 2));
    assert_eq!(b.source_range, TimeRange::new(10 * S, 3 * S));
    assert_eq!(b.speed, 2.0);
    let o = p.segment("o").unwrap().1;
    assert_eq!(o.target_range, TimeRange::new(5 * S / 2, S / 2));
    assert_eq!(o.source_range, TimeRange::new(30 * S, S));
}

#[test]
fn a_curve_inside_a_curve_is_refused_with_a_reason() {
    use crate::modules::project::{SpeedCurveMaterial, SpeedPoint};
    let mut p = project();
    let made = build::create_compound(&p, &ids(&["b"]), None).unwrap();
    made.command.apply(&mut p).unwrap();
    let flat_curve = |id: &str, speed: f32| SpeedCurveMaterial {
        id: id.into(),
        preset: None,
        points: vec![SpeedPoint { source: 0, speed }],
    };
    p.materials.speed_curves.push(flat_curve("inner", 1.0));
    p.materials.speed_curves.push(flat_curve("outer", 1.0));
    p.materials.sequences[0].tracks[0].segments[0]
        .extras
        .push("inner".into());
    p.segment_mut(&made.segment_id)
        .unwrap()
        .extras
        .push("outer".into());
    let refused = build::flatten(&p, &made.segment_id).unwrap_err();
    assert!(
        refused.contains("remove one of the two curves"),
        "{refused}"
    );
}
