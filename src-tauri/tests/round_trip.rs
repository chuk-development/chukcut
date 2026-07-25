//! Save and load.
//!
//! A project file is the only artefact of an editing session that outlives the
//! process. If any field of the document does not survive a trip through JSON,
//! the user loses work — silently, and usually only noticing after the next
//! time they open the file. So these tests build a document that uses every
//! part of the schema and check that reading it back produces bytes identical
//! to what was written.
//!
//! "Byte identical on a second save" is the assertion that matters, and it is
//! stronger than comparing structs: it also catches a field that deserializes
//! to a *different* default than the one it was written with, which a
//! field-by-field comparison of the parts somebody remembered to check would
//! miss.

mod support;

use chukcut_lib::modules::project::document::{
    AnimatableProperty, AudioMaterial, CanvasConfig, Crop, Easing, ImageMaterial, Keyframe,
    KeyframeTrack, MaterialKind, Micros, Project, Segment, TextAlign, TextMaterial, TextShadow,
    TimeRange, Track, TrackKind, Transform, VideoMaterial, SCHEMA_VERSION,
};

use support::{canonical, Lcg};

/// Serialize exactly the way `project::commands::write_project` does.
fn save(project: &Project) -> String {
    serde_json::to_string_pretty(project).expect("a project always serializes")
}

fn load(json: &str) -> Project {
    serde_json::from_str(json).expect("a saved project always parses")
}

/// A document that exercises every part of the schema: all four material
/// kinds, every track kind, transforms, crops, keyframes on several
/// properties, extras, speed and volume, and fifty segments.
fn full_document() -> Project {
    let mut rng = Lcg::new(0xC0FFEE);
    let mut project = Project::new(
        "Every field",
        CanvasConfig {
            width: 1080,
            height: 1920,
            background: [0.1, 0.2, 0.3, 1.0],
        },
        29.97,
    );
    project.created_at = 1_700_000_000_000;
    project.updated_at = 1_700_000_123_456;

    for i in 0..4 {
        project.materials.videos.push(VideoMaterial {
            id: format!("video-{i}"),
            path: format!("/media/clip {i}.mp4"),
            width: 1920,
            height: 1080,
            duration: 30_000_000,
            fps: 29.97,
            has_audio: i % 2 == 0,
            rotation: [0, 90, 180, 270][i as usize % 4],
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
        content: "Ünïcödé — with an em dash and a \"quote\"".into(),
        font_family: "Inter".into(),
        font_size: 64.0,
        color: [1.0, 1.0, 1.0, 1.0],
        bold: true,
        italic: false,
        align: TextAlign::Right,
        stroke_width: 2.5,
        stroke_color: [0.0, 0.0, 0.0, 1.0],
        shadow: Some(TextShadow {
            color: [0.0, 0.0, 0.0, 0.5],
            offset: [4.0, 4.0],
            blur: 8.0,
        }),
        background: Some([0.0, 0.0, 0.0, 0.25]),
    });
    project.materials.extras.insert(
        "effect-0".into(),
        serde_json::json!({ "kind": "blur", "radius": 12.5, "nested": { "a": [1, 2, 3] } }),
    );
    project.materials.extras.insert(
        "effect-1".into(),
        serde_json::json!({ "kind": "transition", "duration": 500_000 }),
    );

    let material_ids = [
        "video-0", "video-1", "video-2", "video-3", "audio-0", "image-0", "text-0",
    ];

    for (track_index, kind) in [
        TrackKind::Video,
        TrackKind::Video,
        TrackKind::Audio,
        TrackKind::Text,
        TrackKind::Sticker,
    ]
    .into_iter()
    .enumerate()
    {
        let mut track = Track::new(kind, format!("Lane {track_index}"));
        track.id = format!("track-{track_index}");
        track.muted = track_index % 2 == 0;
        track.locked = track_index == 3;
        track.hidden = track_index == 4;
        track.volume = 0.5 + track_index as f32 * 0.1;

        // Ten segments per track, laid end to end with a gap, so the document
        // is fifty segments in total and stays a valid one.
        let mut at: Micros = track_index as Micros * 250_000;
        for segment_index in 0..10 {
            let duration = 1_000_000 + (segment_index as Micros % 3) * 250_000;
            let mut segment = Segment {
                id: format!("segment-{track_index}-{segment_index}"),
                material_id: material_ids[(track_index + segment_index) % material_ids.len()]
                    .to_string(),
                target_range: TimeRange::new(at, duration),
                source_range: TimeRange::new(segment_index as Micros * 500_000, duration),
                render_index: track_index as i32,
                speed: 0.5 + segment_index as f32 * 0.25,
                volume: rng.unit() * 2.0,
                transform: Transform {
                    position: [rng.unit() - 0.5, rng.unit() - 0.5],
                    scale: [0.5 + rng.unit(), 0.5 + rng.unit()],
                    rotation: rng.between(-180, 180) as f32,
                    opacity: rng.unit(),
                    flip_h: segment_index % 2 == 0,
                    flip_v: segment_index % 3 == 0,
                },
                crop: (segment_index % 4 == 0).then_some(Crop {
                    left: 0.1,
                    top: 0.2,
                    right: 0.85,
                    bottom: 0.95,
                }),
                extras: if segment_index % 5 == 0 {
                    vec!["effect-0".into(), "effect-1".into()]
                } else {
                    Vec::new()
                },
                keyframes: Vec::new(),
            };

            if segment_index % 2 == 0 {
                segment.keyframes = vec![
                    KeyframeTrack {
                        property: AnimatableProperty::Opacity,
                        keyframes: vec![
                            Keyframe {
                                time: 0,
                                value: 0.0,
                                easing: Easing::EaseIn,
                            },
                            Keyframe {
                                time: duration / 2,
                                value: 1.0,
                                easing: Easing::Hold,
                            },
                            Keyframe {
                                time: duration,
                                value: 0.25,
                                easing: Easing::EaseInOut,
                            },
                        ],
                    },
                    KeyframeTrack {
                        property: AnimatableProperty::PositionX,
                        keyframes: vec![
                            Keyframe {
                                time: 0,
                                value: -1.0,
                                easing: Easing::Linear,
                            },
                            Keyframe {
                                time: duration,
                                value: 1.0,
                                easing: Easing::EaseOut,
                            },
                        ],
                    },
                    KeyframeTrack {
                        property: AnimatableProperty::Volume,
                        keyframes: vec![Keyframe {
                            time: 0,
                            value: 0.8,
                            easing: Easing::Linear,
                        }],
                    },
                ];
            }

            at += duration + 100_000;
            track.segments.push(segment);
        }
        project.tracks.push(track);
    }

    project
}

#[test]
fn a_document_using_every_field_survives_a_save_and_load_unchanged() {
    let original = full_document();
    assert_eq!(
        original.tracks.iter().map(|t| t.segments.len()).sum::<usize>(),
        50,
        "the fixture is supposed to be fifty segments"
    );

    let first = save(&original);
    let reloaded = load(&first);
    let second = save(&reloaded);

    assert_eq!(
        first, second,
        "saving a project that was just loaded produced different bytes"
    );
}

#[test]
fn a_reloaded_document_is_the_same_document() {
    let original = full_document();
    let reloaded = load(&save(&original));

    // Byte equality of the second save proves the file is stable; this proves
    // the *structure* survived, so a field that round-trips through JSON but
    // lands somewhere else cannot hide.
    assert_eq!(canonical(&original), canonical(&reloaded));

    assert_eq!(reloaded.id, original.id);
    assert_eq!(reloaded.schema_version, SCHEMA_VERSION);
    assert_eq!(reloaded.name, "Every field");
    assert_eq!(reloaded.fps, 29.97);
    assert_eq!(reloaded.created_at, 1_700_000_000_000);
    assert_eq!(reloaded.canvas.width, 1080);
    assert_eq!(reloaded.canvas.background, [0.1, 0.2, 0.3, 1.0]);
    assert_eq!(reloaded.duration(), original.duration());

    assert_eq!(reloaded.materials.videos.len(), 4);
    assert_eq!(reloaded.materials.audios.len(), 1);
    assert_eq!(reloaded.materials.images.len(), 1);
    assert_eq!(reloaded.materials.texts.len(), 1);
    assert_eq!(reloaded.materials.extras.len(), 2);
    assert_eq!(
        reloaded.materials.extras["effect-0"]["nested"]["a"],
        serde_json::json!([1, 2, 3]),
        "an opaque extras blob is stored verbatim, not flattened"
    );
    assert_eq!(reloaded.materials.kind_of("text-0"), Some(MaterialKind::Text));
    assert_eq!(
        reloaded.materials.video("video-1").map(|m| m.rotation),
        Some(90)
    );

    let text = reloaded.materials.texts[0].clone();
    assert!(text.content.contains('Ü') && text.content.contains('—'));
    assert_eq!(text.align, TextAlign::Right);
    assert!(text.bold && !text.italic);
    assert_eq!(text.shadow.expect("shadow").blur, 8.0);
}

#[test]
fn keyframes_survive_with_their_times_easings_and_order() {
    let reloaded = load(&save(&full_document()));

    let segment = reloaded
        .segment("segment-0-0")
        .map(|(_, s)| s.clone())
        .expect("the fixture has this segment");
    assert_eq!(segment.keyframes.len(), 3);

    let opacity = segment
        .keyframes
        .iter()
        .find(|k| k.property == AnimatableProperty::Opacity)
        .expect("an opacity track");
    assert_eq!(opacity.keyframes.len(), 3);
    assert_eq!(opacity.keyframes[0].easing, Easing::EaseIn);
    assert_eq!(opacity.keyframes[1].easing, Easing::Hold);
    assert_eq!(opacity.keyframes[2].value, 0.25);

    // The sampler is the thing that has to keep working, so check it agrees
    // across the round trip rather than only checking the numbers.
    let duration = segment.target_range.duration;
    assert_eq!(opacity.sample(0), Some(0.0));
    assert_eq!(
        opacity.sample(duration / 2),
        Some(1.0),
        "the middle keyframe is exact"
    );
    assert_eq!(
        opacity.sample(duration / 2 + 1),
        Some(1.0),
        "and Hold keeps it there until the next one"
    );
    assert_eq!(opacity.sample(duration), Some(0.25));
}

#[test]
fn a_document_with_no_optional_fields_loads_with_the_documented_defaults() {
    // What a hand-written or older project file looks like: every `#[serde(
    // default)]` field absent. Getting a default wrong here silently changes
    // how an existing project renders.
    let minimal = serde_json::json!({
        "id": "p1",
        "schema_version": 1,
        "name": "Minimal",
        "created_at": 0,
        "updated_at": 0,
        "canvas": { "width": 640, "height": 480, "background": [0.0, 0.0, 0.0, 1.0] },
        "fps": 30.0,
        "materials": {},
        "tracks": [{
            "id": "t1",
            "kind": "video",
            "name": "V1",
            "segments": [{
                "id": "s1",
                "material_id": "m1",
                "target_range": { "start": 0, "duration": 1000000 },
                "source_range": { "start": 0, "duration": 1000000 },
                "render_index": 0
            }]
        }]
    })
    .to_string();

    let project: Project = serde_json::from_str(&minimal).expect("a minimal project parses");

    let track = &project.tracks[0];
    assert!(!track.muted && !track.locked && !track.hidden);
    assert_eq!(track.volume, 1.0, "a missing track volume is unity, not zero");

    let segment = &track.segments[0];
    assert_eq!(segment.speed, 1.0, "a missing speed is 1x, not zero");
    assert_eq!(segment.volume, 1.0);
    assert_eq!(segment.transform.opacity, 1.0);
    assert_eq!(segment.transform.scale, [1.0, 1.0]);
    assert!(segment.crop.is_none());
    assert!(segment.extras.is_empty());
    assert!(segment.keyframes.is_empty());

    assert!(project.materials.videos.is_empty());
    assert!(project.materials.extras.is_empty());

    // And once loaded it saves and reloads stably like any other document.
    assert_eq!(save(&project), save(&load(&save(&project))));
}

#[test]
fn a_project_survives_a_trip_through_the_file_system() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("round_trip");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    let file = dir.join("project.chukcut");

    let original = full_document();
    std::fs::write(&file, save(&original)).expect("write");
    let raw = std::fs::read_to_string(&file).expect("read");
    let reloaded: Project = serde_json::from_str(&raw).expect("parse");

    assert_eq!(canonical(&original), canonical(&reloaded));
    // Pretty-printed on purpose: the format is meant to be diffable, and a
    // switch to compact output would be a silent change to every project file.
    assert!(
        raw.contains("\n  \"schema_version\""),
        "projects are saved as indented JSON"
    );

    std::fs::remove_file(&file).ok();
}

#[test]
fn a_saved_document_is_structurally_valid_when_it_is_read_back() {
    let reloaded = load(&save(&full_document()));

    // Warnings are expected — the fixture's media does not exist — but an
    // error means an edit produced an inconsistent document, and one that
    // reached disk is a document the app will keep reopening.
    let errors: Vec<String> = reloaded
        .validate()
        .into_iter()
        .filter(|i| i.severity == chukcut_lib::modules::project::document::Severity::Error)
        .map(|i| i.message)
        .collect();
    assert!(errors.is_empty(), "validation errors after a round trip: {errors:?}");
}

#[test]
fn compositing_order_is_the_same_after_a_round_trip() {
    let original = full_document();
    let reloaded = load(&save(&original));

    // `segments_at` sorts by render_index, and render_index is stored rather
    // than derived at load time, so a lost field would restack the picture.
    for time in [0, 500_000, 3_000_000, 12_000_000] {
        let before: Vec<(&str, i32)> = original
            .segments_at(time)
            .into_iter()
            .map(|(_, s)| (s.id.as_str(), s.render_index))
            .collect();
        let after: Vec<(&str, i32)> = reloaded
            .segments_at(time)
            .into_iter()
            .map(|(_, s)| (s.id.as_str(), s.render_index))
            .collect();
        assert_eq!(before, after, "at {time} µs");
    }
}
