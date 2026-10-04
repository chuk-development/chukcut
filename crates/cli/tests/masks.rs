//! Masks, chroma key and blend modes from the command line: a green-screen
//! clip over bars, keyed by picking its colour, masked, blended, and the
//! rendered frame read back pixel by pixel.

mod common;

use std::path::Path;
use std::process::Command;

use common::{no_gpu, ok, run};
use serde_json::json;

/// The green screen: a red box on 0x14c828, three seconds.
fn green_screen(dir: &Path) -> std::path::PathBuf {
    let out = dir.join("green.mp4");
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("color=c=0x14c828:size=640x360:rate=30:duration=3")
        .args([
            "-vf",
            "drawbox=x=220:y=100:w=200:h=160:color=0xd02020:t=fill",
        ])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success());
    out
}

/// A rendered PNG as RGB rows, through ffmpeg.
fn pixels(png: &Path) -> (usize, Vec<u8>) {
    let output = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-i"])
        .arg(png)
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("ffmpeg runs");
    assert!(output.status.success());
    (1920, output.stdout)
}

fn at(frame: &(usize, Vec<u8>), x: usize, y: usize) -> [u8; 3] {
    let i = (y * frame.0 + x) * 3;
    [frame.1[i], frame.1[i + 1], frame.1[i + 2]]
}

#[test]
fn key_mask_and_blend_a_green_screen_clip() {
    require_ffmpeg!();
    let dir = common::scratch("masks");
    let (_, bars) = common::media(&dir);
    let green = green_screen(&dir);
    let project = dir.join("masks.chukcut");
    let p = project.to_str().unwrap();

    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, bars.to_str().unwrap(), "--append"]);
    ok(&dir, &["import", p, green.to_str().unwrap()]);
    ok(&dir, &["place", p, "green.mp4", "--at", "0"]);
    let info = ok(&dir, &["info", p]);
    assert!(
        info["tracks"][1]["clips"][0]["source"]
            .as_str()
            .unwrap()
            .ends_with("green.mp4"),
        "{info}"
    );
    let clip = "1:0";

    let catalog = ok(&dir, &["catalog", "masks"]);
    assert!(catalog["shapes"]
        .as_array()
        .unwrap()
        .contains(&json!("heart")));
    let modes = ok(&dir, &["catalog", "blend"]);
    assert_eq!(modes.as_array().unwrap().len(), 14);

    // The eyedropper renders a frame; skip the rest without a GPU.
    let picked = run(
        &dir,
        &["chroma-key", p, clip, "--pick", "0.05,0.5", "--at", "1"],
    );
    if no_gpu(&picked) {
        eprintln!("skipping: no GPU on this machine");
        return;
    }
    assert_eq!(picked.code, 0, "{}", picked.json);
    let key = &picked.json["data"]["compositing"]["key"];
    let hex = key["color"].as_str().unwrap();
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    assert!(byte(3) > 150 && byte(1) < 80, "picked {hex}, not the green");

    let frame = dir.join("keyed.png");
    ok(
        &dir,
        &["render-frame", p, "--at", "1", frame.to_str().unwrap()],
    );
    let keyed = pixels(&frame);
    let edge = at(&keyed, 100, 540);
    assert!(
        !(edge[1] > 150 && edge[0] < 80),
        "the green is still there: {edge:?}"
    );
    let centre = at(&keyed, 960, 540);
    assert!(
        centre[0] > 150 && centre[1] < 80,
        "the red box was keyed: {centre:?}"
    );

    // A circle around the box, with a small square cut out of its middle.
    let masked = ok(
        &dir,
        &[
            "mask",
            p,
            clip,
            "--add",
            "circle",
            "--set",
            "width=0.8",
            "--set",
            "height=0.8",
        ],
    );
    assert_eq!(masked["compositing"]["masks"][0]["shape"], "ellipse");
    ok(
        &dir,
        &[
            "mask",
            p,
            clip,
            "--add",
            "rectangle",
            "--op",
            "subtract",
            "--set",
            "width=0.1",
            "--set",
            "height=0.1",
        ],
    );
    let frame = dir.join("masked.png");
    ok(
        &dir,
        &["render-frame", p, "--at", "1", frame.to_str().unwrap()],
    );
    let masked = pixels(&frame);
    let hole = at(&masked, 960, 540);
    assert!(
        !(hole[0] > 150 && hole[1] < 80),
        "the subtracted square still shows red: {hole:?}"
    );
    let ring = at(&masked, 960 + 150, 540);
    assert!(
        ring[0] > 150 && ring[1] < 80,
        "the box outside the hole is gone: {ring:?}"
    );

    // Animate the first mask, then blend the clip.
    let animated = ok(
        &dir,
        &[
            "mask", p, clip, "--mask", "0", "--set", "x=-0.2", "--at", "0",
        ],
    );
    assert_eq!(
        animated["compositing"]["masks"][0]["animated"],
        json!(["x"])
    );
    let blended = ok(&dir, &["blend", p, clip, "multiply", "--opacity", "0.8"]);
    assert_eq!(blended["compositing"]["blend"], "multiply");
    let opacity = blended["clip"]["transform"]["opacity"].as_f64().unwrap();
    assert!((opacity - 0.8).abs() < 1e-6, "{blended}");
    // `info` carries the same description.
    let info = ok(&dir, &["info", p]);
    assert_eq!(
        info["tracks"][1]["clips"][0]["compositing"]["blend"],
        "multiply"
    );
    ok(&dir, &["validate", p]);

    // In one session, an added mask undoes back to the file as it was.
    let ops = dir.join("ops.json");
    std::fs::write(
        &ops,
        json!([
            {"op": "mask", "clip": clip, "add": "heart"},
            {"op": "undo"},
            {"op": "chroma_key", "clip": clip, "off": true},
        ])
        .to_string(),
    )
    .unwrap();
    let batch = ok(&dir, &["batch", p, ops.to_str().unwrap()]);
    let last = &batch.as_array().unwrap()[2]["data"]["compositing"];
    assert_eq!(last["masks"].as_array().unwrap().len(), 2, "{batch}");
    assert!(last["key"].is_null());
}

/// The ML operations' arguments are checked before any model is touched:
/// a background model that does not exist, a selection without a point on
/// the object, points outside the canvas, a playhead off the clip. And
/// `ml status` / `ml bundles` say what runs and what can be installed
/// without starting the worker.
#[test]
fn ml_operations_refuse_bad_arguments_and_status_says_what_runs() {
    require_ffmpeg!();
    let dir = common::scratch("ml-args");
    let (_, bars) = common::media(&dir);
    let project = dir.join("ml.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, bars.to_str().unwrap(), "--append"]);

    let bad_model = run(&dir, &["remove-background", p, "0:0", "--model", "cats"]);
    assert_ne!(bad_model.code, 0);
    let message = bad_model.json["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("people or objects"), "{message}");

    let no_point = run(&dir, &["select-object", p, "0:0", "--at", "1"]);
    assert_ne!(no_point.code, 0);
    let outside = run(
        &dir,
        &["select-object", p, "0:0", "--at", "1", "--point", "1.5,0.2"],
    );
    assert_ne!(outside.code, 0);
    let message = outside.json["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("inside the canvas"), "{message}");
    let off_clip = run(
        &dir,
        &[
            "select-object",
            p,
            "0:0",
            "--at",
            "30",
            "--point",
            "0.5,0.5",
        ],
    );
    assert_ne!(off_clip.code, 0);
    let message = off_clip.json["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains("playhead"), "{message}");
    // Nothing was recorded by the refusals.
    let info = ok(&dir, &["info", p]);
    assert!(!info.to_string().contains("mobilesam"), "{info}");

    let status = ok(&dir, &["ml", "status"]);
    assert!(status["active"].is_string(), "{status}");
    assert!(status["mattes"]["bytes"].is_u64(), "{status}");
    let bundles = ok(&dir, &["ml", "bundles"]);
    let ids: Vec<&str> = bundles
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|b| b["id"].as_str())
        .collect();
    assert_eq!(ids, ["nvidia-cu13", "nvidia-cu12"]);
    assert!(bundles[0]["bytes"].as_u64().unwrap() > 500_000_000);
    let unknown = run(&dir, &["ml", "install", "gpu:nvidia-cu11"]);
    assert_ne!(unknown.code, 0);
}
