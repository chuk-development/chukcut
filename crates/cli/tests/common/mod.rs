//! Running the real `chukcut-cli` binary against generated media.
//!
//! Every run gets its own XDG directories under the test's scratch folder, so
//! a test can never touch the user's settings, LUT library, caches or working
//! copy, and two tests never see each other's.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// A scratch directory of the test's own, emptied first.
pub fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("chukcut-cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("xdg")).expect("scratch directory");
    dir
}

pub fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `let Some(..) = require_ffmpeg!()` — skip, with a reason, when the
/// fixture generator or the checker is missing.
#[macro_export]
macro_rules! require_ffmpeg {
    () => {
        if !common::have("ffmpeg") || !common::have("ffprobe") {
            eprintln!("skipping: ffmpeg and ffprobe are needed to make and check media");
            return;
        }
    };
}

/// Four seconds of moving test card with a tone, and three of another card,
/// both 640x360 at 30 fps.
#[rustfmt::skip]
pub fn media(dir: &Path) -> (PathBuf, PathBuf) {
    let a = dir.join("card.mp4");
    let b = dir.join("bars.mp4");
    let run = |args: &[&str]| {
        let status = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-y"])
            .args(args)
            .status()
            .expect("ffmpeg runs");
        assert!(status.success(), "ffmpeg failed: {args:?}");
    };
    run(&[
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30:duration=4",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=4",
        "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest",
        a.to_str().unwrap(),
    ]);
    run(&[
        "-f", "lavfi", "-i", "smptebars=size=640x360:rate=30:duration=3",
        "-c:v", "libx264", "-pix_fmt", "yuv420p",
        b.to_str().unwrap(),
    ]);
    (a, b)
}

/// The binary, with the scratch directory's XDG folders.
pub fn cli(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_chukcut-cli"));
    let xdg = dir.join("xdg");
    command
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("XDG_CACHE_HOME", xdg.join("cache"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_STATE_HOME", xdg.join("state"))
        .env_remove("RUST_LOG");
    command
}

pub struct Run {
    pub code: i32,
    pub json: Value,
    pub stderr: String,
}

/// Run `args` with `--json` and parse what it printed.
pub fn run(dir: &Path, args: &[&str]) -> Run {
    let output: Output = cli(dir)
        .arg("--json")
        .args(args)
        .output()
        .expect("chukcut-cli runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{args:?} printed no JSON ({e}):\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    Run {
        code: output.status.code().unwrap_or(-1),
        json,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Run and require success; returns the `data`.
pub fn ok(dir: &Path, args: &[&str]) -> Value {
    let run = run(dir, args);
    assert_eq!(
        run.code, 0,
        "{args:?} failed: {}\nstderr:\n{}",
        run.json, run.stderr
    );
    assert_eq!(run.json["ok"], true);
    run.json["data"].clone()
}

/// Whether a failure was this machine having no GPU to render with, which
/// is a reason to skip, not a failure.
pub fn no_gpu(run: &Run) -> bool {
    run.json["error"]["message"]
        .as_str()
        .is_some_and(|m| m.contains("no GPU"))
}

/// What ffprobe says about a file's streams and duration.
pub struct Probe {
    pub width: u64,
    pub height: u64,
    pub frames: u64,
    pub duration: f64,
    pub audio: bool,
}

pub fn probe(path: &Path) -> Probe {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-print_format", "json"])
        .args(["-show_streams", "-show_format"])
        .arg(path)
        .output()
        .expect("ffprobe runs");
    assert!(
        output.status.success(),
        "ffprobe failed on {}",
        path.display()
    );
    let v: Value = serde_json::from_slice(&output.stdout).expect("ffprobe JSON");
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .expect("a video stream");
    let number = |v: &Value| -> u64 {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0)
    };
    Probe {
        width: number(&video["width"]),
        height: number(&video["height"]),
        frames: number(&video["nb_read_frames"]),
        duration: v["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        audio: streams.iter().any(|s| s["codec_type"] == "audio"),
    }
}
