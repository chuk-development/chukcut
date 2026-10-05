//! A damaged or hand-edited project is refused before it renders: the CLI
//! runs the app's own check (`Project::render_check`) on `export` and
//! `render-frame`. Before, overlapping clips, speed 0 and fps 0 exported
//! with exit 0, and a clip at 9·10¹⁸ µs aborted the export with a core dump.

mod common;

use std::path::{Path, PathBuf};

use common::{ok, run};
use serde_json::Value;

fn project_with_card(name: &str) -> (PathBuf, PathBuf) {
    let dir = common::scratch(name);
    let (card, _) = common::media(&dir);
    let project = dir.join(format!("{name}.chukcut"));
    let p = project.to_str().unwrap();
    ok(&dir, &["new", p]);
    ok(&dir, &["import", p, card.to_str().unwrap(), "--append"]);
    (dir, project)
}

/// Break the file the way a hand edit would, outside the CLI.
fn damage(project: &Path, edit: impl FnOnce(&mut Value)) {
    let mut doc: Value = serde_json::from_slice(&std::fs::read(project).unwrap()).unwrap();
    edit(&mut doc);
    std::fs::write(project, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}

fn first_clip(doc: &mut Value) -> &mut Value {
    doc["tracks"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|t| !t["segments"].as_array().unwrap().is_empty())
        .map(|t| &mut t["segments"][0])
        .expect("a clip")
}

type Damage = Box<dyn Fn(&mut Value)>;

fn refuses(dir: &Path, project: &Path, what: &str) {
    let p = project.to_str().unwrap();
    let out = dir.join("out.mp4");
    let png = dir.join("frame.png");
    for args in [
        vec![
            "export",
            p,
            out.to_str().unwrap(),
            "--from",
            "0",
            "--to",
            "1",
        ],
        vec!["render-frame", p, "--at", "0.5", png.to_str().unwrap()],
    ] {
        let result = run(dir, &args);
        assert_ne!(result.code, 0, "{what}: {args:?} succeeded");
        let message = result.json["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains("must be fixed before it can render"),
            "{what}: {args:?} failed for another reason: {message}"
        );
    }
    assert!(!out.exists(), "{what}: a file was written");
    // Refused, not locked out: the project still opens and says why.
    let issues = run(dir, &["validate", p]);
    assert!(issues.json.to_string().contains("error"), "{what}");
}

#[test]
fn a_damaged_project_is_refused_before_it_renders() {
    require_ffmpeg!();
    let (dir, project) = project_with_card("guard");

    let pristine = std::fs::read(&project).unwrap();
    let cases: Vec<(&str, Damage)> = vec![
        ("fps 0", Box::new(|doc: &mut Value| doc["fps"] = 0.into())),
        (
            "fps 100000",
            Box::new(|doc: &mut Value| doc["fps"] = 100_000.into()),
        ),
        (
            "canvas 0 wide",
            Box::new(|doc: &mut Value| doc["canvas"]["width"] = 0.into()),
        ),
        (
            "speed 0",
            Box::new(|doc: &mut Value| first_clip(doc)["speed"] = 0.0.into()),
        ),
        (
            "a clip at 9e18 µs",
            Box::new(|doc: &mut Value| {
                first_clip(doc)["target_range"]["start"] = 9_000_000_000_000_000_000i64.into()
            }),
        ),
        (
            "overlapping clips",
            Box::new(|doc: &mut Value| {
                let lane = doc["tracks"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|t| !t["segments"].as_array().unwrap().is_empty())
                    .unwrap();
                let mut copy = lane["segments"][0].clone();
                copy["id"] = "overlapping-copy".into();
                copy["target_range"]["start"] = 500_000.into();
                lane["segments"].as_array_mut().unwrap().push(copy);
            }),
        ),
    ];
    for (what, edit) in cases {
        std::fs::write(&project, &pristine).unwrap();
        damage(&project, edit);
        refuses(&dir, &project, what);
    }

    // And the undamaged file still renders.
    std::fs::write(&project, &pristine).unwrap();
    let png = dir.join("frame.png");
    let result = run(
        &dir,
        &[
            "render-frame",
            project.to_str().unwrap(),
            "--at",
            "0.5",
            png.to_str().unwrap(),
        ],
    );
    if !common::no_gpu(&result) {
        assert_eq!(result.code, 0, "{}", result.json);
    }
}
