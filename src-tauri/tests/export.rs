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

use chukcut_lib::modules::export::presets::{Container, Fps};
use chukcut_lib::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportOverrides, ExportProgress, ExportRequest,
    ExportStage, FnSink,
};
use chukcut_lib::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_lib::modules::project::document::{
    CanvasConfig, Micros, Project, Track, TrackKind,
};
use chukcut_lib::modules::render::{Compositor, CompositorConfig, SourceProvider};

use support::{material_for, probe_output, read_counter_rgba, segment, Probed};

/// A one-second timeline showing the start of the counter clip.
fn counter_project(duration: Micros, fps: f64) -> Option<Project> {
    let media = support::media().ok()?;
    let mut project = Project::new(
        "export integration",
        CanvasConfig {
            width: 320,
            height: 240,
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

fn request(path: &std::path::Path, overrides: Option<ExportOverrides>, audio: bool) -> ExportRequest {
    ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides,
        hardware: None,
        include_audio: audio,
    }
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
    let sink = FnSink(move |message: ExportProgress| {
        recorder.lock().expect("lock").push(message.stage)
    });

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
    let reported = failed.lock().expect("lock").clone().expect("a failure message");
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
