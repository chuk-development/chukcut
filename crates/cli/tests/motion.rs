//! Frame blending, motion blur and animated stickers from the command line.

mod common;

use common::{ok, run};
use serde_json::Value;

/// A 100×50, 2 s Lottie: a red box sliding left to right.
const SLIDE: &str = r#"{"v":"5.7.0","fr":30,"ip":0,"op":60,"w":100,"h":50,"nm":"slide","ddd":0,"assets":[],
"layers":[{"ddd":0,"ind":1,"ty":4,"nm":"box","sr":1,"ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},
"p":{"a":1,"k":[{"t":0,"s":[25,25,0],"i":{"x":[1],"y":[1]},"o":{"x":[0],"y":[0]}},{"t":60,"s":[75,25,0]}]},
"a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]}},"ao":0,"ip":0,"op":60,"st":0,"bm":0,
"shapes":[{"ty":"gr","nm":"g","it":[{"ty":"rc","nm":"r","d":1,"s":{"a":0,"k":[50,50]},"p":{"a":0,"k":[0,0]},"r":{"a":0,"k":0}},
{"ty":"fl","nm":"f","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100},"r":1},
{"ty":"tr","p":{"a":0,"k":[0,0]},"a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100}}]}]}]}"#;

fn document(project: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap()
}

#[test]
fn frame_blending_motion_blur_and_an_animated_sticker() {
    require_ffmpeg!();
    let dir = common::scratch("motion");
    let (card, _) = common::media(&dir);
    let project = dir.join("motion.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);

    // Frame blending: read, switch on, optical flow, switch off.
    assert_eq!(ok(&dir, &["frame-blend", p, "0:0"])["frame_blend"], "none");
    assert_eq!(
        ok(&dir, &["frame-blend", p, "0:0", "--mode", "blend"])["frame_blend"],
        "blend"
    );
    let extras = document(&project)["materials"]["extras"].to_string();
    assert!(extras.contains("frame_blend"), "{extras}");
    // Optical flow, set without baking (no model needed here); a mode
    // that does not exist is a usage error.
    assert_eq!(
        ok(
            &dir,
            &["frame-blend", p, "0:0", "--mode", "flow", "--no-bake"]
        )["frame_blend"],
        "flow"
    );
    assert!(document(&project)["materials"]["extras"]
        .to_string()
        .contains("\"flow\""));
    assert_eq!(
        run(&dir, &["frame-blend", p, "0:0", "--mode", "sideways"]).code,
        2
    );
    assert_eq!(
        ok(&dir, &["frame-blend", p, "0:0", "--mode", "none"])["frame_blend"],
        "none"
    );

    // Motion blur is a catalogue effect.
    let effects = ok(&dir, &["catalog", "effects"]).to_string();
    assert!(effects.contains("motion_blur"));
    ok(
        &dir,
        &[
            "effect",
            "add",
            p,
            "motion_blur",
            "--clip",
            "0:0",
            "--set",
            "shutter=270",
        ],
    );
    assert!(document(&project)["materials"]["effects"]
        .to_string()
        .contains("motion_blur"));

    // An animated sticker of our own: a Lottie, played once.
    let lottie = dir.join("slide.json");
    std::fs::write(&lottie, SLIDE).unwrap();
    let sticker = ok(
        &dir,
        &[
            "sticker",
            p,
            "--file",
            lottie.to_str().unwrap(),
            "--at",
            "0.5",
            "--once",
        ],
    );
    assert_eq!(sticker["clip"]["kind"], "image");
    let id = sticker["clip"]["id"].as_str().unwrap().to_string();
    let playback = ok(&dir, &["sticker-playback", p, &id]);
    assert_eq!(playback["playback"], "once");
    assert_eq!(playback["animation"]["format"], "lottie");
    assert_eq!(playback["animation"]["duration"], 2_000_000);
    assert_eq!(
        ok(&dir, &["sticker-playback", p, &id, "--mode", "loop"])["playback"],
        "loop"
    );
    // A still is not an animated sticker; a broken Lottie does not import.
    assert_eq!(run(&dir, &["sticker-playback", p, "0:0"]).code, 2);
    let broken = dir.join("broken.json");
    std::fs::write(&broken, "{\"hello\": 1}").unwrap();
    assert_ne!(
        run(&dir, &["sticker", p, "--file", broken.to_str().unwrap()]).code,
        0
    );
}
