//! The operations that closed the CLI's gaps to the command layer: clip
//! names and links, pasted attributes, looks, the effect stack, transition
//! settings, title words and fonts, mask order, caption edits, the library
//! and the cloud tools. Nothing here needs the network: the library is
//! exercised through what ships with the engine (looks, curated music
//! titles, the pack list, local image stickers), and the cloud tools only
//! as far as their refusal without an account.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{ok, run};
use serde_json::Value;

fn document(project: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap()
}

fn with_media(name: &str) -> (PathBuf, PathBuf) {
    let dir = common::scratch(name);
    let (card, bars) = common::media(&dir);
    let project = dir.join(format!("{name}.chukcut"));
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(
        &dir,
        &[
            "import",
            p,
            card.to_str().unwrap(),
            bars.to_str().unwrap(),
            "--append",
        ],
    );
    (dir, project)
}

/// Every clip `info` lists, flattened.
fn clips(dir: &Path, project: &str) -> Vec<Value> {
    let info = ok(dir, &["info", project]);
    info["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["clips"].as_array().cloned().unwrap_or_default())
        .collect()
}

fn clip(dir: &Path, project: &str, reference: &str) -> Value {
    clips(dir, project)
        .into_iter()
        .find(|c| c["ref"] == reference || c["id"] == reference)
        .unwrap_or_else(|| panic!("no clip {reference}"))
}

#[test]
fn names_links_pasted_attributes_and_looks() {
    require_ffmpeg!();
    let (dir, project) = with_media("clipops");
    let p = project.to_str().unwrap();

    let named = ok(&dir, &["rename", p, "0:0", "Intro"]);
    assert_eq!(named["name"], "Intro");
    let cleared = ok(&dir, &["rename", p, "0:0", ""]);
    assert!(cleared["name"].is_null());

    assert!(clip(&dir, p, "0:0")["link_group"].is_null());
    ok(&dir, &["link", p, "0:0", "0:1"]);
    let group = clip(&dir, p, "0:0")["link_group"].clone();
    assert!(group.is_string());
    assert_eq!(clip(&dir, p, "0:1")["link_group"], group);
    ok(&dir, &["unlink", p, "0:0"]);
    assert!(clip(&dir, p, "0:0")["link_group"].is_null());
    assert!(!run(&dir, &["link", p, "0:0"]).json["ok"].as_bool().unwrap());

    ok(&dir, &["set", p, "0:0", "--scale", "0.5"]);
    ok(&dir, &["grade", p, "0:0", "--set", "exposure=0.5"]);
    let pasted = ok(&dir, &["paste-attributes", p, "--from", "0:0", "0:1"]);
    let target = &pasted["clips"][0];
    assert_eq!(target["transform"]["scale"][0], 0.5);
    assert!(target["grade"]["exposure"].as_f64().unwrap() > 0.4);

    ok(&dir, &["grade", p, "0:1", "--reset", "all"]);
    ok(&dir, &["grade-to-all", p, "0:0"]);
    assert!(clip(&dir, p, "0:1")["grade"]["exposure"].as_f64().unwrap() > 0.4);

    let looks = ok(&dir, &["catalog", "looks"]);
    let look = looks[0]["name"].as_str().unwrap().to_string();
    let looked = ok(&dir, &["look", p, "0:0", &look, "--intensity", "0.6"]);
    let lut = looked["clip"]["grade"]["lut"].as_str().unwrap().to_string();
    assert!(Path::new(&lut).is_file(), "{lut} is installed");
    // The exposure stayed: a look changes only the LUT.
    assert!(looked["clip"]["grade"]["exposure"].as_f64().unwrap() > 0.4);

    // A .cube of our own is copied into the library first.
    let cube = dir.join("mine.cube");
    std::fs::write(
        &cube,
        "TITLE \"mine\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n",
    )
    .unwrap();
    let own = ok(&dir, &["look", p, "0:1", cube.to_str().unwrap()]);
    let lut = own["clip"]["grade"]["lut"].as_str().unwrap().to_string();
    assert_ne!(
        Path::new(&lut),
        cube,
        "the clip points at the library's copy"
    );
    assert!(
        lut.contains("xdg"),
        "{lut} is in the test's own data folder"
    );
    let none = ok(&dir, &["look", p, "0:1", "none"]);
    assert!(none["clip"]["grade"]["lut"].is_null());
    let unknown = run(&dir, &["look", p, "0:1", "No Such Look"]);
    assert_eq!(unknown.code, 2);
}

#[test]
fn effect_stack_transitions_titles_and_mask_order() {
    require_ffmpeg!();
    let (dir, project) = with_media("stackops");
    let p = project.to_str().unwrap();

    let catalog = ok(&dir, &["catalog", "effects"]);
    let kinds: Vec<&Value> = catalog
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["params"].as_array().is_some_and(|p| !p.is_empty()))
        .take(2)
        .collect();
    let (first, second) = (
        kinds[0]["id"].as_str().unwrap(),
        kinds[1]["id"].as_str().unwrap(),
    );
    let param = kinds[1]["params"][0]["id"].as_str().unwrap();
    ok(&dir, &["effect", "add", p, first, "--clip", "0:0"]);
    ok(&dir, &["effect", "add", p, second, "--clip", "0:0"]);
    let moved = ok(&dir, &["effect", "move", p, "0:0", "1", "--to", "0"]);
    assert_eq!(moved["clip"]["effects"][0]["kind"], second);
    ok(&dir, &["effect", "reset", p, "0:0", "0"]);
    let keyed = ok(
        &dir,
        &[
            "effect", "keyframe", p, "0:0", "0", "--param", param, "--at", "1",
        ],
    );
    assert_eq!(keyed["clip"]["effects"][0]["animated"][0], param);

    ok(&dir, &["transition", "add", p, "0:1", "--duration", "0.5"]);
    let set = ok(
        &dir,
        &[
            "transition",
            "set",
            p,
            "0:1",
            "--duration",
            "0.8",
            "--easing",
            "linear",
        ],
    );
    assert_eq!(set["transition"]["duration"], 0.8);
    let doc = document(&project);
    let found = doc["materials"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .any(|m| m["duration"] == 800_000 && m["easing"] == "linear");
    assert!(found, "the transition material is 0.8 s and linear");
    assert_eq!(
        run(&dir, &["transition", "set", p, "0:0", "--zoom", "0.2"]).code,
        2,
        "the first clip has no transition"
    );

    let title = ok(&dir, &["title", "add", p, "Hello", "--at", "0.5"]);
    let id = title["clip"]["id"].as_str().unwrap().to_string();
    let retyped = ok(&dir, &["title", "text", p, &id, "World"]);
    assert_eq!(retyped["clip"]["text"]["content"], "World");
    let families = ok(&dir, &["catalog", "fonts_system"]);
    let family = families[0].as_str().unwrap().to_string();
    let fonted = ok(&dir, &["title", "font", p, &id, &family]);
    assert_eq!(fonted["clip"]["text"]["font"], family.as_str());
    assert_eq!(fonted["installed"], false);

    ok(&dir, &["mask", p, "0:0", "--add", "circle"]);
    ok(&dir, &["mask", p, "0:0", "--add", "rectangle"]);
    let order = ok(&dir, &["mask-move", p, "0:0", "1", "--to", "0"]);
    assert_eq!(order["compositing"]["masks"][0]["shape"], "rectangle");
    assert_eq!(order["compositing"]["masks"][1]["shape"], "ellipse");
}

#[test]
fn captions_one_at_a_time() {
    require_ffmpeg!();
    let (dir, project) = with_media("captionops");
    let p = project.to_str().unwrap();

    ok(
        &dir,
        &[
            "captions", "add", p, "one two", "--start", "0", "--end", "1",
        ],
    );
    ok(
        &dir,
        &["captions", "add", p, "three", "--start", "1", "--end", "2"],
    );
    let list = ok(&dir, &["captions", "list", p]);
    assert_eq!(list.as_array().unwrap().len(), 2);
    let (a, b) = (
        list[0]["id"].as_str().unwrap().to_string(),
        list[1]["id"].as_str().unwrap().to_string(),
    );

    ok(&dir, &["captions", "text", p, &a, "uno dos"]);
    let merged = ok(&dir, &["captions", "merge", p, &a, &b]);
    let one = merged["clip"].as_str().unwrap().to_string();
    let list = ok(&dir, &["captions", "list", p]);
    assert_eq!(list.as_array().unwrap().len(), 1);
    let text = list[0]["text"].as_str().unwrap();
    assert!(text.contains("uno dos") && text.contains("three"), "{text}");

    ok(&dir, &["captions", "split", p, &one, "--at", "1"]);
    assert_eq!(
        ok(&dir, &["captions", "list", p]).as_array().unwrap().len(),
        2
    );

    let words = ok(&dir, &["captions", "regroup", p, "--words", "1"]);
    assert_eq!(words.as_array().unwrap().len(), 3, "{words}");

    // The video clip is not a caption.
    assert_eq!(run(&dir, &["captions", "text", p, "0:0", "x"]).code, 2);

    ok(&dir, &["captions", "clear", p]);
    assert!(ok(&dir, &["captions", "list", p])
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn library_lists_stickers_and_cloud_refusals() {
    require_ffmpeg!();
    let (dir, project) = with_media("libraryops");
    let p = project.to_str().unwrap();

    for kind in [
        "providers",
        "fal_actions",
        "sfx",
        "music",
        "fonts_system",
        "recent",
    ] {
        let list = ok(&dir, &["catalog", kind]);
        assert!(list.is_array(), "catalog {kind}: {list}");
        if kind != "recent" {
            assert!(
                !list.as_array().unwrap().is_empty(),
                "catalog {kind} is empty"
            );
        }
    }
    let packs = ok(&dir, &["catalog", "sfx"]);
    assert_eq!(packs[0]["downloaded"], false);
    let moods = ok(&dir, &["catalog", "music", "--filter", "Calm"]);
    assert!(moods
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["mood"] == "Calm"));
    assert!(ok(&dir, &["catalog", "cache"])["cache_bytes"].is_number());
    assert!(ok(&dir, &["catalog", "settings"]).is_object());
    assert!(ok(&dir, &["catalog", "machine"]).is_object());
    assert_eq!(
        run(&dir, &["catalog", "icons"]).code,
        2,
        "icons need --search"
    );

    let image = dir.join("badge.png");
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi"])
        .args(["-i", "color=c=orange:s=128x128", "-frames:v", "1"])
        .arg(&image)
        .status()
        .unwrap();
    assert!(status.success());
    let sticker = ok(
        &dir,
        &["sticker", p, "--file", image.to_str().unwrap(), "--at", "1"],
    );
    assert_eq!(sticker["clip"]["kind"], "image");
    assert_eq!(sticker["clip"]["start"], 1.0);
    assert_eq!(
        run(&dir, &["sticker", p, "--emoji", "x", "--file", "y.png"]).code,
        2
    );

    let credits = ok(&dir, &["cloud", "credits", p]);
    assert!(credits["summary"].is_object());

    // No account in the test's own config folder.
    assert_eq!(run(&dir, &["cloud", "sound", p, "a door slams"]).code, 1);
    assert_eq!(
        run(&dir, &["cloud", "fal", p, "0:0", "--action", "upscale"]).code,
        1
    );
    assert_eq!(run(&dir, &["catalog", "voices"]).code, 1);
}

#[test]
fn two_titles_at_once_on_two_lanes_and_styled_lengths() {
    require_ffmpeg!();
    let (dir, project) = with_media("laneops");
    let p = project.to_str().unwrap();

    let head = ok(
        &dir,
        &["title", "add", p, "Day 1", "--at", "0.5", "--duration", "4"],
    );
    let lane = ok(&dir, &["lane-add", p, "--kind", "text"]);
    assert_eq!(lane["track"]["name"], "Text 2");
    let sub = ok(
        &dir,
        &[
            "title",
            "add",
            p,
            "Lisbon",
            "--at",
            "0.5",
            "--duration",
            "4",
            "--track",
            "Text 2",
        ],
    );
    assert_eq!(sub["clip"]["start"], 0.5, "not moved to the next gap");
    assert_ne!(
        head["clip"]["ref"].as_str().unwrap().split(':').next(),
        sub["clip"]["ref"].as_str().unwrap().split(':').next(),
        "two lanes"
    );
    // A lane a title may not go on is refused, not silently replaced.
    assert_eq!(run(&dir, &["title", "add", p, "x", "--track", "0"]).code, 2);

    let styles = ok(&dir, &["catalog", "title_styles"]);
    let style = styles[0]["id"].as_str().unwrap();
    let styled = ok(
        &dir,
        &["title", "style", p, style, "--at", "6", "--duration", "5.5"],
    );
    assert_eq!(styled["clip"]["duration"], 5.5);
    let templates = ok(&dir, &["catalog", "title_templates"]);
    let template = templates[0]["id"].as_str().unwrap();
    let templated = ok(
        &dir,
        &[
            "title",
            "template",
            p,
            template,
            "--at",
            "13",
            "--duration",
            "1.5",
            "--track",
            "Text 2",
        ],
    );
    assert_eq!(templated["clip"]["duration"], 1.5);
}
