//! The media the suite measures itself against, generated locally by ffmpeg.
//!
//! The existing one-off benchmarks were pointed at `~/git/editing/footage`,
//! which is why none of the numbers in `docs/STATUS.md` can be reproduced on
//! another machine, or on this one after that directory moves. Everything here
//! is generated from `lavfi` sources and cached under `target/bench-media/`, so
//! a fresh clone produces comparable numbers with no setup.
//!
//! ## Why the clips look the way they do
//!
//! - **`testsrc2` plus noise, not a gradient.** A smooth synthetic source is a
//!   best case for every entropy coder and would flatter the decoders by a
//!   factor of two or more. The noise filter puts real high-frequency residual
//!   in the picture so the bitrate — and therefore the decode cost — is in the
//!   region real footage occupies. It is what makes fixture generation slow;
//!   it is also what makes the numbers mean anything.
//! - **Half-second GOPs (`-g 15`).** Long GOPs would make the random-access
//!   measurement dominated by one arbitrary encoder setting. Half a second is
//!   what phone footage and most delivery encoders use.
//! - **Both orientations.** `docs/STATUS.md` records 1920×1080 and 1080×1920
//!   behaving differently — VP9 software decode is 15.1 ms one way and 38.4 ms
//!   the other — so a suite that measured one of them would miss the larger
//!   number entirely.
//! - **An audio track.** The export benchmark is end to end, and an export with
//!   no audio skips the mixer, which is part of what an export costs.
//!
//! Nothing here touches the network and nothing prompts. A codec whose encoder
//! is missing from the local ffmpeg produces a named skip, not a failure.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One generated clip.
#[derive(Clone, Debug)]
pub struct Fixture {
    /// `h264`, `hevc`, `vp9`, `av1` — the codec as `probe` reports it.
    pub codec: &'static str,
    pub width: u32,
    pub height: u32,
    pub path: PathBuf,
}

impl Fixture {
    /// `h264 1920x1080`, which is what appears in the result table.
    pub fn label(&self) -> String {
        format!("{} {}x{}", self.codec, self.width, self.height)
    }
}

struct Recipe {
    codec: &'static str,
    /// The ffmpeg encoder this needs, checked against `-encoders` before use.
    encoder: &'static str,
    video_args: &'static [&'static str],
    audio_args: &'static [&'static str],
    extension: &'static str,
}

/// Four codecs, chosen because they are the four this chip can hardware-decode
/// and therefore the four where software and hardware can be compared.
///
/// The encoder presets are all fast rather than good. Nothing here measures
/// encoding quality, and a `libx265 -preset medium` fixture would take longer
/// to generate than the whole benchmark takes to run.
const RECIPES: &[Recipe] = &[
    Recipe {
        codec: "h264",
        encoder: "libx264",
        video_args: &["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"],
        audio_args: &["-c:a", "aac", "-b:a", "128k"],
        extension: "mp4",
    },
    Recipe {
        codec: "hevc",
        encoder: "libx265",
        video_args: &[
            "-c:v",
            "libx265",
            "-preset",
            "ultrafast",
            "-crf",
            "24",
            "-x265-params",
            "log-level=error",
            "-tag:v",
            "hvc1",
        ],
        audio_args: &["-c:a", "aac", "-b:a", "128k"],
        extension: "mp4",
    },
    Recipe {
        codec: "vp9",
        encoder: "libvpx-vp9",
        video_args: &[
            "-c:v",
            "libvpx-vp9",
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-b:v",
            "0",
            "-crf",
            "32",
            "-row-mt",
            "1",
        ],
        audio_args: &["-c:a", "libopus", "-b:a", "96k"],
        extension: "webm",
    },
    Recipe {
        codec: "av1",
        encoder: "libsvtav1",
        video_args: &[
            "-c:v",
            "libsvtav1",
            "-preset",
            "10",
            "-crf",
            "38",
            "-svtav1-params",
            "fast-decode=1",
        ],
        audio_args: &["-c:a", "aac", "-b:a", "128k"],
        extension: "mp4",
    },
];

pub const SIZES: [(u32, u32); 2] = [(1920, 1080), (1080, 1920)];

/// Where the cache lives. Relative to the crate root, which is where cargo runs
/// the binary from, so `cargo run` from `src-tauri/` and from the repository
/// root both land in the same place.
pub fn media_dir(override_dir: Option<&Path>) -> PathBuf {
    match override_dir {
        Some(dir) => dir.to_path_buf(),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("bench-media"),
    }
}

pub fn have_ffmpeg() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn ffmpeg_version() -> String {
    let Ok(out) = Command::new("ffmpeg").args(["-hide_banner", "-version"]).output() else {
        return "not found".into();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("unknown")
        .to_string()
}

fn available_encoders() -> String {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-encoders"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// What the suite could not build, and why. One string per missing codec, in
/// prose, so it can be printed as a skip reason.
pub struct Fixtures {
    pub clips: Vec<Fixture>,
    pub missing: Vec<(String, String)>,
}

impl Fixtures {
    pub fn find(&self, codec: &str, size: (u32, u32)) -> Option<&Fixture> {
        self.clips
            .iter()
            .find(|f| f.codec == codec && (f.width, f.height) == size)
    }

    /// The clip everything that is not a decode benchmark uses: H.264 is the
    /// one codec present on every machine, so the composite, preview and export
    /// figures stay comparable across machines even where the others are absent.
    pub fn workhorse(&self, size: (u32, u32)) -> Option<&Fixture> {
        self.find("h264", size).or_else(|| self.clips.first())
    }
}

/// Build everything that is missing and hand back what exists.
///
/// Generation is the slow part of a first run — a couple of minutes — and it
/// happens once per machine. Progress is printed as it goes, because a silent
/// two-minute pause reads as a hang.
pub fn ensure(dir: &Path, seconds: f64, fps: u32) -> std::io::Result<Fixtures> {
    std::fs::create_dir_all(dir)?;
    let mut clips = Vec::new();
    let mut missing = Vec::new();

    if !have_ffmpeg() {
        for recipe in RECIPES {
            missing.push((
                recipe.codec.to_string(),
                "ffmpeg is not on PATH, so no fixture could be generated".to_string(),
            ));
        }
        return Ok(Fixtures { clips, missing });
    }

    let encoders = available_encoders();
    let mut announced = false;

    for recipe in RECIPES {
        if !encoders.contains(recipe.encoder) {
            missing.push((
                recipe.codec.to_string(),
                format!("this ffmpeg has no {} encoder", recipe.encoder),
            ));
            continue;
        }
        for (width, height) in SIZES {
            let path = dir.join(format!(
                "bench_{}_{width}x{height}.{}",
                recipe.codec, recipe.extension
            ));
            if !usable(&path) {
                if !announced {
                    announced = true;
                    eprintln!(
                        "generating benchmark fixtures in {} — a couple of minutes, once per \
                         machine",
                        dir.display()
                    );
                }
                eprintln!("  {} …", path.file_name().unwrap_or_default().to_string_lossy());
                if let Err(error) = generate(recipe, &path, width, height, seconds, fps) {
                    missing.push((
                        format!("{} {width}x{height}", recipe.codec),
                        error.to_string(),
                    ));
                    // A half-written file would be picked up as usable next run.
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            }
            clips.push(Fixture {
                codec: recipe.codec,
                width,
                height,
                path,
            });
        }
    }

    Ok(Fixtures { clips, missing })
}

/// A file counts as cached only if it is large enough to be a real clip. An
/// empty or truncated file from an interrupted run would otherwise be trusted
/// forever, and would fail deep inside a decoder with an unhelpful message.
fn usable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 64 * 1024)
        .unwrap_or(false)
}

fn generate(
    recipe: &Recipe,
    path: &Path,
    width: u32,
    height: u32,
    seconds: f64,
    fps: u32,
) -> std::io::Result<()> {
    // Written to a temporary name and renamed, so an interrupted run never
    // leaves something at the real path that the next run would treat as cached.
    let temporary = path.with_extension(format!(
        "partial.{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or("mp4")
    ));

    let mut command = Command::new("ffmpeg");
    command.args([
        // No stdin: this must never stop to ask whether to overwrite something.
        "-nostdin",
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
    ]);
    command.arg(format!(
        "testsrc2=size={width}x{height}:rate={fps}:duration={seconds},noise=alls=10:allf=t+u"
    ));
    command.args(["-f", "lavfi", "-i"]);
    command.arg(format!(
        "sine=frequency=440:sample_rate=48000:duration={seconds}"
    ));
    command.args(recipe.video_args);
    command.args(recipe.audio_args);
    command.args([
        "-g",
        "15",
        "-pix_fmt",
        "yuv420p",
        "-shortest",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
    ]);
    command.arg(&temporary);

    let output = command.output()?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&temporary);
        return Err(std::io::Error::other(format!(
            "ffmpeg failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    std::fs::rename(&temporary, path)?;
    Ok(())
}
