//! Colour AI from the command line: auto adjust, colour match, grade
//! presets — each one undo step, saved into the project file — and the
//! face and body tools' settings.

mod common;

use common::{ok, run};

#[test]
fn auto_adjust_colour_match_and_presets() {
    require_ffmpeg!();
    let dir = common::scratch("colour-ai");
    let (card, bars) = common::media(&dir);
    let project = dir.join("colour.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    ok(&dir, &["import", p, bars.to_str().unwrap(), "--append"]);

    // Auto adjust writes ordinary grade controls.
    let data = ok(&dir, &["auto-adjust", p, "0:0", "--amount", "0.8"]);
    assert!(data["controls"]["exposure"].is_number(), "{data}");
    assert!(data["before"]["key"].is_number() && data["after"]["key"].is_number());
    let out = run(&dir, &["auto-adjust", p, "0:0", "--amount", "2"]);
    assert_eq!(out.code, 2, "{}", out.json);

    // Colour match: the bars to the card.
    let data = ok(&dir, &["colour-match", p, "0:1", "--to", "0:0"]);
    let before = data["distance_before"].as_f64().unwrap();
    let after = data["distance_after"].as_f64().unwrap();
    assert!(after < before * 0.5, "{before} -> {after}");
    // One frame of the reference.
    ok(
        &dir,
        &["colour-match", p, "0:1", "--to", "0:0", "--at", "1.5"],
    );

    // Presets: save, list, apply, remove.
    let saved = ok(&dir, &["grade-preset-save", p, "0:1", "--name", "Bars fix"]);
    assert_eq!(saved["preset"]["name"], "Bars fix");
    let out = run(&dir, &["grade-preset-save", p, "0:1", "--name", "Bars fix"]);
    assert_eq!(out.code, 1, "a clash is refused: {}", out.json);
    ok(
        &dir,
        &[
            "grade-preset-save",
            p,
            "0:1",
            "--name",
            "Bars fix",
            "--replace",
        ],
    );
    let listed = ok(&dir, &["grade-presets", p]);
    assert_eq!(listed["presets"][0]["name"], "Bars fix");
    let applied = ok(&dir, &["grade-preset-apply", p, "0:0", "Bars fix"]);
    let bars_grade = ok(&dir, &["grade", p, "0:1"]);
    assert_eq!(applied["grade"], bars_grade["grade"]);
    let removed = ok(&dir, &["grade-presets", p, "--remove", "Bars fix"]);
    assert_eq!(removed["presets"].as_array().unwrap().len(), 0);
}

/// The setting alone: the CLI's private cache has no model, so the render is
/// left to the app and the export (`--no-render`).
#[test]
fn isolate_voice_sets_and_clears_the_setting() {
    require_ffmpeg!();
    let dir = common::scratch("isolate-voice");
    let (card, _) = common::media(&dir);
    let project = dir.join("voice.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    let data = ok(
        &dir,
        &[
            "isolate-voice",
            p,
            "0:0",
            "--keep",
            "background",
            "--strength",
            "0.5",
            "--no-render",
        ],
    );
    let isolate = &data["cleanup"]["isolate"];
    assert_eq!(isolate["keep"], "background", "{data}");
    assert_eq!(isolate["strength"], 0.5);
    assert!(isolate["model"]
        .as_str()
        .unwrap()
        .starts_with("htdemucs-vocals"));
    let out = run(
        &dir,
        &["isolate-voice", p, "0:0", "--keep", "drums", "--no-render"],
    );
    assert_eq!(out.code, 2, "{}", out.json);
    let data = ok(&dir, &["isolate-voice", p, "0:0", "--off"]);
    assert!(data["cleanup"].is_null(), "{data}");
}

/// Retouch's setting alone (the private cache has no face model): one
/// effect on the clip, its values from the preset and the flags.
#[test]
fn retouch_sets_presets_and_values() {
    require_ffmpeg!();
    let dir = common::scratch("retouch");
    let (card, _) = common::media(&dir);
    let project = dir.join("retouch.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    let data = ok(
        &dir,
        &[
            "retouch",
            p,
            "0:0",
            "--preset",
            "sculpt",
            "--strength",
            "80",
            "--no-analyse",
        ],
    );
    assert_eq!(data["retouch"]["slim"], 45.0, "{data}");
    assert_eq!(data["retouch"]["strength"], 80.0);
    // Again: the same effect changes, from its own values; no second one
    // appears.
    let data = ok(
        &dir,
        &["retouch", p, "0:0", "--smooth", "10", "--no-analyse"],
    );
    assert_eq!(data["retouch"]["slim"], 45.0, "{data}");
    assert_eq!(data["retouch"]["smooth"], 10.0, "{data}");
    assert_eq!(retouches(&project), 1, "one retouch effect on the clip");
    let out = run(
        &dir,
        &["retouch", p, "0:0", "--preset", "glam", "--no-analyse"],
    );
    assert_eq!(out.code, 2, "{}", out.json);
    let out = run(
        &dir,
        &["retouch", p, "0:0", "--slim", "101", "--no-analyse"],
    );
    assert_eq!(out.code, 2, "{}", out.json);
    ok(&dir, &["retouch", p, "0:0", "--off"]);
    assert_eq!(retouches(&project), 0, "the effect is gone");
}

/// Follow body part's words are checked before anything is analysed (the
/// private cache has no body model): an unknown part, a person counted from
/// zero, an unknown mode.
#[test]
fn follow_body_refuses_unknown_parts_and_people() {
    require_ffmpeg!();
    let dir = common::scratch("follow-body");
    let (card, bars) = common::media(&dir);
    let project = dir.join("body.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    ok(&dir, &["import", p, bars.to_str().unwrap(), "--append"]);
    for args in [
        &["--part", "tail"][..],
        &["--part", "left_hand", "--person", "0"],
        &["--part", "hips", "--mode", "orbit"],
    ] {
        let mut all = vec!["follow-body", p, "0:1", "--body-of", "0:0"];
        all.extend_from_slice(args);
        let out = run(&dir, &all);
        assert_eq!(out.code, 2, "{args:?}: {}", out.json);
    }
}

/// Retouch effects the clips of a saved project carry.
fn retouches(project: &std::path::Path) -> usize {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(project).unwrap()).unwrap();
    let effects = doc["materials"]["effects"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let is_retouch = |id: &serde_json::Value| {
        effects
            .iter()
            .any(|e| &e["id"] == id && e["kind"] == "retouch")
    };
    doc["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["segments"].as_array().cloned().unwrap_or_default())
        .flat_map(|s| s["extras"].as_array().cloned().unwrap_or_default())
        .filter(is_retouch)
        .count()
}
