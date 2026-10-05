//! The commands added after the first CLI: markers, crop, curves, freeze
//! frame, speed curves, layouts, title styles and templates, analysis and
//! cloud. Each test runs the binary on generated media and reads the saved
//! project back, so it checks what is in the file, not what was printed.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{ok, run};
use serde_json::{json, Value};

/// The saved document itself.
fn document(project: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap()
}

fn segment<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["segments"].as_array().unwrap().iter())
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("no segment {id}"))
}

fn lane_clips(info: &Value, lane: usize) -> Vec<Value> {
    info["tracks"][lane]["clips"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[rustfmt::skip]
fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y"])
        .args(args)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg failed: {args:?}");
}

/// Two seconds of one test card, then two of another: one hard cut at 2 s.
#[rustfmt::skip]
fn hard_cut(dir: &Path) -> PathBuf {
    let out = dir.join("cut.mp4");
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=2",
        "-f", "lavfi", "-i", "smptebars=size=640x360:rate=30:duration=2",
        "-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0[v]", "-map", "[v]",
        "-c:v", "libx264", "-pix_fmt", "yuv420p",
        out.to_str().unwrap(),
    ]);
    out
}

/// Eight seconds of clicks at 120 BPM.
#[rustfmt::skip]
fn click_track(dir: &Path) -> PathBuf {
    let out = dir.join("clicks.wav");
    ffmpeg(&[
        "-f", "lavfi",
        "-i", "aevalsrc=if(lt(mod(t\\,0.5)\\,0.03)\\,sin(2*PI*1500*t)*exp(-80*mod(t\\,0.5))\\,0):s=44100:d=8",
        out.to_str().unwrap(),
    ]);
    out
}

/// A new project with the 4 s test card on lane 0.
fn with_card(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let dir = common::scratch(name);
    let (card, bars) = common::media(&dir);
    let project = dir.join(format!("{name}.chukcut"));
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    (dir, project, card, bars)
}

#[test]
fn markers_are_added_changed_listed_and_removed() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("markers");
    let p = project.to_str().unwrap();

    let a = ok(
        &dir,
        &["marker", "add", p, "--at", "1.5", "--label", "Drop"],
    );
    let id = a["marker"]["id"].as_str().unwrap().to_string();
    ok(&dir, &["marker", "add", p, "--at", "0.5", "--color", "red"]);

    let doc = document(&project);
    assert_eq!(doc["markers"].as_array().unwrap().len(), 2);

    // Index 1 in time order is the 1.5 s marker.
    ok(
        &dir,
        &[
            "marker", "set", p, "1", "--at", "3s", "--label", "Chorus", "--color", "green",
        ],
    );
    let doc = document(&project);
    let moved = doc["markers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == id.as_str())
        .unwrap();
    assert_eq!(moved["time"], 3_000_000);
    assert_eq!(moved["label"], "Chorus");
    assert_eq!(moved["color"], "green");

    let list = ok(&dir, &["marker", "list", p]);
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["color"], "red");
    assert_eq!(list[1]["label"], "Chorus");

    let bad = run(&dir, &["marker", "add", p, "--at", "1", "--color", "pink"]);
    assert_eq!(bad.code, 2, "an unknown colour is a usage error");

    ok(&dir, &["marker", "remove", p, &id[..8]]);
    let doc = document(&project);
    assert_eq!(doc["markers"].as_array().unwrap().len(), 1);
    assert_eq!(doc["markers"][0]["color"], "red");
}

#[test]
fn crop_curve_freeze_and_speed_curve_change_the_clip() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("frame");
    let p = project.to_str().unwrap();
    let info = ok(&dir, &["info", p]);
    let id = lane_clips(&info, 0)[0]["id"].as_str().unwrap().to_string();

    // Crop: the edges land in the segment.
    ok(&dir, &["crop", p, "0:0", "--left", "0.1", "--right", "0.8"]);
    let doc = document(&project);
    let crop = &segment(&doc, &id)["crop"];
    assert!((crop["left"].as_f64().unwrap() - 0.1).abs() < 1e-6);
    assert!((crop["right"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    assert_eq!(crop["top"], 0.0);

    // A tone curve: the points are in the clip's grade, sorted by x.
    let curved = ok(
        &dir,
        &[
            "curve",
            p,
            "0:0",
            "--channel",
            "red",
            "--point",
            "1,0.9",
            "--point",
            "0,0.1",
            "--point",
            "0.5,0.6",
        ],
    );
    let red: Vec<Vec<f64>> = serde_json::from_value(curved["curves"]["red"].clone()).unwrap();
    let want = [[0.0, 0.1], [0.5, 0.6], [1.0, 0.9]];
    assert_eq!(red.len(), 3, "{red:?}");
    for (got, want) in red.iter().zip(want) {
        assert!(
            (got[0] - want[0]).abs() < 1e-6 && (got[1] - want[1]).abs() < 1e-6,
            "{red:?}"
        );
    }
    let doc = document(&project);
    let adjusts = doc["materials"]["color_adjusts"].as_array().unwrap();
    assert!(
        adjusts
            .iter()
            .any(|m| m["grade"]["curves"]["red"].as_array().map(Vec::len) == Some(3)),
        "the curve is saved: {adjusts:?}"
    );
    let bad = run(&dir, &["curve", p, "0:0", "--point", "0,0"]);
    assert_eq!(bad.code, 2, "one point is not a curve");
    ok(&dir, &["curve", p, "0:0", "--channel", "red", "--reset"]);

    // Speed curve: a preset changes the clip's length; removing it restores it.
    let ramped = ok(&dir, &["speed-curve", p, "0:0", "--preset", "hero"]);
    assert_eq!(ramped["curve"]["preset"], "hero");
    assert_ne!(ramped["clip"]["duration"], 4.0);
    let custom = ok(
        &dir,
        &[
            "speed-curve",
            p,
            "0:0",
            "--point",
            "0=1",
            "--point",
            "2s=0.5",
            "--point",
            "3.9=2",
        ],
    );
    assert_eq!(custom["curve"]["points"].as_array().unwrap().len(), 3);
    assert!(custom["curve"]["preset"].is_null());
    let removed = ok(&dir, &["speed-curve", p, "0:0", "--remove"]);
    assert!(removed["curve"].is_null());
    assert_eq!(removed["clip"]["duration"], 4.0);

    // Freeze: a still goes in at 1 s and the lane grows by its length.
    let frozen = ok(
        &dir,
        &["freeze", p, "0:0", "--at", "1", "--duration", "0.5"],
    );
    assert_eq!(frozen["still"]["kind"], "image");
    assert_eq!(frozen["still"]["start"], 1.0);
    assert_eq!(frozen["still"]["duration"], 0.5);
    let info = ok(&dir, &["info", p]);
    let lane = lane_clips(&info, 0);
    assert_eq!(lane.len(), 3, "before, still, after");
    assert_eq!(info["duration"], 4.5);
}

#[test]
fn crop_keyframes_are_set_at_a_time_changed_and_removed() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("cropkeys");
    let p = project.to_str().unwrap();
    let info = ok(&dir, &["info", p]);
    let id = lane_clips(&info, 0)[0]["id"].as_str().unwrap().to_string();
    let crop_keys = |doc: &Value, edge: &str| -> Vec<(i64, f64)> {
        segment(doc, &id)["keyframes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["property"] == edge)
            .flat_map(|t| t["keyframes"].as_array().unwrap().clone())
            .map(|k| (k["time"].as_i64().unwrap(), k["value"].as_f64().unwrap()))
            .collect()
    };

    // A keyframe at the start with the full picture, one at 1 s with the
    // right half gone: all four edges are keyed at both.
    ok(&dir, &["crop", p, "0:0", "--at", "0", "--right", "1"]);
    ok(&dir, &["crop", p, "0:0", "--at", "1s", "--right", "0.5"]);
    let doc = document(&project);
    assert_eq!(
        crop_keys(&doc, "crop_right"),
        vec![(0, 1.0), (1_000_000, 0.5)]
    );
    assert_eq!(crop_keys(&doc, "crop_left").len(), 2);

    // Without a time an animated crop is refused, not silently lost.
    let refused = run(&dir, &["crop", p, "0:0", "--left", "0.2"]);
    assert_ne!(refused.code, 0);

    // One edge through the generic keyframe command, by its name.
    ok(
        &dir,
        &[
            "keyframe",
            p,
            "0:0",
            "--property",
            "crop_left",
            "--at",
            "1s",
            "--value",
            "0.1",
        ],
    );
    assert_eq!(
        crop_keys(&document(&project), "crop_left"),
        vec![(0, 0.0), (1_000_000, 0.1)]
    );

    ok(&dir, &["crop", p, "0:0", "--at", "1s", "--remove"]);
    let doc = document(&project);
    assert_eq!(crop_keys(&doc, "crop_right"), vec![(0, 1.0)]);
    let missing = run(&dir, &["crop", p, "0:0", "--at", "1s", "--remove"]);
    assert_ne!(missing.code, 0, "no keyframe there any more");

    ok(&dir, &["crop", p, "0:0", "--clear"]);
    let doc = document(&project);
    assert!(crop_keys(&doc, "crop_right").is_empty());
    assert!(segment(&doc, &id)["crop"].is_null());
}

#[test]
fn layouts_make_a_picture_in_picture_and_a_split_screen() {
    require_ffmpeg!();
    let (dir, project, _, bars) = with_card("layout");
    let p = project.to_str().unwrap();
    ok(&dir, &["import", p, bars.to_str().unwrap()]);
    let placed = ok(&dir, &["place", p, "bars.mp4", "--at", "0"]);
    let over = placed["clip"]["id"].as_str().unwrap().to_string();
    let base = ok(&dir, &["info", p])["tracks"][0]["clips"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let pip = ok(&dir, &["layout", "pip", p, &over, "--corner", "top_left"]);
    let t = &pip["clip"]["transform"];
    assert!(t["scale"][0].as_f64().unwrap() < 0.5, "{t}");
    assert!(t["position"][0].as_f64().unwrap() < 0.0, "left");
    assert!(t["position"][1].as_f64().unwrap() > 0.0, "top");
    assert_eq!(pip["clip"]["effects"][0]["kind"], "frame");

    let split = ok(
        &dir,
        &["layout", "split", p, &base, &over, "--layout", "two_rows"],
    );
    let clips = split["clips"].as_array().unwrap();
    let y = |c: &Value| c["transform"]["position"][1].as_f64().unwrap_or(0.0);
    assert!(y(&clips[0]) > 0.0 && y(&clips[1]) < 0.0, "{clips:?}");
    let doc = document(&project);
    assert_ne!(
        segment(&doc, &base)["transform"],
        segment(&doc, &over)["transform"]
    );

    let wrong = run(&dir, &["layout", "split", p, &base, "--layout", "diagonal"]);
    assert_eq!(wrong.code, 2);
}

#[test]
fn title_styles_templates_positions_and_duplicates() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("titles");
    let p = project.to_str().unwrap();

    let styles = ok(&dir, &["catalog", "title_styles"]);
    let templates = ok(&dir, &["catalog", "title_templates"]);
    assert!(styles.as_array().unwrap().len() > 3);
    assert!(templates.as_array().unwrap().len() > 3);
    let first = styles[0]["id"].as_str().unwrap().to_string();
    let last = styles.as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let template = templates[0]["id"].as_str().unwrap().to_string();

    let added = ok(
        &dir,
        &[
            "title", "style", p, &first, "--at", "0.5", "--text", "Styled",
        ],
    );
    let id = added["clip"]["id"].as_str().unwrap().to_string();
    assert_eq!(added["clip"]["text"]["content"], "Styled");
    let before = document(&project);
    let material = segment(&before, &id)["material_id"]
        .as_str()
        .unwrap()
        .to_string();

    // Restyle: a new material with the same words.
    ok(&dir, &["title", "style", p, &last, "--clip", &id]);
    let after = document(&project);
    let restyled = segment(&after, &id)["material_id"]
        .as_str()
        .unwrap()
        .to_string();
    let text = |doc: &Value, mid: &str| {
        doc["materials"]["texts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == mid)
            .cloned()
            .unwrap()
    };
    assert_eq!(text(&after, &restyled)["content"], "Styled");
    assert_ne!(
        text(&before, &material),
        text(&after, &restyled),
        "the style changed the title"
    );

    let templated = ok(&dir, &["title", "template", p, &template, "--clip", &id]);
    assert!(templated["clip"]["animation"].is_object(), "{templated}");

    let moved = ok(
        &dir,
        &["title", "position", p, &id, "--position", "top-left"],
    );
    // The left column is the alignment; the row moves the layer up.
    let t = &moved["clip"]["transform"]["position"];
    assert!(t[1].as_f64().unwrap() > 0.0, "{t}");
    let doc = document(&project);
    let mid = segment(&doc, &id)["material_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(text(&doc, &mid)["align"], "left");

    let copy = ok(&dir, &["title", "duplicate", p, &id]);
    let copy_id = copy["clip"]["id"].as_str().unwrap().to_string();
    assert_eq!(copy["clip"]["text"]["content"], "Styled");
    assert_eq!(copy["clip"]["start"], added["clip"]["end"]);
    let doc = document(&project);
    assert_ne!(
        segment(&doc, &copy_id)["material_id"],
        segment(&doc, &id)["material_id"],
        "the copy has a material of its own"
    );
    assert_eq!(
        segment(&doc, &copy_id)["transform"],
        segment(&doc, &id)["transform"]
    );

    let from_template = ok(&dir, &["title", "template", p, &template, "--at", "3"]);
    assert_eq!(from_template["clip"]["kind"], "title");

    // `title set` still works after the restyles.
    ok(&dir, &["title", "set", p, &id, "--text", "Changed"]);
    let unknown = run(&dir, &["title", "style", p, "no-such-style"]);
    assert_eq!(unknown.code, 2);
}

#[test]
fn scene_detection_finds_the_hard_cut_and_splits_there() {
    require_ffmpeg!();
    let dir = common::scratch("scenes");
    let cut = hard_cut(&dir);
    let project = dir.join("s.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, cut.to_str().unwrap(), "--append"]);

    let found = ok(&dir, &["scenes", "detect", p, "0:0"]);
    let scenes = found["scenes"].as_array().unwrap();
    assert_eq!(scenes.len(), 1, "{found}");
    assert!((scenes[0].as_f64().unwrap() - 2.0).abs() < 0.1, "{found}");
    let shown = ok(&dir, &["analysis", p, "0:0"]);
    assert_eq!(shown["scenes"], found["scenes"]);

    ok(&dir, &["scenes", "split", p, "0:0"]);
    let info = ok(&dir, &["info", p]);
    let lane = lane_clips(&info, 0);
    assert_eq!(lane.len(), 2);
    assert!((lane[1]["start"].as_f64().unwrap() - 2.0).abs() < 0.1);

    let cleared = ok(&dir, &["scenes", "clear", p, "0:0"]);
    assert!(cleared["scenes"].as_array().unwrap().is_empty());
}

#[test]
fn beats_are_found_on_a_click_track() {
    require_ffmpeg!();
    let dir = common::scratch("beats");
    let clicks = click_track(&dir);
    let project = dir.join("b.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, clicks.to_str().unwrap(), "--append"]);
    let info = ok(&dir, &["info", p]);
    let lane = info["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .position(|t| t["kind"] == "audio" && !t["clips"].as_array().unwrap().is_empty())
        .unwrap();
    let clip = format!("{lane}:0");

    let beats = ok(&dir, &["beats", "detect", p, &clip]);
    let bpm = beats["bpm"].as_f64().unwrap();
    assert!((bpm - 120.0).abs() < 2.0, "{beats}");
    assert!(beats["beats"].as_array().unwrap().len() >= 12, "{beats}");

    let cleared = ok(&dir, &["beats", "clear", p, &clip]);
    assert!(cleared["beats"].as_array().unwrap().is_empty());
}

#[test]
fn stabilise_and_reframe_run_to_the_end() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("stabilise");
    let p = project.to_str().unwrap();

    let applied = ok(
        &dir,
        &["stabilise", "apply", p, "0:0", "--strength", "0.85"],
    );
    assert_eq!(applied["stabilise"]["enabled"], true);
    let set = ok(&dir, &["stabilise", "set", p, "0:0", "--enabled", "false"]);
    assert_eq!(set["stabilise"]["enabled"], false);
    let removed = ok(&dir, &["stabilise", "remove", p, "0:0"]);
    assert!(removed["stabilise"].is_null());
    let nothing = run(&dir, &["stabilise", "set", p, "0:0", "--strength", "0.2"]);
    assert_eq!(nothing.code, 1, "there is no stabilisation to change");

    // The 16:9 card in a 16:9 project, reframed to 9:16.
    let reframed = ok(&dir, &["reframe", p, "--ratio", "9:16"]);
    assert_eq!(reframed["canvas"]["width"], 1080);
    assert_eq!(reframed["canvas"]["height"], 1920);
    let info = ok(&dir, &["info", p]);
    assert_eq!(info["canvas"]["height"], 1920);
}

#[test]
fn cloud_commands_without_an_account_are_refused() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("cloud");
    let p = project.to_str().unwrap();
    let before = std::fs::read(&project).unwrap();

    let accounts = ok(&dir, &["catalog", "accounts"]);
    assert!(
        accounts.as_array().unwrap().is_empty(),
        "isolated XDG config"
    );

    for args in [
        vec!["cloud", "translate", p, "--to", "de"],
        vec!["cloud", "tts", p, "Hello there", "--voice", "alloy"],
        vec!["cloud", "stock-kinds", p],
        vec!["cloud", "stock-search", p, "ocean waves", "--kind", "video"],
        vec!["cloud", "stock-download", p, "ocean", "--id", "123"],
    ] {
        let refused = run(&dir, &args);
        assert_eq!(refused.code, 1, "{args:?}: {}", refused.json);
        let message = refused.json["error"]["message"].as_str().unwrap();
        assert!(message.contains("no cloud account"), "{message}");
    }

    // A named account that does not exist gets the engine's own words.
    let named = run(
        &dir,
        &["cloud", "stock-search", p, "ocean", "--account", "nope"],
    );
    assert_eq!(named.code, 1);
    assert!(named.json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no longer exists"));

    // Arguments are checked before any account is looked for.
    let kind = run(
        &dir,
        &["cloud", "stock-search", p, "ocean", "--kind", "gif"],
    );
    assert_eq!(kind.code, 2);
    let missing = run(&dir, &["cloud", "tts", p, "Hello"]);
    assert_eq!(missing.code, 2, "--voice is required");

    assert_eq!(
        std::fs::read(&project).unwrap(),
        before,
        "nothing was saved"
    );
}

#[test]
fn the_new_operations_run_from_a_batch() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("coverage-batch");
    let p = project.to_str().unwrap();
    let ops = json!([
        {"op": "marker_add", "at": 1, "label": "A"},
        {"op": "crop", "clip": "0:0", "top": 0.2},
        {"op": "curve", "clip": "0:0", "points": [[0, 0], "0.5,0.7", [1, 1]]},
        {"op": "speed_curve", "clip": "0:0", "points": [{"at": 0, "speed": 1}, "1s=2"]},
        {"op": "speed_curve", "clip": "0:0", "remove": true},
        {"op": "title_style", "style": ok(&dir, &["catalog", "title_styles"])[0]["id"], "at": 0},
        {"op": "marker_list"},
        {"op": "undo"},
    ]);
    let file = dir.join("ops.json");
    std::fs::write(&file, ops.to_string()).unwrap();
    let results = ok(&dir, &["batch", p, file.to_str().unwrap()]);
    assert_eq!(results.as_array().unwrap().len(), 8);
    assert_eq!(results[6]["data"][0]["label"], "A");

    let doc = document(&project);
    assert_eq!(doc["markers"].as_array().unwrap().len(), 1);
    let id = ok(&dir, &["info", p])["tracks"][0]["clips"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!((segment(&doc, &id)["crop"]["top"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    let info = ok(&dir, &["info", p]);
    assert!(
        info["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["kind"] != "text" || t["clips"].as_array().unwrap().is_empty()),
        "the styled title was undone"
    );
}

/// A stand-in for an OpenAI-compatible server on 127.0.0.1: chat
/// completions translate each line to "DE: <line>", and speech answers with
/// `voice`, an MP3 file. Nothing leaves this machine.
fn mock_openai(voice: Vec<u8>) -> u16 {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':') {
                    if k.eq_ignore_ascii_case("content-length") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let (kind, reply): (&str, Vec<u8>) = if request_line.contains("/chat/completions") {
                let request: Value = serde_json::from_slice(&body).unwrap_or_default();
                let lines: Vec<String> = request["messages"][1]["content"]
                    .as_str()
                    .and_then(|c| serde_json::from_str(c).ok())
                    .unwrap_or_default();
                let translated: Vec<String> = lines.iter().map(|l| format!("DE: {l}")).collect();
                let answer = json!({"choices": [{"message": {"role": "assistant",
                    "content": serde_json::to_string(&translated).unwrap()}}]});
                ("application/json", answer.to_string().into_bytes())
            } else if request_line.contains("/audio/speech") {
                ("audio/mpeg", voice.clone())
            } else {
                ("text/plain", b"not found".to_vec())
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&reply);
        }
    });
    port
}

/// Run with `--json`, without any proxy between the CLI and the mock.
fn run_local(dir: &Path, args: &[&str]) -> Value {
    let output = common::cli(dir)
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?}: no JSON ({e}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(output.status.code(), Some(0), "{args:?}: {json}");
    json["data"].clone()
}

#[test]
fn translation_and_tts_work_against_a_local_mock_account() {
    require_ffmpeg!();
    let (dir, project, _, _) = with_card("cloud-mock");
    let p = project.to_str().unwrap();
    let mp3 = dir.join("voice.mp3");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=300:duration=1.5",
        mp3.to_str().unwrap(),
    ]);
    let port = mock_openai(std::fs::read(&mp3).unwrap());
    let config = dir.join("xdg/config/chukcut");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("accounts.toml"),
        format!(
            "[[account]]\nid = \"mock\"\nkind = \"openai_compatible\"\nname = \"Mock\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\ndefault_model = \"m\"\n"
        ),
    )
    .unwrap();

    let accounts = ok(&dir, &["catalog", "accounts"]);
    assert_eq!(accounts[0]["id"], "mock");

    let srt = dir.join("subs.srt");
    std::fs::write(
        &srt,
        "1\n00:00:00,000 --> 00:00:01,000\nHello\n\n2\n00:00:01,500 --> 00:00:03,000\nGood night\n",
    )
    .unwrap();
    ok(&dir, &["captions", "import", p, srt.to_str().unwrap()]);

    let translated = run_local(
        &dir,
        &["cloud", "translate", p, "--to", "de", "--model", "m"],
    );
    assert_eq!(translated["lines"], 2);
    let track = translated["track_id"].as_str().unwrap();
    let doc = document(&project);
    let lane = doc["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == track)
        .unwrap();
    assert_eq!(lane["segments"].as_array().unwrap().len(), 2);
    let texts: Vec<&str> = doc["materials"]["texts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["content"].as_str())
        .collect();
    assert!(texts.contains(&"DE: Good night"), "{texts:?}");

    let spoken = run_local(
        &dir,
        &[
            "cloud",
            "tts",
            p,
            "Hello there",
            "--voice",
            "alloy",
            "--at",
            "0.5",
        ],
    );
    assert_eq!(spoken["material"]["kind"], "audio");
    assert_eq!(spoken["clip"]["start"], 0.5);
    assert_eq!(spoken["clip"]["kind"], "audio");
    let path = PathBuf::from(spoken["path"].as_str().unwrap());
    assert!(
        path.starts_with(dir.join("xdg/data")),
        "the voice goes under the data folder: {}",
        path.display()
    );
}
