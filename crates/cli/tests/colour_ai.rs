//! Colour AI from the command line: auto adjust, colour match, grade
//! presets — each one undo step, saved into the project file.

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
