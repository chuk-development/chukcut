//! Exporting, verified with a tool that did not write the file.
//!
//! Every check here goes through `ffprobe` rather than through this codebase's
//! own probe. Verifying an export with the same library that produced it will
//! agree with itself even when both are wrong about the time base; an
//! independent decoder will not.
//!
//! The two failures this is really guarding against:
//!
//! - **A truncated tail.** An encoder holds frames — lookahead, B-frame
//!   reordering, rate control — so the last second of video is still inside
//!   libavcodec when the timeline ends. A writer that is not flushed produces a
//!   file that plays perfectly and stops early, which nobody notices until the
//!   upload is live. So the frame count is *counted by decoding*, not read off
//!   the header, and it has to be exact.
//! - **Fractional frame rates.** 23.976 and 29.97 are 24000/1001 and
//!   30000/1001, and encoding them with a 1/24 or 1/30 time base drifts a frame
//!   every thousand — half a second over an hour. A one-second export cannot
//!   show the drift, but it can show that the container came out tagged with
//!   the right fraction.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chukcut_engine::modules::export::presets::{Container, Fps};
use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportOverrides, ExportProgress, ExportRequest,
    ExportStage, FnSink,
};
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{CanvasConfig, Micros, Project, Track, TrackKind};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};

use support::{material_for, probe_output, read_counter_rgba, segment, Probed};

/// A one-second timeline showing the start of the counter clip.
fn counter_project(duration: Micros, fps: f64) -> Option<Project> {
    counter_project_sized(320, 240, duration, fps)
}

/// The counter clip on a canvas of the caller's choosing.
///
/// The size matters more than it looks: 320 is a multiple of 64, and for months
/// that made every export test blind to a bug that destroyed real exports —
/// the VAAPI encoder misreads any plane whose pitch its driver disagrees with,
/// and only unaligned widths expose it. Tests that care about the hardware
/// path must run at a width that is *not* a multiple of 64.
fn counter_project_sized(width: u32, height: u32, duration: Micros, fps: f64) -> Option<Project> {
    let media = support::media().ok()?;
    let mut project = Project::new(
        "export integration",
        CanvasConfig {
            width,
            height,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        fps,
    );
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment("counter", 0, duration));
    project.tracks.push(track);
    Some(project)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("export");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir.join(name)
}

fn request(
    path: &std::path::Path,
    overrides: Option<ExportOverrides>,
    audio: bool,
) -> ExportRequest {
    ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides,
        hardware: None,
        include_audio: audio,
        range: None,
    }
}

/// The first hardware encoder this machine can actually drive.
///
/// `hwaccel::detect` answers by opening the device and encoding a frame, so a
/// `usable` entry here means the driver really works — which is exactly the
/// condition under which the tests below are worth running. On CI, and on any
/// machine without a GPU, this is `None` and they skip.
fn usable_hardware() -> Option<chukcut_engine::modules::export::HwEncoder> {
    chukcut_engine::modules::export::hwaccel::detect()
        .into_iter()
        .find(|encoder| encoder.usable)
}

/// Every usable hardware encoder, because "the hardware path works" is a claim
/// per encoder, not per machine. The stripes bug was reported against H.264
/// *and* H.265, and only H.264 had ever been pixel-checked.
fn all_usable_hardware() -> Vec<chukcut_engine::modules::export::HwEncoder> {
    chukcut_engine::modules::export::hwaccel::detect()
        .into_iter()
        .filter(|encoder| encoder.usable)
        .collect()
}

/// Mean luma PSNR between two same-sized RGBA frames, in dB.
///
/// The blunt instrument for the class of bug where the picture is *recognisable
/// but wrong* — displaced stripes, a shifted plane, a swapped chroma order.
/// Codec loss between two encodes of the same frame sits above 40 dB; the
/// stride bug measured 17–19 dB. A threshold of 30 cannot confuse the two.
fn luma_psnr_rgba(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "frames must be the same size to compare");
    let mut sum = 0.0f64;
    let mut count = 0.0f64;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        // BT.601 luma from RGB, the same weights the counter fixture uses.
        let ya = 0.299 * pa[0] as f64 + 0.587 * pa[1] as f64 + 0.114 * pa[2] as f64;
        let yb = 0.299 * pb[0] as f64 + 0.587 * pb[1] as f64 + 0.114 * pb[2] as f64;
        sum += (ya - yb) * (ya - yb);
        count += 1.0;
    }
    let mse = sum / count.max(1.0);
    if mse <= f64::EPSILON {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

/// Everything an export produced: the file, what ffprobe says about it, and
/// every progress message the job sent.
struct Exported {
    path: std::path::PathBuf,
    probe: Probed,
    progress: Vec<ExportProgress>,
    expected_frames: u64,
}

/// Run a whole export, or `None` when this machine cannot (no ffmpeg, no GPU).
fn export(project: &Project, request: ExportRequest) -> Option<Exported> {
    let ctx = support::gpu()?;
    let settings = resolve_settings(project, &request).expect("resolve the settings");
    let expected_frames = settings.total_frames;
    let path = settings.output_path.clone();
    let _ = std::fs::remove_file(&path);

    // The same configuration the export command builds: strict about sources,
    // because an export that quietly leaves a clip out is worse than one that
    // fails — the hole is only discovered after the upload.
    let compositor = Arc::new(Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    ));
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(project));

    let progress: Arc<Mutex<Vec<ExportProgress>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let progress = Arc::clone(&progress);
        FnSink(move |message: ExportProgress| progress.lock().expect("sink lock").push(message))
    };

    let job = ExportJob {
        job_id: "integration".into(),
        project: project.clone(),
        settings,
        compositor,
        sources,
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let outcome = run_export(&job, &sink).expect("the export should finish");
    assert!(!outcome.cancelled);
    assert_eq!(outcome.frames, expected_frames);

    let messages = progress.lock().expect("sink lock").clone();
    Some(Exported {
        probe: probe_output(&path).expect("probe the export"),
        path,
        progress: messages,
        expected_frames,
    })
}

macro_rules! exported {
    ($project:expr, $request:expr) => {{
        let _ = require_media!();
        let _ = require_gpu!();
        match export($project, $request) {
            Some(result) => result,
            None => return,
        }
    }};
}

// ---------------------------------------------------------------------------
// Frame rates
// ---------------------------------------------------------------------------

/// One frame of `fps`, in seconds.
fn frame_seconds(fps: Fps) -> f64 {
    fps.den as f64 / fps.num as f64
}

fn check_rate(name: &str, fps: f64, want: Fps) {
    let Some(project) = counter_project(1_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch(&format!("rate_{name}.mp4"));
    let overrides = ExportOverrides {
        fps: Some(fps),
        container: Some(Container::Mp4),
        ..Default::default()
    };
    let result = exported!(&project, request(&path, Some(overrides), false));

    // The container has to carry the exact fraction, not a decimal that is
    // close to it. 30000/1001 reduces to itself; 30/1 does not.
    assert_eq!(
        result.probe.avg_frame_rate,
        (want.num as u64, want.den as u64),
        "{name}: the file is tagged {:?}, not {}/{}",
        result.probe.avg_frame_rate,
        want.num,
        want.den
    );

    // Every frame the walk produced has to come back out of the file. This is
    // the flush: an encoder that is not told the stream ended keeps its last
    // GOP, and the file plays and stops early.
    assert_eq!(
        result.probe.decoded_frames, result.expected_frames,
        "{name}: {} frames went in and {} came out — the encoder was not flushed",
        result.expected_frames, result.probe.decoded_frames
    );

    // The timeline is one second. The output is allowed to be one frame longer,
    // because the last frame of the timeline is still visible for its whole
    // duration, and no more than that.
    let tolerance = frame_seconds(want) * 1.5;
    assert!(
        (result.probe.duration - 1.0).abs() <= tolerance,
        "{name}: a one-second timeline exported as {:.4} s, outside a {tolerance:.4} s tolerance",
        result.probe.duration
    );

    assert_eq!((result.probe.width, result.probe.height), (320, 240));
    assert_eq!(result.probe.video_codec.as_deref(), Some("h264"));
    let _ = std::fs::remove_file(&result.path);
}

#[test]
fn exporting_at_thirty_frames_a_second_produces_exactly_thirty_frames() {
    check_rate("30", 30.0, Fps::THIRTY);
}

#[test]
fn exporting_at_twenty_nine_ninety_seven_keeps_its_thousand_and_first() {
    check_rate("2997", 29.97, Fps::NTSC);
}

#[test]
fn exporting_at_twenty_three_nine_seven_six_keeps_its_thousand_and_first() {
    check_rate("23976", 23.976, Fps::FILM_NTSC);
}

// ---------------------------------------------------------------------------
// The tail
// ---------------------------------------------------------------------------

#[test]
fn the_last_frame_of_the_timeline_is_in_the_file() {
    let Some(project) = counter_project(2_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("tail.mp4");
    let result = exported!(&project, request(&path, None, false));

    assert_eq!(result.expected_frames, 60);
    assert_eq!(
        result.probe.decoded_frames, 60,
        "a truncated tail: the last GOP never left the encoder"
    );

    // Not merely sixty frames, but the *right* sixty. The counter clip writes
    // its own index into its pixels, so this reads the exported file back and
    // checks that frame 59 of the export really is frame 59 of the source —
    // which no amount of counting could tell you.
    let mut decoder = VideoDecoder::open(&result.path).expect("open the export");
    for n in [0u64, 1, 29, 30, 59] {
        let at = (n as f64 * 1_000_000.0 / 30.0).round() as i64 + 16_000;
        let frame = decoder.seek_and_decode(at).expect("decode the export");
        assert_eq!(
            read_counter_rgba(&frame.data, frame.width, frame.height),
            Some(n),
            "frame {n} of the export is not frame {n} of the source"
        );
    }
    let _ = std::fs::remove_file(&result.path);
}

// ---------------------------------------------------------------------------
// The tail, on the GPU
// ---------------------------------------------------------------------------

/// The hardware path has its own encoder, its own pixel format and its own
/// frame pool, and none of that is allowed to change what comes out.
///
/// This is the same check as `the_last_frame_of_the_timeline_is_in_the_file`,
/// deliberately: a truncated tail is *the* classic hardware-encode bug, because
/// people write a second flush for the hardware path and get it subtly wrong.
/// The frames are counted by decoding, and then read for their content, so a
/// green picture or a one-frame offset fails here rather than after an upload.
#[test]
fn a_hardware_export_is_not_truncated_and_holds_the_right_frames() {
    let Some(hardware) = usable_hardware() else {
        eprintln!("skipping: no usable hardware encoder on this machine");
        return;
    };
    let Some(project) = counter_project(2_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };

    let path = scratch("tail_hardware.mp4");
    let mut req = request(&path, None, false);
    req.hardware = Some(hardware.id.clone());
    let result = exported!(&project, req);

    assert_eq!(result.expected_frames, 60);
    assert_eq!(
        result.probe.decoded_frames, 60,
        "{}: a truncated tail on the hardware path — the last GOP never left the encoder",
        hardware.id
    );

    // The counter clip writes its own frame index into its pixels, so this is
    // the check that the *right* sixty frames came out. A hardware path that
    // uploaded into the wrong surface, or that lost a frame's PTS on the way
    // through `av_hwframe_transfer_data`, fails here and only here.
    let mut decoder = VideoDecoder::open(&result.path).expect("open the hardware export");
    for n in [0u64, 1, 29, 30, 59] {
        let at = (n as f64 * 1_000_000.0 / 30.0).round() as i64 + 16_000;
        let frame = decoder
            .seek_and_decode(at)
            .expect("decode the hardware export");
        assert_eq!(
            read_counter_rgba(&frame.data, frame.width, frame.height),
            Some(n),
            "{}: frame {n} of the hardware export is not frame {n} of the source",
            hardware.id
        );
    }
    let _ = std::fs::remove_file(&result.path);
}

/// Every hardware encoder must produce the same *picture* as the software
/// path, at the canvas widths whose plane pitch is not a multiple of 64.
///
/// This is the regression test for the stripes bug, written the way the owner
/// described the check: export the same cut through both paths and compare the
/// frames — first, middle and last, because a bug that corrupts geometry
/// corrupts every frame, and one that only corrupts late frames (a ring reuse
/// fault) is invisible at frame zero.
///
/// Why PSNR and not equality: two encoders are lossy differently, so identical
/// pixels are impossible. Codec loss between the two paths measures 46–49 dB
/// on real footage; the stripes bug measured 17–19 dB. The 30 dB line cannot
/// confuse them, and a failure prints the number so the next person sees which
/// side of it they are on.
///
/// Why these sizes: 1440 (mod 64 = 32) is what a 4:3 clip adopts and is the
/// exact canvas the bug shipped on; 1080×1920 (mod 64 = 56) is the app's
/// default vertical canvas. 320-wide tests stayed green through the whole
/// affair because 320 divides by 64 — that blindness is documented on
/// `counter_project_sized` and must not be reintroduced.
#[test]
fn every_hardware_encoder_shows_the_same_picture_as_the_software_path() {
    let encoders = all_usable_hardware();
    if encoders.is_empty() {
        eprintln!("skipping: no usable hardware encoder on this machine");
        return;
    }

    for (width, height) in [(1440u32, 1080u32), (1080, 1920)] {
        let Some(project) = counter_project_sized(width, height, 1_000_000, 30.0) else {
            eprintln!("skipping: no media fixtures");
            return;
        };

        let sw_path = scratch(&format!("picture_sw_{width}x{height}.mp4"));
        let software = exported!(&project, request(&sw_path, None, false));

        // Decode the software reference frames once per size, outside the
        // encoder loop.
        let sample_times: Vec<i64> = [0u64, 14, 29]
            .iter()
            .map(|n| (*n as f64 * 1_000_000.0 / 30.0).round() as i64 + 16_000)
            .collect();
        let mut sw_decoder = VideoDecoder::open(&software.path).expect("open the software export");
        let sw_frames: Vec<_> = sample_times
            .iter()
            .map(|at| {
                sw_decoder
                    .seek_and_decode(*at)
                    .expect("decode the software export")
            })
            .collect();

        for hardware in &encoders {
            let hw_path = scratch(&format!("picture_{}_{width}x{height}.mp4", hardware.id));
            let mut req = request(&hw_path, None, false);
            req.hardware = Some(hardware.id.clone());
            let result = exported!(&project, req);

            let mut decoder = VideoDecoder::open(&result.path).expect("open the hardware export");
            for (at, reference) in sample_times.iter().zip(&sw_frames) {
                let frame = decoder
                    .seek_and_decode(*at)
                    .expect("decode the hardware export");
                assert_eq!(
                    (frame.width, frame.height),
                    (reference.width, reference.height),
                    "{} at {width}x{height}: the two exports disagree about the frame size",
                    hardware.id
                );
                let psnr = luma_psnr_rgba(&frame.data, &reference.data);
                assert!(
                    psnr > 30.0,
                    "{} at {width}x{height}, t={at}: luma PSNR against the software export \
                     is {psnr:.1} dB — the picture is structurally wrong, not merely lossy. \
                     Displaced vertical strips at this size were the plane-pitch bug; see \
                     ROW_ALIGN in render::nv12",
                    hardware.id
                );
            }
            let _ = std::fs::remove_file(&result.path);
        }
        let _ = std::fs::remove_file(&software.path);
    }
}

/// A hardware export has to be a normal file: same size, same rate, same codec
/// family, an audio track when one was asked for.
#[test]
fn a_hardware_export_is_shaped_like_a_software_one() {
    let Some(hardware) = usable_hardware() else {
        eprintln!("skipping: no usable hardware encoder on this machine");
        return;
    };
    let Some(project) = counter_project(1_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };

    let path = scratch("shape_hardware.mp4");
    let mut req = request(&path, None, true);
    req.hardware = Some(hardware.id.clone());
    let result = exported!(&project, req);

    assert_eq!((result.probe.width, result.probe.height), (320, 240));
    assert_eq!(result.probe.decoded_frames, 30);
    assert_eq!(result.probe.avg_frame_rate, (30, 1));
    assert!(
        result.probe.has_audio,
        "no audio stream in the hardware file"
    );
    let _ = std::fs::remove_file(&result.path);
}

// ---------------------------------------------------------------------------
// The export range
// ---------------------------------------------------------------------------

/// A range export walks exactly its own frames, and the frames are the right
/// ones. The counter clip writes its index into its pixels, so "frame 0 of a
/// range starting at one second is source frame 30" is a fact to check rather
/// than an inference from the file's length.
#[test]
fn a_range_export_holds_exactly_the_ranged_frames_rebased_to_zero() {
    // Four seconds of counter (frames 0..120); export the middle two.
    let Some(project) = counter_project(4_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("range.mp4");
    let mut req = request(&path, None, false);
    req.range = Some((1_000_000, 3_000_000));
    let result = exported!(&project, req);

    assert_eq!(result.expected_frames, 60, "two seconds at 30 fps");
    assert_eq!(
        result.probe.decoded_frames, 60,
        "the range's frame count, counted by decoding"
    );
    // The file starts at zero: a two-second range is a two-second file, give
    // or take the last frame's own duration.
    assert!(
        (result.probe.duration - 2.0).abs() <= 1.5 / 30.0,
        "a two-second range exported as {:.4} s",
        result.probe.duration
    );

    // First, middle and last frame of the output are source frames 30, 59 and
    // 89 — the range rebased, not the project truncated.
    let mut decoder = VideoDecoder::open(&result.path).expect("open the range export");
    for (out_frame, source_frame) in [(0u64, 30u64), (29, 59), (59, 89)] {
        let at = (out_frame as f64 * 1_000_000.0 / 30.0).round() as i64 + 16_000;
        let frame = decoder
            .seek_and_decode(at)
            .expect("decode the range export");
        assert_eq!(
            read_counter_rgba(&frame.data, frame.width, frame.height),
            Some(source_frame),
            "output frame {out_frame} should be source frame {source_frame}"
        );
    }
    let _ = std::fs::remove_file(&result.path);
}

/// The duration of a stream, asked with ffprobe rather than with anything that
/// wrote the file. `None` when the stream is not there.
fn stream_duration_seconds(file: &std::path::Path, stream: &str) -> Option<f64> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            stream,
            "-show_entries",
            "stream=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(file)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[test]
fn a_range_export_audio_is_as_long_as_the_range() {
    let Some(project) = counter_project(4_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("range_audio.mp4");
    let mut req = request(&path, None, true);
    req.range = Some((1_000_000, 3_000_000));
    let result = exported!(&project, req);

    assert!(
        result.probe.has_audio,
        "no audio stream in the range export"
    );
    let audio =
        stream_duration_seconds(&result.path, "a:0").expect("the audio stream has a duration");
    // AAC pads to its 1024-sample frame and the muxer may carry priming
    // samples, so the tolerance is a couple of codec frames, not zero.
    assert!(
        (audio - 2.0).abs() <= 0.1,
        "a two-second range carries {audio:.3} s of audio"
    );
    let _ = std::fs::remove_file(&result.path);
}

/// Marks that outlive an edit clamp to the timeline rather than failing.
#[test]
fn a_range_past_the_end_of_the_timeline_clamps_to_what_exists() {
    let Some(project) = counter_project(2_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("range_clamped.mp4");
    let mut req = request(&path, None, false);
    // The out mark sits a second past the last clip.
    req.range = Some((1_000_000, 3_000_000));
    let result = exported!(&project, req);

    assert_eq!(result.expected_frames, 30, "only the second that exists");
    assert_eq!(result.probe.decoded_frames, 30);

    let mut decoder = VideoDecoder::open(&result.path).expect("open the clamped export");
    let frame = decoder.seek_and_decode(16_000).expect("decode frame 0");
    assert_eq!(
        read_counter_rgba(&frame.data, frame.width, frame.height),
        Some(30),
        "the clamped range still starts at its in mark"
    );
    let _ = std::fs::remove_file(&result.path);
}

// ---------------------------------------------------------------------------
// Frame snapshots
// ---------------------------------------------------------------------------

/// A snapshot is a decodable PNG, at canvas size, of the requested frame — and
/// it comes through the real compositor, which the counter readback proves.
#[test]
fn a_snapshot_is_a_decodable_png_of_the_canvas_at_the_requested_frame() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(project) = counter_project(4_000_000, 30.0) else {
        return;
    };

    let compositor = Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );
    let sources = MediaSourceProvider::from_project(&project);

    // Frame 45, asked for at mid-frame so rounding cannot land next door.
    let time = (45.0f64 * 1_000_000.0 / 30.0).round() as i64 + 16_000;
    let asked = scratch("snapshot.jpg"); // deliberately the wrong extension
    let written = chukcut_engine::modules::export::snapshot::write_png(
        &project,
        time,
        &compositor,
        &sources,
        &asked,
    )
    .expect("write the snapshot");

    assert_eq!(
        written.extension().and_then(|e| e.to_str()),
        Some("png"),
        "the extension follows the bytes, as an export's follows its container"
    );

    let decoded = image::open(&written).expect("the PNG decodes").to_rgba8();
    assert_eq!(decoded.dimensions(), (320, 240), "full canvas resolution");
    assert_eq!(
        read_counter_rgba(decoded.as_raw(), 320, 240),
        Some(45),
        "the snapshot is not the frame that was asked for"
    );

    // A time past the end clamps to the last frame rather than failing or
    // rendering black.
    let past = scratch("snapshot_past.png");
    let written = chukcut_engine::modules::export::snapshot::write_png(
        &project,
        99_000_000,
        &compositor,
        &sources,
        &past,
    )
    .expect("write the clamped snapshot");
    let decoded = image::open(&written).expect("the PNG decodes").to_rgba8();
    assert_eq!(read_counter_rgba(decoded.as_raw(), 320, 240), Some(119));

    let _ = std::fs::remove_file(scratch("snapshot.png"));
    let _ = std::fs::remove_file(past);
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

#[test]
fn an_export_that_asked_for_audio_has_an_audio_stream() {
    let Some(project) = counter_project(1_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("with_audio.mp4");
    let result = exported!(&project, request(&path, None, true));

    // There is no PCM reader in `media` yet, so the bed is silence — but the
    // stream has to be there, correctly muxed, or a platform that requires one
    // rejects the upload.
    assert!(result.probe.has_audio, "no audio stream in the file");
    assert_eq!(result.probe.audio_codec.as_deref(), Some("aac"));
    assert_eq!(result.probe.decoded_frames, 30);
    let _ = std::fs::remove_file(&result.path);
}

#[test]
fn an_export_that_did_not_ask_for_audio_has_no_audio_stream() {
    let Some(project) = counter_project(1_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("without_audio.mp4");
    let result = exported!(&project, request(&path, None, false));

    assert!(!result.probe.has_audio, "an audio stream nobody asked for");
    assert_eq!(result.probe.decoded_frames, 30);
    let _ = std::fs::remove_file(&result.path);
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

#[test]
fn progress_starts_at_preparing_ends_at_done_and_never_goes_backwards() {
    let Some(project) = counter_project(1_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let path = scratch("progress.mp4");
    let result = exported!(&project, request(&path, None, false));

    let stages: Vec<ExportStage> = result.progress.iter().map(|p| p.stage).collect();
    assert_eq!(
        stages.first(),
        Some(&ExportStage::Preparing),
        "the first message proves the export started"
    );
    assert_eq!(
        stages.last(),
        Some(&ExportStage::Done),
        "the last message is what closes the dialog"
    );
    assert_eq!(
        stages.iter().filter(|s| s.is_terminal()).count(),
        1,
        "exactly one terminal message: {stages:?}"
    );

    // A progress bar that goes backwards is read as a bug even when it is not.
    let frames: Vec<u64> = result.progress.iter().map(|p| p.frame).collect();
    assert!(
        frames.windows(2).all(|w| w[1] >= w[0]),
        "progress went backwards: {frames:?}"
    );

    let done = result.progress.last().expect("a terminal message");
    assert_eq!(done.fraction, 1.0);
    assert_eq!(done.total_frames, 30);
    assert_eq!(
        done.output_path.as_deref(),
        Some(result.path.to_string_lossy().as_ref()),
        "the terminal message carries the path, so the UI can offer to show it"
    );
    assert!(done.message.is_none());
    let _ = std::fs::remove_file(&result.path);
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

#[test]
fn a_cancelled_export_leaves_no_file_behind() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(project) = counter_project(4_000_000, 30.0) else {
        return;
    };

    let path = scratch("cancelled.mp4");
    let _ = std::fs::remove_file(&path);
    let settings = resolve_settings(&project, &request(&path, None, false)).expect("settings");
    let output = settings.output_path.clone();

    let cancel = Arc::new(AtomicBool::new(false));
    let job = ExportJob {
        job_id: "cancelled".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::clone(&cancel),
    };

    // Stop it from inside the progress sink rather than from a timer: the flag
    // is then set at a known point in the run, and the test does not depend on
    // how fast the encoder is.
    let flag = Arc::clone(&cancel);
    let sink = FnSink(move |message: ExportProgress| {
        if message.stage == ExportStage::Encoding {
            flag.store(true, Ordering::Relaxed);
        }
    });

    let outcome = run_export(&job, &sink).expect("a cancellation is an outcome, not an error");
    assert!(outcome.cancelled);

    // A half-written file has no trailer and will not play. Leaving it puts
    // something that looks like a finished video in the user's folder.
    assert!(
        !output.exists(),
        "the aborted export left {} behind",
        output.display()
    );
}

#[test]
fn a_job_cancelled_before_the_first_frame_writes_nothing_and_says_so() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(project) = counter_project(1_000_000, 30.0) else {
        return;
    };

    let path = scratch("cancelled_early.mp4");
    let _ = std::fs::remove_file(&path);
    let settings = resolve_settings(&project, &request(&path, None, false)).expect("settings");
    let output = settings.output_path.clone();

    let job = ExportJob {
        job_id: "cancelled-early".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::new(ctx)),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(true)),
    };

    let stages: Arc<Mutex<Vec<ExportStage>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&stages);
    let sink =
        FnSink(move |message: ExportProgress| recorder.lock().expect("lock").push(message.stage));

    let outcome = run_export(&job, &sink).expect("cancellation is not a failure");
    assert!(outcome.cancelled);
    assert_eq!(outcome.frames, 0);
    assert!(!output.exists());
    assert_eq!(
        stages.lock().expect("lock").last(),
        Some(&ExportStage::Cancelled),
        "the dialog has to be told which kind of ending this was"
    );
}

// ---------------------------------------------------------------------------
// Settings that cannot produce a file
// ---------------------------------------------------------------------------

#[test]
fn an_export_to_a_directory_that_cannot_be_written_fails_with_prose() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(project) = counter_project(1_000_000, 30.0) else {
        return;
    };

    // A path under a file rather than a directory: `create_dir_all` cannot
    // make it and the muxer cannot open it.
    let blocked = scratch("not_a_directory.mp4");
    std::fs::write(&blocked, b"x").expect("write the blocking file");
    let path = blocked.join("inside.mp4");

    let settings = resolve_settings(&project, &request(&path, None, false)).expect("settings");
    let job = ExportJob {
        job_id: "unwritable".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::new(ctx)),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let recorder = Arc::clone(&failed);
    let sink = FnSink(move |message: ExportProgress| {
        if message.stage == ExportStage::Failed {
            *recorder.lock().expect("lock") = message.message.clone();
        }
    });

    let error = run_export(&job, &sink).expect_err("this cannot be written");
    assert!(!error.is_cancellation());
    // The string reaches the user unchanged, so it has to read as a sentence
    // and name the file.
    let reported = failed
        .lock()
        .expect("lock")
        .clone()
        .expect("a failure message");
    assert!(
        reported.contains("inside.mp4"),
        "the failure message does not say which file: {reported}"
    );
    let _ = std::fs::remove_file(&blocked);
}

// ---------------------------------------------------------------------------
// Longer, and therefore ignored by default
// ---------------------------------------------------------------------------

/// Run with `cargo test --test export -- --ignored`.
#[test]
#[ignore = "encodes four seconds at 1080x1920; run with --ignored"]
fn a_vertical_export_at_delivery_resolution_is_the_length_it_should_be() {
    let Some(mut project) = counter_project(4_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    project.canvas.width = 1080;
    project.canvas.height = 1920;

    let path = scratch("vertical.mp4");
    let result = exported!(&project, request(&path, None, true));

    assert_eq!((result.probe.width, result.probe.height), (1080, 1920));
    assert_eq!(result.probe.decoded_frames, 120);
    assert!((result.probe.duration - 4.0).abs() < 0.05);
    assert!(result.probe.has_audio);
    let _ = std::fs::remove_file(&result.path);
}

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

/// An effect clip is not missing media, and what it does reaches the file:
/// the counter clip under a "mirror left onto right" effect clip exports with
/// every frame, and each frame decodes mirror-symmetric.
#[test]
fn an_effect_clip_exports_and_its_effect_is_in_the_file() {
    use chukcut_engine::modules::project::document::{Segment, TimeRange, Transform};
    use chukcut_engine::modules::project::EffectMaterial;

    let Some(mut project) = counter_project(1_000_000, 30.0) else {
        eprintln!("skipping: no media fixtures");
        return;
    };
    let mirror = EffectMaterial::new("mirror");
    let mut lane = Track::new(TrackKind::Effect, "Effects 1");
    lane.segments.push(Segment {
        id: "fx".into(),
        material_id: mirror.id.clone(),
        target_range: TimeRange::new(0, 1_000_000),
        source_range: TimeRange::new(0, 1_000_000),
        render_index: 1,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    });
    project.materials.effects.push(mirror);
    project.tracks.push(lane);
    assert!(job::missing_media(&project).is_empty());

    let path = scratch("effect-clip.mp4");
    let result = exported!(&project, request(&path, None, false));
    assert_eq!(result.probe.decoded_frames, 30);

    let mut decoder = VideoDecoder::open(&result.path).expect("open the export");
    let frame = decoder.seek_and_decode(500_000).expect("decode the export");
    let (w, h) = (frame.width as usize, frame.height as usize);
    let px = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        [
            frame.data[i] as i32,
            frame.data[i + 1] as i32,
            frame.data[i + 2] as i32,
        ]
    };
    let mut worst = 0;
    for y in (4..h - 4).step_by(17) {
        for x in (4..w / 2 - 4).step_by(13) {
            let (a, b) = (px(x, y), px(w - 1 - x, y));
            for c in 0..3 {
                worst = worst.max((a[c] - b[c]).abs());
            }
        }
    }
    assert!(
        worst <= 24,
        "the export is not mirrored: worst difference {worst}"
    );
    let _ = std::fs::remove_file(&result.path);
}
