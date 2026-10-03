//! Splitting a clip that carries everything the features add to one.
//!
//! Each feature tested its own split: the motion agent that an animation's
//! entrance stays left, tracking that a follower stays on its object, captions
//! that words divide, colour that a grade is shared. This test is the seam
//! between them: one document where a clip has a grade, an animation,
//! keyframes, a transition, a linked sound, a follower and captions above it,
//! and a few hundred seeded splits — of one clip, and of every lane at once —
//! each checked for:
//!
//! - `validate()` finds no error;
//! - every id a clip's `extras` names resolves to a material in the pool;
//! - every caption word is still on the timeline instant it was spoken, and
//!   none was lost or doubled;
//! - every follower piece still resolves its follow link;
//! - one undo restores the document byte for byte, and redo the split one.

use chukcut_engine::modules::captions::{
    self,
    edit::{cues, place, PlaceOptions},
    CaptionStyle, Cue, TimedWord,
};
use chukcut_engine::modules::inspector::edit::{grade_control_command, GradeControl};
use chukcut_engine::modules::motion;
use chukcut_engine::modules::project::animation::{AnimationPreset, ClipAnimation};
use chukcut_engine::modules::project::document::{
    AnimatableProperty, AudioMaterial, CanvasConfig, Easing, Keyframe, Micros, Project, Segment,
    Severity, TextMaterial, TimeRange, Track, TrackKind, Transform, TransitionKind, VideoMaterial,
};
use chukcut_engine::modules::timeline::ops::{split_all_at, split_at, EditCommand};
use chukcut_engine::modules::timeline::History;
use chukcut_engine::modules::tracking::{self, FollowMode, TrackSample, TrackSettings};
use chukcut_engine::modules::transitions;

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

fn apply(project: &mut Project, command: EditCommand) {
    command.apply(project).expect("the setup edit applies");
}

/// Three cuts of one take on the main lane with transitions between them, the
/// first linked to its sound; a grade, an animation and keyframes on them; a
/// title following a track through the first; captions over all of it.
fn document() -> Project {
    let mut p = Project::new("glue", CanvasConfig::default(), 30.0);
    p.materials.videos.push(VideoMaterial {
        id: "take".into(),
        path: "/nonexistent/take.mp4".into(),
        width: 1920,
        height: 1080,
        duration: 60 * S,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
    });
    p.materials.audios.push(AudioMaterial {
        id: "music".into(),
        path: "/nonexistent/music.mp3".into(),
        duration: 60 * S,
        sample_rate: 48_000,
        channels: 2,
    });
    let mut v = Track::new(TrackKind::Video, "V1");
    v.segments = vec![
        segment("c1", "take", 0, 4 * S, 0),
        segment("c2", "take", 4 * S, 4 * S, 10 * S),
        segment("c3", "take", 8 * S, 4 * S, 20 * S),
    ];
    let mut a = Track::new(TrackKind::Audio, "A1");
    a.segments = vec![segment("a1", "take", 0, 4 * S, 0)];
    let mut m = Track::new(TrackKind::Audio, "Music");
    m.segments = vec![segment("m1", "music", 0, 12 * S, 0)];
    let mut t = Track::new(TrackKind::Text, "Titles");
    p.materials.texts.push(TextMaterial {
        content: "look".into(),
        ..captions::edit::material_for(
            &Cue::new(0, S, "look"),
            &CaptionStyle::default_for(&p.canvas),
            false,
        )
    });
    p.materials.texts[0].caption = None;
    p.materials.texts[0].id = "title".into();
    t.segments = vec![segment("follower", "title", S, 5 * S, 0)];
    p.tracks = vec![t, v, a, m];

    // Linked picture and sound.
    p.materials.links.insert("g".into());
    for id in ["c1", "a1"] {
        apply(
            &mut p,
            EditCommand::SetLinkGroup {
                segment_id: id.into(),
                before: None,
                after: Some("g".into()),
            },
        );
    }
    // Transitions on both cuts.
    for id in ["c2", "c3"] {
        let add = transitions::edit::add_command(&p, id, TransitionKind::Dissolve, Some(S / 2))
            .expect("a transition fits");
        apply(&mut p, add);
    }
    // A grade on the middle clip.
    let (material, grade) =
        grade_control_command(&p, "c2", GradeControl::Brightness, 0.3).expect("grade");
    p.materials.color_adjusts.extend(material);
    apply(&mut p, grade);
    // In and out animations on the first clip, keyframes on the last.
    let animate = motion::edit::edit_command(&p, "c1", |m| {
        let slot = |preset| ClipAnimation {
            preset,
            duration: S / 2,
            easing: Default::default(),
            strength: 1.0,
        };
        m.intro = Some(slot(AnimationPreset::Fade));
        m.outro = Some(slot(AnimationPreset::SlideLeft));
    })
    .expect("animation");
    apply(&mut p, animate);
    for (time, value) in [(0, 0.0), (2 * S, 1.0), (4 * S, 0.5)] {
        apply(
            &mut p,
            EditCommand::AddKeyframe {
                segment_id: "c3".into(),
                property: AnimatableProperty::Opacity,
                keyframe: Keyframe {
                    time,
                    value,
                    easing: Easing::Linear,
                },
            },
        );
    }
    // A track over the take, and the title following it through the first clip.
    let samples = (0..=200)
        .map(|i| TrackSample {
            t: i * 100_000,
            x: 0.2 + i as f32 / 400.0,
            y: 0.5,
            w: 0.1,
            h: 0.1,
            a: 0.0,
            c: 1.0,
            f: 0,
        })
        .collect();
    let mut track =
        tracking::TrackingMaterial::new("take".into(), TrackSettings::default(), samples);
    track.id = "track".into();
    tracking::edit::add_track(track).apply(&mut p).unwrap();
    tracking::edit::attach(&p, "follower", "track", "c1", FollowMode::Position, S)
        .unwrap()
        .apply(&mut p)
        .unwrap();
    // Captions with word times across the whole take.
    let words: Vec<TimedWord> = (0..22)
        .map(|i| TimedWord {
            text: format!("w{i}"),
            start: i * S / 2 + 50_000,
            end: i * S / 2 + 350_000,
        })
        .collect();
    let cues: Vec<Cue> = words
        .chunks(3)
        .map(|chunk| Cue {
            start: chunk[0].start - 50_000,
            end: chunk.last().unwrap().end + 100_000,
            text: chunk
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            words: chunk.to_vec(),
        })
        .collect();
    let placed = place(
        &p,
        &cues,
        &CaptionStyle::default_for(&p.canvas),
        PlaceOptions::default(),
    )
    .expect("captions");
    p.materials.texts.extend(placed.materials);
    apply(&mut p, placed.command);

    assert_no_errors(&p, "the starting document");
    p
}

fn assert_no_errors(p: &Project, context: &str) {
    let errors: Vec<_> = p
        .validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{context}: {errors:?}");
}

/// Every caption word as (text, timeline start), sorted.
fn spoken(p: &Project) -> Vec<(String, Micros)> {
    let mut words: Vec<_> = cues(p)
        .into_iter()
        .flat_map(|c| c.words)
        .map(|w| (w.text, w.start))
        .collect();
    words.sort();
    words
}

fn check_references(p: &Project, context: &str) {
    for track in &p.tracks {
        for s in &track.segments {
            for id in &s.extras {
                let known = p.materials.links.contains(id)
                    || p.materials.transition(id).is_some()
                    || p.materials.animation(id).is_some()
                    || p.materials.color_adjust(id).is_some()
                    || p.materials.follow(id).is_some();
                assert!(known, "{context}: clip {} names unknown extra {id}", s.id);
            }
        }
    }
}

fn check_followers(p: &Project, context: &str) {
    for track in &p.tracks {
        for s in &track.segments {
            if p.materials.follow_of(s).is_some() {
                let mid = s.target_range.start + s.target_range.duration / 2;
                assert!(
                    tracking::follow::followed_transform(p, s, mid).is_some(),
                    "{context}: follower piece {} lost its object",
                    s.id
                );
            }
        }
    }
}

/// Where two serialised documents first differ, with some context.
fn first_difference(a: &str, b: &str) -> String {
    let at = a
        .bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let from = at.saturating_sub(300);
    format!(
        "at byte {at}:\n got: {}\nwant: {}",
        &a[from..(at + 200).min(a.len())],
        &b[from..(at + 200).min(b.len())]
    )
}

/// A small deterministic generator, so a failure names a reproducible case.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

#[test]
fn splits_keep_every_feature_consistent_and_undo_exactly() {
    let start = document();
    let words = spoken(&start);
    let mut rng = Lcg(0x5eed_91ce);
    let mut cases = 0;
    // Three splits deep per round, so halves of halves are split too.
    for round in 0..120 {
        let mut p = start.clone();
        let mut history = History::new();
        let mut snapshots = vec![serde_json::to_string(&p).unwrap()];
        for depth in 0..3 {
            let all: Vec<(String, TimeRange, bool)> = p
                .tracks
                .iter()
                .flat_map(|t| t.segments.iter().map(move |s| (t, s)))
                .map(|(t, s)| (s.id.clone(), s.target_range, t.locked))
                .collect();
            let (id, range, _) = &all[rng.below(all.len() as u64) as usize];
            let at = range.start + 1 + rng.below(range.duration as u64 - 1) as Micros;
            let every_lane = rng.below(4) == 0;
            let context = format!(
                "round {round} depth {depth}: {} at {at}",
                if every_lane { "split all" } else { id.as_str() }
            );
            let command = if every_lane {
                split_all_at(&p, at)
            } else {
                split_at(&p, id, at)
            };
            let Ok(command) = command else { continue };
            history.apply(&mut p, command).expect(&context);
            cases += 1;
            assert_no_errors(&p, &context);
            check_references(&p, &context);
            check_followers(&p, &context);
            assert_eq!(spoken(&p), words, "{context}: a caption word moved");
            snapshots.push(serde_json::to_string(&p).unwrap());
        }
        // Undo walks back through every state exactly, redo forward again.
        let last = snapshots.pop().unwrap();
        while history.can_undo() {
            history.undo(&mut p).unwrap();
            let want = snapshots.pop().unwrap();
            let got = serde_json::to_string(&p).unwrap();
            assert!(
                got == want,
                "round {round}: undo differs {}",
                first_difference(&got, &want)
            );
        }
        while history.can_redo() {
            history.redo(&mut p).unwrap();
        }
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            last,
            "round {round}: redo"
        );
    }
    assert!(cases > 250, "only {cases} splits were made");
}
