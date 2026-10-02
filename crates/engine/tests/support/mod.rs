//! Shared machinery for the integration suite.
//!
//! ## Why the fixtures are generated rather than committed
//!
//! A suite that only runs where somebody happened to leave an mp4 is not a
//! suite. Everything these tests decode is built here, by `ffmpeg`, from
//! content this file chose — which is also what makes the assertions strong:
//! the counter clip encodes its own frame number into its pixels, so "seeking
//! to 1.5 s returned frame 45" is a fact the test can check rather than a
//! quality it has to take on trust.
//!
//! Fixtures land in `target/test-media/<VERSION>/` and are reused, so only the
//! first run pays for the encodes. Bump [`FIXTURE_VERSION`] when a generator
//! changes; the old directory is simply ignored.
//!
//! ## Skipping
//!
//! No `ffmpeg` on the machine, or no GPU adapter, means *skip with a reason* —
//! never a red suite. A failure that means "this box has no GPU" trains people
//! to ignore failures.

#![allow(dead_code)]

pub mod edits;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};

use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use chukcut_engine::modules::render::RenderContext;

/// Bump when any generator below changes so stale fixtures are not reused.
const FIXTURE_VERSION: &str = "v2";

// ---------------------------------------------------------------------------
// Skipping
// ---------------------------------------------------------------------------

/// `let media = require_media!();` — skips the test when ffmpeg is missing.
#[macro_export]
macro_rules! require_media {
    () => {
        match $crate::support::media() {
            Ok(media) => media,
            Err(reason) => {
                eprintln!("skipping {}: {reason}", $crate::support::test_name());
                return;
            }
        }
    };
}

/// `let gpu = require_gpu!();` — skips the test when no adapter can be opened.
#[macro_export]
macro_rules! require_gpu {
    () => {
        match $crate::support::gpu() {
            Some(ctx) => ctx,
            None => {
                eprintln!(
                    "skipping {}: no GPU adapter on this machine",
                    $crate::support::test_name()
                );
                return;
            }
        }
    };
}

/// Best-effort name of the running test, for skip messages.
pub fn test_name() -> String {
    std::thread::current().name().unwrap_or("test").to_string()
}

// ---------------------------------------------------------------------------
// The GPU
// ---------------------------------------------------------------------------

/// One device for the whole test binary — the library's own.
///
/// Opening an adapter costs tens of milliseconds and several tests need one;
/// `RenderContext` is `Sync`, so sharing it is both correct and much faster
/// than a device per test. It is the same device the preview server and the
/// exporter use, because `chukcut_engine::modules::gpu` only ever opens one and
/// two live Vulkan instances in one address space have been observed crashing
/// the driver.
pub fn gpu() -> Option<Arc<RenderContext>> {
    chukcut_engine::modules::gpu::render_context()
}

// ---------------------------------------------------------------------------
// Media fixtures
// ---------------------------------------------------------------------------

/// Every generated file, by role.
pub struct Media {
    pub dir: PathBuf,

    /// 320x240, 30 fps, 120 frames, no audio. Every frame carries its own
    /// index in eight black/white stripes — see [`read_counter`].
    pub counter: PathBuf,
    /// The same counter content muxed to MPEG-TS, whose first timestamp is
    /// 1.466 s rather than zero.
    pub counter_late_start: PathBuf,
    /// The counter with an AAC track, for "an audio stream when one was
    /// requested" and for probing.
    pub counter_with_audio: PathBuf,
    /// The counter at 30 fps for the first second and 15 fps afterwards.
    pub counter_vfr: PathBuf,
    /// The same counter content in MPEG-4 Part 2, which this codebase does not
    /// hardware-decode. The fixture exists so the automatic fallback has
    /// something real to fall back *from*: a file whose codec the GPU path
    /// refuses must still open and still decode correctly.
    pub counter_mpeg4: PathBuf,

    /// 320x240: top-left red, top-right green, bottom-left blue,
    /// bottom-right white. One second at 30 fps.
    pub quadrants: PathBuf,
    /// [`Self::quadrants`] tagged to display rotated a quarter turn clockwise.
    pub quadrants_rot90: PathBuf,
    /// …counter-clockwise.
    pub quadrants_rot270: PathBuf,
    /// …upside down.
    pub quadrants_rot180: PathBuf,

    /// 320x240 (4:3) solid red, 2 s.
    pub solid_red_landscape: PathBuf,
    /// 240x320 (3:4) solid green, 2 s.
    pub solid_green_portrait: PathBuf,
    /// 320x180 (16:9) solid white, 2 s.
    pub solid_white_wide: PathBuf,

    /// A 2 s 48 kHz mono sine with no video stream at all.
    pub audio_only: PathBuf,
}

/// Size and shape of the counter clip. The stripe count is what bounds the
/// frame index it can encode.
pub const COUNTER_WIDTH: u32 = 320;
pub const COUNTER_HEIGHT: u32 = 240;
pub const COUNTER_FRAMES: u64 = 120;
pub const COUNTER_FPS: f64 = 30.0;
pub const COUNTER_STRIPES: u32 = 8;

/// Microseconds at which counter frame `n` is presented.
pub fn counter_frame_time(n: u64) -> Micros {
    // Rounded the same way the rest of the codebase computes frame times, so a
    // request lands on the frame it names rather than a microsecond short of it.
    (n as f64 * 1_000_000.0 / COUNTER_FPS).round() as Micros
}

/// Colours of the four quadrant fixtures, as the coded (unrotated) image has
/// them.
pub const QUAD_TL: [u8; 3] = [255, 0, 0];
pub const QUAD_TR: [u8; 3] = [0, 255, 0];
pub const QUAD_BL: [u8; 3] = [0, 0, 255];
pub const QUAD_BR: [u8; 3] = [255, 255, 255];

static MEDIA: OnceLock<Result<Media, String>> = OnceLock::new();

/// The fixture set, generating it on first use.
pub fn media() -> Result<&'static Media, &'static String> {
    MEDIA.get_or_init(build_media).as_ref()
}

fn build_media() -> Result<Media, String> {
    if !has_tool("ffmpeg") {
        return Err("ffmpeg is not on PATH".into());
    }

    let dir = media_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;

    let media = Media {
        counter: dir.join("counter.mp4"),
        counter_late_start: dir.join("counter_late_start.ts"),
        counter_with_audio: dir.join("counter_audio.mp4"),
        counter_vfr: dir.join("counter_vfr.mp4"),
        counter_mpeg4: dir.join("counter_mpeg4.mp4"),
        quadrants: dir.join("quadrants.mp4"),
        quadrants_rot90: dir.join("quadrants_rot90.mp4"),
        quadrants_rot270: dir.join("quadrants_rot270.mp4"),
        quadrants_rot180: dir.join("quadrants_rot180.mp4"),
        solid_red_landscape: dir.join("solid_red_320x240.mp4"),
        solid_green_portrait: dir.join("solid_green_240x320.mp4"),
        solid_white_wide: dir.join("solid_white_320x180.mp4"),
        audio_only: dir.join("audio_only.m4a"),
        dir,
    };

    once(&media.counter, |out| {
        encode_rgb24(
            out,
            COUNTER_WIDTH,
            COUNTER_HEIGHT,
            "30",
            (0..COUNTER_FRAMES).map(|n| counter_frame_rgb(n as u32)),
            // A real GOP and B-frames, so the seek tests exercise the
            // "decode forward from the keyframe" path rather than a file
            // that is all keyframes.
            &["-c:v", "libx264", "-crf", "10", "-g", "30", "-bf", "2"],
        )
    })?;

    once(&media.counter_late_start, |out| {
        // MPEG-TS starts its clock at 1.4 s, which is exactly the "first
        // timestamp is not zero" case a decoder gets wrong by treating the
        // stream's start as the origin.
        ffmpeg(&[
            "-i",
            path(&media.counter),
            "-c",
            "copy",
            "-f",
            "mpegts",
            path(out),
        ])
    })?;

    once(&media.counter_with_audio, |out| {
        ffmpeg(&[
            "-i",
            path(&media.counter),
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4:sample_rate=48000",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-shortest",
            path(out),
        ])
    })?;

    once(&media.counter_vfr, |out| {
        // 30 fps for the first second, 15 fps after it: the timestamps stop
        // being a multiple of a single interval, which is what "variable frame
        // rate" costs a decoder.
        ffmpeg(&[
            "-i",
            path(&media.counter),
            "-vf",
            "setpts='if(lt(N,30),PTS,2*PTS-30/TB/30)'",
            "-fps_mode",
            "vfr",
            "-c:v",
            "libx264",
            "-crf",
            "10",
            "-g",
            "30",
            path(out),
        ])
    })?;

    once(&media.counter_mpeg4, |out| {
        // Deliberately a codec no modern iGPU decodes. `-q:v 2` keeps the
        // counter stripes clean enough to read back, which MPEG-4 at a default
        // quantiser does not.
        ffmpeg(&[
            "-i",
            path(&media.counter),
            "-c:v",
            "mpeg4",
            "-q:v",
            "2",
            "-g",
            "30",
            path(out),
        ])
    })?;

    once(&media.quadrants, |out| {
        encode_rgb24(
            out,
            320,
            240,
            "30",
            (0..30).map(|_| quadrant_frame_rgb(320, 240)),
            &["-c:v", "libx264", "-crf", "5"],
        )
    })?;

    // `-display_rotation D` writes a display matrix for which
    // `av_display_rotation_get` reports D. FFmpeg's own autorotation — and
    // this codebase, which follows it — turns the picture by *minus* that,
    // clockwise. So -90 here is the ordinary phone-held-sideways file.
    for (out, degrees) in [
        (&media.quadrants_rot90, "-90"),
        (&media.quadrants_rot270, "90"),
        (&media.quadrants_rot180, "180"),
    ] {
        once(out, |out| {
            ffmpeg(&[
                "-display_rotation",
                degrees,
                "-i",
                path(&media.quadrants),
                "-c",
                "copy",
                path(out),
            ])
        })?;
    }

    for (out, (w, h), rgb) in [
        (&media.solid_red_landscape, (320u32, 240u32), [255u8, 0, 0]),
        (&media.solid_green_portrait, (240, 320), [0, 255, 0]),
        (&media.solid_white_wide, (320, 180), [255, 255, 255]),
    ] {
        once(out, |out| {
            encode_rgb24(
                out,
                w,
                h,
                "30",
                (0..60).map(|_| solid_frame_rgb(w, h, rgb)),
                &["-c:v", "libx264", "-crf", "5"],
            )
        })?;
    }

    once(&media.audio_only, |out| {
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            path(out),
        ])
    })?;

    Ok(media)
}

/// `target/test-media/<version>`.
fn media_dir() -> PathBuf {
    // CARGO_TARGET_TMPDIR is `<target>/tmp`; its parent is the target dir,
    // wherever the user pointed CARGO_TARGET_DIR.
    let target = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("target"));
    target.join("test-media").join(FIXTURE_VERSION)
}

/// Build `out` if it is not already there.
///
/// The generator writes to a sibling temp path which is then renamed, so a run
/// interrupted halfway cannot leave a truncated fixture that the next run
/// happily decodes.
fn once(out: &Path, generate: impl FnOnce(&Path) -> Result<(), String>) -> Result<(), String> {
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(());
    }
    let extension = out.extension().and_then(|e| e.to_str()).unwrap_or("tmp");
    let staging = out.with_extension(format!("partial.{extension}"));
    let _ = std::fs::remove_file(&staging);
    generate(&staging)?;
    std::fs::rename(&staging, out).map_err(|e| format!("cannot publish {}: {e}", out.display()))?;
    Ok(())
}

fn has_tool(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn path(p: &Path) -> &str {
    p.to_str().expect("fixture paths are utf-8")
}

/// Run ffmpeg with `-y` and quiet logging, failing with its stderr attached.
fn ffmpeg(args: &[&str]) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-nostdin"])
        .args(args)
        .output()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Encode raw RGB24 frames produced in-process.
///
/// Generating the pixels here rather than with a `lavfi` source is the whole
/// point: the test knows exactly what went in, so it can assert on exactly
/// what comes out.
fn encode_rgb24(
    out: &Path,
    width: u32,
    height: u32,
    rate: &str,
    frames: impl Iterator<Item = Vec<u8>>,
    encoder: &[&str],
) -> Result<(), String> {
    let size = format!("{width}x{height}");
    let mut child = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-nostdin"])
        .args([
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &size, "-r", rate, "-i", "-",
        ])
        .args(encoder)
        .args(["-pix_fmt", "yuv420p", path(out)])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;

    {
        let mut stdin = child.stdin.take().expect("stdin was piped");
        for frame in frames {
            stdin
                .write_all(&frame)
                .map_err(|e| format!("cannot feed ffmpeg: {e}"))?;
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("ffmpeg did not finish: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "encoding {} failed: {}",
            out.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Frame `n` of the counter: eight vertical stripes, stripe `i` white when bit
/// `i` of `n` is set.
///
/// 16 and 235 rather than 0 and 255 because those are the limited-range video
/// levels; anything outside them survives the RGB→YUV→RGB round trip less
/// predictably.
fn counter_frame_rgb(n: u32) -> Vec<u8> {
    let stripe = COUNTER_WIDTH / COUNTER_STRIPES;
    let mut row = Vec::with_capacity(COUNTER_WIDTH as usize * 3);
    for x in 0..COUNTER_WIDTH {
        let bit = (x / stripe).min(COUNTER_STRIPES - 1);
        let value = if (n >> bit) & 1 == 1 { 235 } else { 16 };
        row.extend_from_slice(&[value, value, value]);
    }
    row.repeat(COUNTER_HEIGHT as usize)
}

fn quadrant_frame_rgb(width: u32, height: u32) -> Vec<u8> {
    let mut frame = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            let colour = match (x < width / 2, y < height / 2) {
                (true, true) => QUAD_TL,
                (false, true) => QUAD_TR,
                (true, false) => QUAD_BL,
                (false, false) => QUAD_BR,
            };
            frame.extend_from_slice(&colour);
        }
    }
    frame
}

fn solid_frame_rgb(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
    rgb.repeat((width * height) as usize)
}

// ---------------------------------------------------------------------------
// Reading the counter back
// ---------------------------------------------------------------------------

/// Recover the frame index a decoded counter frame carries, or `None` when the
/// stripes are not cleanly black or white.
///
/// `data` is tightly packed RGBA8. `None` is a real answer, not a shrug: it
/// means the picture is not a counter frame at all — a blend of two, a
/// letterboxed composite, or a decode that produced garbage — and a test that
/// unwraps it is asserting that it got one clean frame.
pub fn read_counter_rgba(data: &[u8], width: u32, height: u32) -> Option<u64> {
    let stripe = width / COUNTER_STRIPES;
    if stripe == 0 || height == 0 || data.len() < (width * height * 4) as usize {
        return None;
    }
    let y = height / 2;
    let mut index = 0u64;
    for bit in 0..COUNTER_STRIPES {
        let x = bit * stripe + stripe / 2;
        let offset = ((y * width + x) * 4) as usize;
        let value = data[offset];
        // A wide dead band: a value in the middle means the sample landed on a
        // stripe edge or on a blend, and guessing at it would turn a real
        // failure into a wrong frame number.
        match value {
            0..=90 => {}
            160..=255 => index |= 1 << bit,
            _ => return None,
        }
    }
    Some(index)
}

// ---------------------------------------------------------------------------
// Project construction
// ---------------------------------------------------------------------------

/// A project with one video track holding one clip that covers the canvas.
pub fn single_clip_project(
    canvas: (u32, u32),
    fps: f64,
    material: VideoMaterial,
    duration: Micros,
) -> Project {
    let mut project = Project::new(
        "integration",
        CanvasConfig {
            width: canvas.0,
            height: canvas.1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        fps,
    );
    let material_id = material.id.clone();
    project.materials.videos.push(material);

    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment(&material_id, 0, duration));
    project.tracks.push(track);
    project
}

/// A segment covering `[start, start + duration)` of the timeline, showing the
/// matching slice of its material from the top.
pub fn segment(material_id: &str, start: Micros, duration: Micros) -> Segment {
    Segment {
        id: format!("seg-{material_id}-{start}"),
        material_id: material_id.to_string(),
        target_range: TimeRange::new(start, duration),
        source_range: TimeRange::new(0, duration),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// A `VideoMaterial` describing a fixture, probed rather than assumed.
pub fn material_for(id: &str, file: &Path) -> Result<VideoMaterial, String> {
    let info = chukcut_engine::modules::media::probe(file).map_err(|e| e.to_string())?;
    let video = info
        .video
        .ok_or_else(|| format!("{} has no video stream", file.display()))?;
    Ok(VideoMaterial {
        id: id.to_string(),
        path: file.to_string_lossy().into_owned(),
        width: video.width,
        height: video.height,
        duration: info.duration,
        fps: video.fps,
        has_audio: info.has_audio,
        rotation: video.rotation,
    })
}

// ---------------------------------------------------------------------------
// Pixels
// ---------------------------------------------------------------------------

/// The RGBA texel at `(x, y)` of a tightly packed buffer.
pub fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [data[i], data[i + 1], data[i + 2], data[i + 3]]
}

/// Assert a pixel is within `tolerance` of `want` on every channel.
///
/// Exact equality is the wrong bar for anything that has been through a video
/// codec or a GPU filter; a tolerance with the actual value in the message is
/// what makes a failure diagnosable.
#[track_caller]
pub fn assert_pixel_near(got: [u8; 4], want: [u8; 4], tolerance: i32, what: &str) {
    let close = got
        .iter()
        .zip(want.iter())
        .all(|(g, w)| (*g as i32 - *w as i32).abs() <= tolerance);
    assert!(
        close,
        "{what}: got {got:?}, wanted {want:?} within {tolerance}"
    );
}

// ---------------------------------------------------------------------------
// ffprobe
// ---------------------------------------------------------------------------

/// What [`probe_output`] can tell a test about a file it just wrote.
#[derive(Debug, Clone)]
pub struct Probed {
    pub width: u32,
    pub height: u32,
    /// `avg_frame_rate` as an exact fraction, straight from the container.
    pub avg_frame_rate: (u64, u64),
    /// Container duration in seconds.
    pub duration: f64,
    /// Frames a decoder actually produced — counted, not read off a header,
    /// because a truncated tail is precisely a header that lies.
    pub decoded_frames: u64,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
}

impl Probed {
    pub fn fps(&self) -> f64 {
        if self.avg_frame_rate.1 == 0 {
            0.0
        } else {
            self.avg_frame_rate.0 as f64 / self.avg_frame_rate.1 as f64
        }
    }
}

/// Inspect a file with `ffprobe`, counting decoded frames.
///
/// Deliberately not our own `media::probe`: verifying an export with the same
/// library that wrote it would pass even if both agreed on something wrong.
pub fn probe_output(file: &Path) -> Result<Probed, String> {
    if !has_tool("ffprobe") {
        return Err("ffprobe is not on PATH".into());
    }

    let video = ffprobe(&[
        "-select_streams",
        "v:0",
        "-count_frames",
        "-show_entries",
        "stream=width,height,avg_frame_rate,nb_read_frames,codec_name",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1",
        path(file),
    ])?;
    let audio = ffprobe(&[
        "-select_streams",
        "a:0",
        "-show_entries",
        "stream=codec_name",
        "-of",
        "default=noprint_wrappers=1",
        path(file),
    ])?;

    let get = |text: &str, key: &str| -> Option<String> {
        let prefix = format!("{key}=");
        text.lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .map(|v| v.trim().to_string())
    };

    let rate = get(&video, "avg_frame_rate").unwrap_or_else(|| "0/1".into());
    let (num, den) = rate.split_once('/').unwrap_or((rate.as_str(), "1"));

    Ok(Probed {
        width: get(&video, "width")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        height: get(&video, "height")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        avg_frame_rate: (num.parse().unwrap_or(0), den.parse().unwrap_or(1).max(1)),
        duration: get(&video, "duration")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        decoded_frames: get(&video, "nb_read_frames")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        has_audio: !audio.trim().is_empty(),
        audio_codec: get(&audio, "codec_name"),
        video_codec: get(&video, "codec_name"),
    })
}

fn ffprobe(args: &[&str]) -> Result<String, String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(args)
        .output()
        .map_err(|e| format!("cannot run ffprobe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffprobe {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// A seeded linear congruential generator.
///
/// A dependency-free PRNG so the fuzzers are reproducible from their seed:
/// "iteration 1447 of seed 20250725 breaks it" has to be a sentence anyone can
/// act on, which rules out anything that reseeds itself.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub fn next_u64(&mut self) -> u64 {
        // Knuth's LCG constants, then a mix so the low bits are usable —
        // taking `% n` of a raw LCG samples its worst bits.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^= x >> 29;
        x
    }

    /// A value in `0..n`. `n == 0` yields 0.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }

    /// A value in `lo..=hi`.
    pub fn between(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % (hi - lo + 1) as u64) as i64
    }

    /// A float in `0.0..1.0`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in.max(1)) == 0
    }

    /// Pick an element, or `None` from an empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len()))
        }
    }
}

/// A project serialized to a form two documents can be compared by.
///
/// `serde_json::Value` and not the struct's own `to_string`: `MaterialPool`
/// holds a `HashMap`, whose iteration order is not stable, and a comparison
/// that depends on it would fail at random. `to_value` builds a `BTreeMap`,
/// so the bytes below are canonical.
pub fn canonical(project: &Project) -> String {
    serde_json::to_value(project)
        .expect("a project always serializes")
        .to_string()
}
