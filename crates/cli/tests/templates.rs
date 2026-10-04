//! Templates from the command line: list, make a project from one, list and
//! fill its slots, save a project as a template and use that.

mod common;

use common::{ok, run};

#[test]
fn a_template_round_trip_through_the_cli() {
    require_ffmpeg!();
    let dir = common::scratch("templates");
    let (card, bars) = common::media(&dir);
    let (card, bars) = (card.to_str().unwrap(), bars.to_str().unwrap());

    let list = ok(&dir, &["template", "list"]);
    let list = list.as_array().unwrap();
    assert!(list.len() >= 8);
    assert!(list.iter().any(|t| t["id"] == "quick-cuts"));

    let project = dir.join("cuts.chukcut");
    let p = project.to_str().unwrap();
    let applied = ok(&dir, &["template", "apply", p, "quick-cuts", card, bars]);
    assert_eq!(applied["filled"].as_array().unwrap().len(), 2);
    assert_eq!(applied["empty"].as_array().unwrap().len(), 4);

    let slots = ok(&dir, &["template", "slots", p]);
    let slots = slots.as_array().unwrap();
    assert_eq!(slots.len(), 6);
    assert_eq!(slots[0]["filled"], true);
    assert_eq!(slots[2]["filled"], false);

    ok(
        &dir,
        &[
            "template", "replace", p, "--clip", "slot:3", "--media", card, "--from", "1.5",
        ],
    );
    let slots = ok(&dir, &["template", "slots", p]);
    assert_eq!(slots[2]["filled"], true);

    // Applying over an existing file needs --force.
    let refused = run(&dir, &["template", "apply", p, "quick-cuts"]);
    assert_ne!(refused.code, 0);

    // Without --slot, the project's slots stay slots.
    let saved = ok(&dir, &["template", "save", p, "--name", "My cuts"]);
    assert_eq!(saved["slots"].as_array().unwrap().len(), 6);
    ok(&dir, &["template", "delete", saved["id"].as_str().unwrap()]);
}

#[test]
fn saving_and_deleting_a_template_of_ones_own() {
    require_ffmpeg!();
    let dir = common::scratch("templates-save");
    let (card, _) = common::media(&dir);
    let card = card.to_str().unwrap();
    let project = dir.join("mine.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card, "--append"]);

    let saved = ok(
        &dir,
        &[
            "template", "save", p, "--name", "One shot", "--slot", "0:0", "--label", "Hero",
        ],
    );
    let id = saved["id"].as_str().unwrap().to_string();
    assert_eq!(saved["slots"][0]["label"], "Hero");
    assert_eq!(saved["builtin"], false);

    let made = dir.join("made.chukcut");
    let applied = ok(
        &dir,
        &["template", "apply", made.to_str().unwrap(), &id, card],
    );
    assert_eq!(applied["filled"].as_array().unwrap().len(), 1);

    ok(&dir, &["template", "delete", &id]);
    let list = ok(&dir, &["template", "list"]);
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["id"] != id.as_str()));
    let refused = run(&dir, &["template", "delete", "quick-cuts"]);
    assert_ne!(refused.code, 0);
}

#[test]
fn a_template_goes_into_an_existing_project_as_a_timeline_or_a_compound_clip() {
    require_ffmpeg!();
    let dir = common::scratch("templates-into");
    let (card, bars) = common::media(&dir);
    let (card, bars) = (card.to_str().unwrap(), bars.to_str().unwrap());
    let project = dir.join("mine.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card, "--append"]);

    // --as without --into would silently make a new file: refused.
    let refused = run(
        &dir,
        &["template", "apply", p, "quick-cuts", "--as", "compound"],
    );
    assert_ne!(refused.code, 0);

    // As a new timeline, opened.
    let applied = ok(
        &dir,
        &[
            "template",
            "apply",
            "--into",
            p,
            "quick-cuts",
            bars,
            "--name",
            "Cuts",
        ],
    );
    assert_eq!(applied["as"], "timeline");
    assert_eq!(applied["filled"].as_array().unwrap().len(), 1);
    let list = ok(&dir, &["timeline", "list", p]);
    let names: Vec<&str> = list["timelines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"Cuts"));
    let slots = ok(&dir, &["template", "slots", p]);
    assert_eq!(slots.as_array().unwrap().len(), 6);

    // As a compound clip at 1 s on the first timeline: its slots are slots
    // inside the compound clip, numbered after the ones before them.
    ok(&dir, &["timeline", "switch", p, "0"]);
    let compound = ok(
        &dir,
        &[
            "template",
            "apply",
            "--into",
            p,
            "before-after",
            "--as",
            "compound",
            "--at",
            "1",
        ],
    );
    assert_eq!(compound["as"], "compound");
    assert_eq!(compound["clip"]["start"], 1.0);
    let slots = ok(&dir, &["template", "slots", p]);
    let slots = slots.as_array().unwrap();
    assert_eq!(slots.len(), 8);
    let inside: Vec<&serde_json::Value> =
        slots.iter().filter(|s| s["in_compound"] == true).collect();
    assert_eq!(inside.len(), 2);
    // Numbered across the project: 1 to 8, each once.
    let mut numbers: Vec<u64> = slots
        .iter()
        .map(|s| s["number"].as_u64().unwrap())
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=8).collect::<Vec<u64>>());
    let second_inside = inside.iter().find(|s| s["index"] == 2).unwrap()["number"]
        .as_u64()
        .unwrap();
    ok(
        &dir,
        &[
            "template",
            "replace",
            p,
            "--clip",
            &format!("slot:{second_inside}"),
            "--media",
            card,
        ],
    );
    let slots = ok(&dir, &["template", "slots", p]);
    let filled = slots
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["in_compound"] == true && s["index"] == 2)
        .unwrap()
        .clone();
    assert_eq!(filled["filled"], true);
    // A slot inside the compound clip is also reached by its clip id.
    let first = inside[0]["clip"].as_str().unwrap();
    ok(
        &dir,
        &[
            "template",
            "replace",
            p,
            "--clip",
            &first[..8],
            "--media",
            bars,
        ],
    );
}
