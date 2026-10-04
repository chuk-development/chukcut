//! "Remove object" and "Enhance quality" from the command line: the
//! settings, set without baking (no model is needed here), read back, and
//! switched off. The bakes themselves are the engine's tests
//! (`crates/engine/tests/enhance.rs`).

mod common;

use common::{ok, run};
use serde_json::Value;

fn document(project: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap()
}

#[test]
fn remove_object_and_enhance_quality_settings() {
    require_ffmpeg!();
    let dir = common::scratch("enhance");
    let (card, _) = common::media(&dir);
    let project = dir.join("enhance.chukcut");
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);

    // Nothing yet.
    let read = ok(&dir, &["remove-object", p, "0:0"]);
    assert!(read["settings"]["removal"].is_null(), "{read}");

    // A box and a stroke, set without baking.
    let set = ok(
        &dir,
        &[
            "remove-object",
            p,
            "0:0",
            "--box",
            "0.7,0.05,0.2,0.1",
            "--stroke",
            "0.1,0.9 0.3,0.9",
            "--radius",
            "0.03",
            "--no-bake",
        ],
    );
    let removal = &set["settings"]["removal"];
    assert_eq!(removal["boxes"][0][2].as_f64().unwrap() as f32, 0.2);
    assert_eq!(removal["strokes"][0]["points"].as_array().unwrap().len(), 2);
    assert_eq!(removal["model"], "lama");
    assert_eq!(set["coverage"]["total"], 120, "{set}");
    assert!(set["estimate"]["seconds_gpu"].as_f64().unwrap() > 0.0);
    let extras = document(&project)["materials"]["extras"].to_string();
    assert!(extras.contains("object_removal"), "{extras}");

    // --add keeps the box; --grow alone regrows the same mask.
    let added = ok(
        &dir,
        &[
            "remove-object",
            p,
            "0:0",
            "--box",
            "0.1,0.1,0.1,0.1",
            "--add",
            "--no-bake",
        ],
    );
    assert_eq!(
        added["settings"]["removal"]["boxes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let grown = ok(
        &dir,
        &["remove-object", p, "0:0", "--grow", "0.05", "--no-bake"],
    );
    assert_eq!(
        grown["settings"]["removal"]["grow"].as_f64().unwrap() as f32,
        0.05
    );
    // A box outside the picture is a usage error; a click needs its time.
    assert_eq!(
        run(
            &dir,
            &["remove-object", p, "0:0", "--box", "0.9,0.1,1.5,0.1"]
        )
        .code,
        2
    );
    assert_eq!(
        run(&dir, &["remove-object", p, "0:0", "--point", "0.5,0.5"]).code,
        2
    );

    // Enhance quality: 2x, set without baking; 3x does not exist.
    let up = ok(
        &dir,
        &["enhance-quality", p, "0:0", "--scale", "2", "--no-bake"],
    );
    assert_eq!(up["settings"]["upscale"]["scale"], 2);
    assert_eq!(
        run(&dir, &["enhance-quality", p, "0:0", "--scale", "3"]).code,
        2
    );
    assert_eq!(
        ok(&dir, &["enhance-quality", p, "0:0"])["settings"]["upscale"]["scale"],
        2
    );

    // Both off again.
    ok(&dir, &["enhance-quality", p, "0:0", "--scale", "off"]);
    ok(&dir, &["remove-object", p, "0:0", "--off"]);
    let off = ok(&dir, &["remove-object", p, "0:0"]);
    assert!(off["settings"]["removal"].is_null());
    assert!(off["settings"]["upscale"].is_null());
}
