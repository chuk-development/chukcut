//! Timelines and compound clips from the command line: each step runs the
//! binary and reads the saved project back.

mod common;

use std::path::Path;

use common::{ok, run};
use serde_json::Value;

fn document(project: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap()
}

fn clip_count(doc: &Value) -> usize {
    doc["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["segments"].as_array().unwrap().len())
        .sum()
}

#[test]
fn compound_clips_and_timelines_round_trip_through_the_file() {
    require_ffmpeg!();
    let dir = common::scratch("compound");
    let (card, bars) = common::media(&dir);
    let project = dir.join("compound.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    ok(&dir, &["import", p, bars.to_str().unwrap(), "--append"]);
    let plain = std::fs::read_to_string(&project).unwrap();
    assert!(
        !plain.contains("\"sequence"),
        "one timeline writes no new keys"
    );
    let before = clip_count(&document(&project));

    // Both pictures into one compound clip.
    let made = ok(
        &dir,
        &["compound", "create", p, "0:0", "0:1", "--name", "Intro"],
    );
    let clip = made["clip"].as_str().unwrap().to_string();
    let doc = document(&project);
    assert_eq!(clip_count(&doc), 1);
    let sequences = doc["materials"]["sequences"].as_array().unwrap();
    assert_eq!(sequences.len(), 1);
    assert_eq!(sequences[0]["name"], "Intro");
    assert_eq!(sequences[0]["kind"], "compound");

    // Open it, cut inside, close it.
    let opened = ok(&dir, &["compound", "open", p, &clip]);
    assert_eq!(opened["path"].as_array().unwrap().len(), 2);
    ok(&dir, &["split", p, "--at", "1"]);
    let doc = document(&project);
    assert_eq!(doc["sequence"]["kind"], "compound");
    ok(&dir, &["compound", "close", p]);
    let doc = document(&project);
    assert!(doc.get("sequence").is_none(), "back on the main timeline");

    // A second timeline, then back; a copy; a rename; a delete.
    let new = ok(&dir, &["timeline", "new", p, "--name", "Shorts"]);
    assert_eq!(new["path"][0]["name"], "Shorts");
    ok(&dir, &["timeline", "switch", p, "0"]);
    ok(&dir, &["timeline", "duplicate", p, "Timeline 01"]);
    ok(&dir, &["timeline", "rename", p, "2", "Long cut"]);
    let list = ok(&dir, &["timeline", "list", p]);
    let names: Vec<&str> = list["timelines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Timeline 01", "Shorts", "Long cut"]);
    assert_eq!(list["compounds"].as_array().unwrap().len(), 1);
    assert_eq!(list["compounds"][0]["uses"], 2, "the copy shows it too");
    ok(&dir, &["timeline", "delete", p, "Shorts"]);
    ok(&dir, &["timeline", "delete", p, "Timeline 01"]);
    let last = run(&dir, &["timeline", "delete", p, "0"]);
    assert_ne!(last.code, 0, "the last timeline stays");
    let doc = document(&project);
    assert_eq!(doc["sequence"]["name"], "Long cut", "its neighbour opened");

    // Flatten the copy's compound clip: the clips are back, one more than
    // before because the split inside came out with them.
    let info = ok(&dir, &["info", p]);
    let compound = info["tracks"][0]["clips"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(&dir, &["compound", "flatten", p, &compound]);
    let doc = document(&project);
    assert_eq!(clip_count(&doc), before + 1);
    assert!(
        doc["materials"].get("sequences").is_none(),
        "nothing uses it now"
    );
    ok(&dir, &["validate", p]);
}
