//! Delivery from the command line: presets, user presets, the estimate and
//! the export queue, on the real binary with generated media.

mod common;

use std::path::Path;
use std::process::Command;

use common::{ok, run};
use serde_json::Value;

/// A 9:16 project with a four-second clip on it.
fn vertical(dir: &Path) -> String {
    let (card, _) = common::media(dir);
    ok(
        dir,
        &[
            "new",
            "p.chukcut",
            "--width",
            "360",
            "--height",
            "640",
            "--fps",
            "30",
        ],
    );
    ok(
        dir,
        &["import", "p.chukcut", card.to_str().unwrap(), "--append"],
    );
    // An unchosen canvas takes the first clip's 16:9 shape; choose 9:16.
    ok(
        dir,
        &[
            "configure",
            "p.chukcut",
            "--width",
            "360",
            "--height",
            "640",
        ],
    );
    "p.chukcut".into()
}

fn codec_of(path: &Path, kind: &str) -> Option<String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", &format!("{kind}:0")])
        .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[test]
fn presets_fit_the_canvas_and_warn_about_the_wrong_shape() {
    require_ffmpeg!();
    let dir = common::scratch("delivery-presets");
    let project = vertical(&dir);
    let data = ok(&dir, &["presets", &project]);
    let presets = data["presets"].as_array().unwrap();
    let find = |id: &str| -> &Value {
        presets
            .iter()
            .find(|p| p["id"] == id)
            .unwrap_or_else(|| panic!("no preset {id}"))
    };
    for id in [
        "tiktok",
        "instagram_reels",
        "youtube_shorts",
        "x_twitter",
        "youtube_1080p",
        "youtube_4k",
        "master_prores",
        "audio_mp3",
        "gif",
    ] {
        assert!(find(id)["estimated_bytes"].as_u64().unwrap() > 0, "{id}");
    }
    let tiktok = find("tiktok");
    assert_eq!(
        (tiktok["width"].as_u64(), tiktok["height"].as_u64()),
        (Some(1080), Some(1920))
    );
    assert_eq!(tiktok["loudness_target"], -14.0);
    assert!(tiktok["warnings"].as_array().unwrap().is_empty());
    // A 9:16 canvas is not what YouTube shows full screen.
    assert_eq!(
        find("youtube_1080p")["warnings"].as_array().unwrap().len(),
        1
    );
    // The master is the canvas itself.
    let master = find("master_prores");
    assert_eq!(
        (master["width"].as_u64(), master["height"].as_u64()),
        (Some(360), Some(640))
    );
}

#[test]
fn a_saved_preset_is_listed_used_and_removed() {
    require_ffmpeg!();
    let dir = common::scratch("delivery-user-preset");
    let project = vertical(&dir);
    let saved = ok(
        &dir,
        &[
            "preset",
            "save",
            &project,
            "Small reel",
            "--preset",
            "instagram_reels",
            "--width",
            "360",
            "--height",
            "640",
            "--crf",
            "30",
        ],
    );
    assert_eq!(saved["preset"]["id"], "user_small_reel");
    assert_eq!(saved["preset"]["quality"]["value"], 30);

    let listed = ok(&dir, &["presets", &project]);
    let mine = listed["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "user_small_reel")
        .expect("the preset is listed")
        .clone();
    assert_eq!(mine["user"], true);
    assert_eq!(
        (mine["width"].as_u64(), mine["height"].as_u64()),
        (Some(360), Some(640))
    );

    let removed = run(&dir, &["preset", "remove", &project, "user_small_reel"]);
    assert_eq!(removed.code, 0);
    let again = run(&dir, &["preset", "remove", &project, "user_small_reel"]);
    assert_eq!(again.code, 1, "removing it twice is refused");
    // Settings that cannot be encoded are not saved.
    let broken = run(
        &dir,
        &[
            "preset",
            "save",
            &project,
            "Broken",
            "--codec",
            "vp9",
            "--container",
            "mp4",
        ],
    );
    assert_eq!(broken.code, 1, "{}", broken.json);
}

#[test]
fn the_estimate_is_arithmetic_for_sound_and_measured_for_crf() {
    require_ffmpeg!();
    let dir = common::scratch("delivery-estimate");
    let project = vertical(&dir);
    let wav = ok(&dir, &["estimate", &project, "--preset", "audio_wav"]);
    assert_eq!(wav["estimate"]["method"], "exact");
    // 4 s of 48 kHz stereo 16-bit, and a 44-byte header.
    assert_eq!(wav["estimate"]["bytes"], 4 * 48_000 * 4 + 44);
    assert_eq!(wav["audio_only"], true);

    let rough = ok(
        &dir,
        &["estimate", &project, "--width", "360", "--height", "640"],
    );
    assert_eq!(rough["estimate"]["method"], "table");
    let measured = run(
        &dir,
        &[
            "estimate", &project, "--width", "360", "--height", "640", "--sample",
        ],
    );
    if common::no_gpu(&measured) {
        eprintln!("skipping the sampled half: no GPU");
        return;
    }
    assert_eq!(measured.code, 0, "{}", measured.json);
    assert_eq!(measured.json["data"]["estimate"]["method"], "sampled");
    let estimate = measured.json["data"]["estimate"]["bytes"].as_u64().unwrap();

    // The real file, for the comparison the estimate is for.
    let exported = run(
        &dir,
        &[
            "export",
            &project,
            "out/real.mp4",
            "--width",
            "360",
            "--height",
            "640",
        ],
    );
    assert_eq!(exported.code, 0, "{}", exported.json);
    let bytes = exported.json["data"]["bytes"].as_u64().unwrap();
    let error = (estimate as f64 - bytes as f64).abs() / bytes as f64;
    assert!(
        error <= 0.25,
        "estimated {estimate} for a {bytes}-byte file"
    );
}

#[test]
fn the_queue_exports_several_presets_and_jobs_and_checks_them_all_first() {
    require_ffmpeg!();
    let dir = common::scratch("delivery-queue");
    let project = vertical(&dir);
    let queued = run(
        &dir,
        &[
            "export-queue",
            &project,
            "--presets",
            "audio_mp3,gif",
            "--from",
            "1",
            "--to",
            "3",
            "--out-dir",
            "out",
        ],
    );
    if common::no_gpu(&queued) {
        eprintln!("skipping: no GPU");
        return;
    }
    assert_eq!(queued.code, 0, "{}", queued.json);
    let exports = queued.json["data"]["exports"].as_array().unwrap();
    assert_eq!(exports.len(), 2);
    let mp3 = dir.join("out/p-audio_mp3.mp3");
    let gif = dir.join("out/p-gif.gif");
    assert_eq!(codec_of(&mp3, "a").as_deref(), Some("mp3"));
    assert_eq!(codec_of(&mp3, "v"), None);
    assert_eq!(codec_of(&gif, "v").as_deref(), Some("gif"));
    let duration = common::probe(&gif).duration;
    assert!(
        (duration - 2.0).abs() < 0.2,
        "the range is 2 s, the GIF {duration} s"
    );

    // A job list with a broken second job: nothing is exported at all.
    let jobs = dir.join("jobs.json");
    std::fs::write(
        &jobs,
        r#"[{"output":"out/first.mp4","preset":"x_twitter"},
            {"output":"out/second.mp4","preset":"no_such_preset"}]"#,
    )
    .unwrap();
    let refused = run(
        &dir,
        &["export-queue", &project, "--jobs", jobs.to_str().unwrap()],
    );
    assert_eq!(refused.code, 1, "{}", refused.json);
    assert!(refused.json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("job 1"));
    assert!(!dir.join("out/first.mp4").exists(), "no job may start");
}
