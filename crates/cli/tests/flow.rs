//! The CLI end to end: generated media in, a checked video file out.
//!
//! Each step is a separate process, as a script would run it, so these tests
//! also prove that what one invocation saves the next one reads back — ids,
//! grades, titles and all.

mod common;

use std::path::Path;

use common::{no_gpu, ok, run};
use serde_json::Value;

fn clips(info: &Value, lane: usize) -> Vec<Value> {
    info["tracks"][lane]["clips"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn new_import_split_grade_title_export() {
    require_ffmpeg!();
    let dir = common::scratch("flow");
    let (card, bars) = common::media(&dir);
    let project = dir.join("flow.chukcut");
    let p = project.to_str().unwrap();

    ok(&dir, &["new", p]);
    assert!(project.is_file(), "new writes the file");

    let imported = ok(
        &dir,
        &[
            "import",
            p,
            card.to_str().unwrap(),
            bars.to_str().unwrap(),
            "--append",
        ],
    );
    assert_eq!(imported["materials"].as_array().unwrap().len(), 2);

    // The first clip shaped the canvas: 16:9 at 1080 on the short edge.
    let info = ok(&dir, &["info", p]);
    assert_eq!(info["canvas"]["width"], 1920);
    assert_eq!(info["canvas"]["height"], 1080);
    assert_eq!(info["duration"], 7.0);
    assert_eq!(clips(&info, 0).len(), 2);

    let split = ok(&dir, &["split", p, "--at", "2s", "--clip", "0:0"]);
    let right = split["created"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(split["created"][0]["start"], 2.0);

    // Grade the right half by its id prefix, as a script that kept it would.
    let graded = ok(
        &dir,
        &[
            "grade",
            p,
            &right[..8],
            "--set",
            "exposure=0.5",
            "--set",
            "saturation=1.3",
        ],
    );
    assert_eq!(graded["grade"]["exposure"], 0.5);

    let title = ok(
        &dir,
        &[
            "title",
            "add",
            p,
            "Hello",
            "--at",
            "0.5",
            "--duration",
            "2",
            "--color",
            "#ffcc00",
            "--y",
            "0.4",
        ],
    );
    assert_eq!(title["clip"]["kind"], "title");
    assert_eq!(title["clip"]["text"]["color"], "#ffcc00ff");

    ok(
        &dir,
        &[
            "effect",
            "add",
            p,
            "gaussian_blur",
            "--clip",
            "0:2",
            "--set",
            "radius=0.2",
        ],
    );
    ok(
        &dir,
        &[
            "transition",
            "add",
            p,
            "0:1",
            "--kind",
            "dissolve",
            "--duration",
            "0.5",
        ],
    );

    // Everything above is in the file, read back by a fresh process.
    let info = ok(&dir, &["info", p]);
    let lane = clips(&info, 0);
    assert_eq!(lane.len(), 3);
    assert_eq!(lane[1]["id"], right.as_str());
    assert_eq!(
        lane[1]["grade"]["saturation"]
            .as_f64()
            .map(|v| (v * 10.0).round()),
        Some(13.0)
    );
    assert!(lane[1]["transition"].is_object());
    assert_eq!(lane[2]["effects"][0]["kind"], "gaussian_blur");
    assert!(
        info["issues"].as_array().unwrap().is_empty(),
        "{}",
        info["issues"]
    );
    ok(&dir, &["validate", p]);

    let frame = dir.join("frame.png");
    let rendered = run(
        &dir,
        &["render-frame", p, "--at", "1", frame.to_str().unwrap()],
    );
    if no_gpu(&rendered) {
        eprintln!("skipping the render half: no GPU on this machine");
        return;
    }
    assert_eq!(rendered.code, 0, "{}", rendered.json);
    let png = std::fs::read(&frame).unwrap();
    assert_eq!(&png[1..4], b"PNG");

    let out = dir.join("out.mp4");
    let exported = ok(&dir, &["export", p, out.to_str().unwrap(), "--crf", "28"]);
    assert_eq!(exported["frames"], 210);
    let probed = common::probe(Path::new(exported["path"].as_str().unwrap()));
    assert_eq!((probed.width, probed.height), (1920, 1080));
    assert_eq!(
        probed.frames, 210,
        "every frame of 7 s at 30 fps, counted by decoding"
    );
    assert!(
        (probed.duration - 7.0).abs() < 0.1,
        "duration {}",
        probed.duration
    );
    assert!(probed.audio, "the tone is mixed in");
}

#[test]
fn a_batch_is_one_session_with_undo_and_all_or_nothing() {
    require_ffmpeg!();
    let dir = common::scratch("batch");
    let (card, _) = common::media(&dir);
    let project = dir.join("batch.chukcut");
    let p = project.to_str().unwrap();

    let ops = serde_json::json!([
        {"op": "new", "width": 1080, "height": 1920},
        {"op": "import", "files": [card], "append": true},
        {"op": "split", "at": 1.5},
        {"op": "grade", "clip": "0:1", "set": {"exposure": 1.0}},
        {"op": "undo"},
        {"op": "undo"},
        {"op": "redo"},
        {"op": "title_add", "text": "Batch", "at": "15f"},
    ]);
    let file = dir.join("ops.json");
    std::fs::write(&file, ops.to_string()).unwrap();
    let results = ok(&dir, &["batch", p, file.to_str().unwrap()]);
    assert_eq!(results.as_array().unwrap().len(), 8);
    assert_eq!(results[4]["data"]["undone"], "Adjust colour");

    let info = ok(&dir, &["info", p]);
    let lane = clips(&info, 0);
    assert_eq!(lane.len(), 2, "the split was redone");
    assert!(lane[1].get("grade").is_none(), "the grade stayed undone");
    let title = &info["tracks"][2]["clips"][0];
    assert_eq!(title["start"], 0.5, "15 frames at 30 fps");

    // A failing operation leaves the file exactly as it was.
    let before = std::fs::read(&project).unwrap();
    let bad = dir.join("bad.json");
    std::fs::write(
        &bad,
        r#"{"ops": [{"op": "split", "at": 3}, {"op": "move", "clip": "0:9", "to": 1}]}"#,
    )
    .unwrap();
    let failed = run(&dir, &["batch", p, bad.to_str().unwrap()]);
    assert_eq!(failed.code, 2);
    assert!(failed.json["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("operation 1 (move)"));
    assert_eq!(
        std::fs::read(&project).unwrap(),
        before,
        "nothing was saved"
    );
}

#[test]
fn failures_have_distinct_exit_codes_and_save_nothing() {
    require_ffmpeg!();
    let dir = common::scratch("errors");
    let (card, _) = common::media(&dir);
    let project = dir.join("e.chukcut");
    let p = project.to_str().unwrap();

    let missing = run(&dir, &["info", dir.join("nope.chukcut").to_str().unwrap()]);
    assert_eq!(missing.code, 3, "no project file");
    assert_eq!(missing.json["error"]["kind"], "project");

    ok(&dir, &["new", p]);
    let again = run(&dir, &["new", p]);
    assert_eq!(again.code, 3, "new refuses to overwrite without --force");

    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    let before = std::fs::read(&project).unwrap();

    let unknown = run(&dir, &["split", p, "--at", "1", "--clip", "5:5"]);
    assert_eq!(unknown.code, 2, "an unknown clip is a usage error");

    let refused = run(&dir, &["split", p, "--at", "99"]);
    assert_eq!(refused.code, 1, "the engine refuses a split past the end");

    let bad_time = run(&dir, &["split", p, "--at", "soon"]);
    assert_eq!(bad_time.code, 2, "clap reports a bad time as usage");

    let bad_value = run(&dir, &["grade", p, "0:0", "--set", "exposure=lots"]);
    assert_eq!(bad_value.code, 2);

    assert_eq!(
        std::fs::read(&project).unwrap(),
        before,
        "nothing was saved"
    );

    // A dry run reports the edit and leaves the file alone.
    let dry = run(&dir, &["--dry-run", "split", p, "--at", "1"]);
    assert_eq!(dry.code, 0);
    assert_eq!(dry.json["saved"], false);
    assert_eq!(std::fs::read(&project).unwrap(), before);
}

#[test]
fn the_working_copy_and_user_settings_are_never_written() {
    require_ffmpeg!();
    let dir = common::scratch("headless");
    let (card, _) = common::media(&dir);
    let project = dir.join("h.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    ok(&dir, &["split", p, "--at", "1"]);
    let autosave = dir.join("xdg/config/chukcut/autosave.chukcut");
    assert!(
        !autosave.exists(),
        "the app's crash-recovery copy is untouched"
    );
}
