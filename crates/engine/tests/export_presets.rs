//! Export presets, new formats, the size estimate and the queue, against real
//! files checked with `ffprobe`.
//!
//! The estimate is the reason this file exists. The export dialog once said
//! 24 MB for a 3.4 MB file, because a CRF encode spends what the picture
//! needs and a table cannot know the picture. So the estimate is tested
//! here against exports of a timeline whose thirds are very different to
//! encode — a smooth test card, a detailed fractal zoom, a field of noise —
//! and must land within ±25 % of the file that is actually written.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::export::commands as export_commands;
use chukcut_engine::modules::export::estimate::{self, EstimateMethod};
use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportOverrides, ExportRequest, Quality,
    QueueStatus, VideoCodec,
};
use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::project::document::{Micros, Project, Track, TrackKind};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};

use support::{material_for, segment};

const SECONDS: i64 = 12;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("export-presets");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir.join(name)
}

/// Twelve seconds at 640×360 in three different-to-encode thirds, with a
/// pink-noise-and-tone soundtrack that AAC cannot shrink below its bitrate.
fn varied_clip() -> Option<PathBuf> {
    // Once per test binary: the tests run in parallel and must not write the
    // same fixture at the same time.
    static CLIP: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    CLIP.get_or_init(build_varied_clip).clone()
}

fn build_varied_clip() -> Option<PathBuf> {
    let out = scratch("varied_v1.mp4");
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Some(out);
    }
    let filter = "[0:v][1:v][2:v]concat=n=3:v=1:a=0,format=yuv420p[v];\
                  [3:a][4:a]amix=inputs=2:duration=shortest[a]";
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-nostdin"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30:duration=4",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "mandelbrot=size=640x360:rate=30,trim=duration=4",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "cellauto=size=640x360:rate=30:rule=110:random_fill_ratio=0.5,trim=duration=4",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "anoisesrc=color=pink:amplitude=0.25:sample_rate=48000:duration=12",
        ])
        .args([
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=330:sample_rate=48000:duration=12",
        ])
        .args(["-filter_complex", filter, "-map", "[v]", "-map", "[a]"])
        .args([
            "-c:v", "libx264", "-crf", "12", "-g", "30", "-c:a", "aac", "-b:a", "256k",
        ])
        .arg(out.with_extension("partial.mp4"))
        .status()
        .ok()?;
    if !status.success() {
        eprintln!("skipping: ffmpeg could not build the varied fixture");
        return None;
    }
    std::fs::rename(out.with_extension("partial.mp4"), &out).ok()?;
    Some(out)
}

fn project(width: u32, height: u32, fps: f64) -> Option<Project> {
    let clip = varied_clip()?;
    let mut project = Project::new(
        "presets",
        chukcut_engine::modules::project::document::CanvasConfig {
            width,
            height,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        fps,
    );
    project
        .materials
        .videos
        .push(material_for("varied", &clip).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    track
        .segments
        .push(segment("varied", 0, SECONDS * 1_000_000 as Micros));
    project.tracks.push(track);
    Some(project)
}

fn request(path: &Path, preset: &str, overrides: Option<ExportOverrides>) -> ExportRequest {
    ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: Some(preset.into()),
        overrides,
        hardware: None,
        include_audio: true,
        range: None,
    }
}

/// The real decoder for the exporter's mixer, as the app and the CLI
/// register it; without it every export would mix silence.
fn real_audio() {
    job::register_audio_source(Arc::new(chukcut_engine::modules::audio::FileAudioSource));
}

fn job_for(project: &Project, request: &ExportRequest) -> Option<ExportJob> {
    let ctx = support::gpu()?;
    real_audio();
    let settings = resolve_settings(project, request).expect("resolve the settings");
    let compositor = Arc::new(Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    ));
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(project));
    Some(ExportJob {
        job_id: "presets".into(),
        project: project.clone(),
        settings,
        compositor,
        sources,
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    })
}

/// Export for real; the size of the file written.
fn export(job: &ExportJob) -> u64 {
    let _ = std::fs::remove_file(&job.settings.output_path);
    let outcome = run_export(job, &()).expect("the export should finish");
    assert!(!outcome.cancelled);
    std::fs::metadata(&outcome.output_path)
        .expect("the export wrote a file")
        .len()
}

/// `ffprobe` stream fields of the first stream of `kind` ("v" or "a").
fn stream(file: &Path, kind: &str, fields: &str) -> Option<String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", &format!("{kind}:0")])
        .args(["-show_entries", &format!("stream={fields}")])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(file)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn duration_of(file: &Path) -> f64 {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration"])
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(file)
        .output()
        .expect("run ffprobe");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0)
}

fn assert_within(what: &str, estimate: u64, actual: u64, tolerance: f64) {
    let error = (estimate as f64 - actual as f64) / actual as f64;
    eprintln!(
        "{what}: estimated {estimate} B, wrote {actual} B ({:+.1} %)",
        error * 100.0
    );
    assert!(
        error.abs() <= tolerance,
        "{what}: estimated {estimate} bytes for a {actual}-byte file ({:+.1} %)",
        error * 100.0
    );
}

macro_rules! setup {
    ($w:expr, $h:expr) => {{
        let _ = require_gpu!();
        match project($w, $h, 30.0) {
            Some(project) => project,
            None => return,
        }
    }};
}

// ---------------------------------------------------------------------------
// The estimate
// ---------------------------------------------------------------------------

#[test]
fn a_sampled_estimate_of_a_crf_export_is_within_a_quarter_of_the_file() {
    let project = setup!(640, 360);
    for (codec, crf, name) in [
        (VideoCodec::H264, 20, "est_h264.mp4"),
        (VideoCodec::H264, 28, "est_h264_28.mp4"),
        (VideoCodec::H265, 24, "est_hevc.mp4"),
    ] {
        let overrides = ExportOverrides {
            video_codec: Some(codec),
            quality: Some(Quality::Crf(crf)),
            ..Default::default()
        };
        let Some(job) = job_for(
            &project,
            &request(&scratch(name), "custom", Some(overrides)),
        ) else {
            return;
        };
        let estimate = estimate::sampled(&job).expect("sample the export");
        assert_eq!(estimate.method, EstimateMethod::Sampled);
        // Three two-second windows of a twelve-second timeline.
        assert!((estimate.sampled_seconds - 6.0).abs() < 0.01);
        let actual = export(&job);
        assert_within(name, estimate.bytes, actual, 0.25);
    }
}

#[test]
fn bitrate_and_sound_only_estimates_are_within_a_quarter_without_sampling() {
    let project = setup!(640, 360);
    let cases = [
        (
            "est_bitrate.mp4",
            "custom",
            Some(ExportOverrides {
                quality: Some(Quality::Bitrate(3_000_000)),
                ..Default::default()
            }),
        ),
        ("est.m4a", "audio_aac", None),
        ("est.mp3", "audio_mp3", None),
        ("est.wav", "audio_wav", None),
    ];
    for (name, preset, overrides) in cases {
        let Some(job) = job_for(&project, &request(&scratch(name), preset, overrides)) else {
            return;
        };
        let estimate = estimate::quick(&job.settings);
        assert_ne!(estimate.method, EstimateMethod::Table, "{name}");
        // Sampling has nothing to add to arithmetic.
        assert_eq!(estimate::sampled(&job).unwrap(), estimate, "{name}");
        let actual = export(&job);
        assert_within(name, estimate.bytes, actual, 0.25);
    }
}

#[test]
fn prores_and_gif_estimates_are_within_a_quarter() {
    let project = setup!(640, 360);
    for (name, preset) in [("est_prores.mov", "master_prores"), ("est.gif", "gif")] {
        let Some(job) = job_for(&project, &request(&scratch(name), preset, None)) else {
            return;
        };
        let estimate = estimate::sampled(&job).expect("sample the export");
        let actual = export(&job);
        assert_within(name, estimate.bytes, actual, 0.25);
    }
}

// ---------------------------------------------------------------------------
// The formats
// ---------------------------------------------------------------------------

#[test]
fn a_prores_master_is_hq_ten_bit_with_pcm_sound_at_the_canvas_size() {
    let project = setup!(640, 360);
    let path = scratch("master.mov");
    let Some(job) = job_for(&project, &request(&path, "master_prores", None)) else {
        return;
    };
    export(&job);
    let video = stream(&path, "v", "codec_name,profile,pix_fmt,width,height").unwrap();
    assert!(video.contains("codec_name=prores"), "{video}");
    assert!(video.contains("profile=HQ"), "{video}");
    assert!(video.contains("pix_fmt=yuv422p10le"), "{video}");
    assert!(
        video.contains("width=640") && video.contains("height=360"),
        "{video}"
    );
    let audio = stream(&path, "a", "codec_name").unwrap();
    assert!(audio.contains("pcm_s16le"), "{audio}");
    assert!((duration_of(&path) - SECONDS as f64).abs() < 0.1);
}

#[test]
fn a_gif_is_small_silent_and_fifteen_frames_a_second() {
    let project = setup!(640, 360);
    let path = scratch("clip.gif");
    let Some(job) = job_for(&project, &request(&path, "gif", None)) else {
        return;
    };
    export(&job);
    let probed = support::probe_output(&path).unwrap();
    assert_eq!(probed.video_codec.as_deref(), Some("gif"));
    assert_eq!((probed.width, probed.height), (854, 480));
    assert!(!probed.has_audio);
    assert_eq!(probed.decoded_frames, 15 * SECONDS as u64);
}

#[test]
fn sound_only_files_have_no_picture_and_the_whole_duration() {
    let project = setup!(640, 360);
    for (name, preset, codec) in [
        ("sound.m4a", "audio_aac", "aac"),
        ("sound.mp3", "audio_mp3", "mp3"),
        ("sound.wav", "audio_wav", "pcm_s16le"),
    ] {
        let path = scratch(name);
        let Some(job) = job_for(&project, &request(&path, preset, None)) else {
            return;
        };
        export(&job);
        assert!(
            stream(&path, "v", "codec_name").is_none(),
            "{name} has a picture"
        );
        let audio = stream(&path, "a", "codec_name,sample_rate,channels").unwrap();
        assert!(
            audio.contains(&format!("codec_name={codec}")),
            "{name}: {audio}"
        );
        assert!(audio.contains("channels=2"), "{name}: {audio}");
        let duration = duration_of(&path);
        assert!(
            (duration - SECONDS as f64).abs() < 0.15,
            "{name} lasts {duration} s"
        );
    }
}

#[test]
fn a_platform_preset_fits_the_canvas_and_brings_the_mix_to_its_loudness() {
    // A vertical canvas: TikTok's own shape. Short side 1080 would be slow on
    // a software renderer, so the size is overridden and the loudness — the
    // part under test — comes from the preset.
    let project = setup!(360, 640);
    let path = scratch("tiktok.mp4");
    let overrides = ExportOverrides {
        width: Some(360),
        height: Some(640),
        ..Default::default()
    };
    let Some(job) = job_for(&project, &request(&path, "tiktok", Some(overrides))) else {
        return;
    };
    assert_eq!(job.settings.loudness_target, Some(-14.0));
    assert!(
        job.settings.warnings.is_empty(),
        "{:?}",
        job.settings.warnings
    );
    export(&job);
    let output = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-i"])
        .arg(&path)
        .args(["-map", "0:a", "-af", "ebur128", "-f", "null", "-"])
        .output()
        .expect("run ffmpeg");
    let text = String::from_utf8_lossy(&output.stderr);
    let integrated: f64 = text
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("I:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .expect("ebur128 printed an integrated loudness");
    assert!(
        (integrated + 14.0).abs() <= 1.0,
        "the TikTok preset exported at {integrated} LUFS"
    );
}

// ---------------------------------------------------------------------------
// The queue
// ---------------------------------------------------------------------------

#[test]
fn the_queue_runs_different_presets_and_ranges_in_the_background() {
    let project = setup!(640, 360);
    real_audio();
    let gif = scratch("queued.gif");
    let wav = scratch("queued.wav");
    let range = scratch("queued_range.mp4");
    for path in [&gif, &wav, &range] {
        let _ = std::fs::remove_file(path);
    }
    let mut ranged = request(&range, "custom", None);
    ranged.range = Some((2_000_000, 5_000_000));
    let ids = [
        export_commands::export_queue_add_project(
            project.clone(),
            request(&gif, "gif", None),
            None,
        )
        .unwrap(),
        export_commands::export_queue_add_project(
            project.clone(),
            request(&wav, "audio_wav", None),
            Some("Sound".into()),
        )
        .unwrap(),
        export_commands::export_queue_add_project(project.clone(), ranged, None).unwrap(),
    ];
    // A request that cannot work is refused when it is added, not later.
    let mut broken = request(&scratch("broken.mp4"), "nope", None);
    broken.preset_id = Some("no_such_preset".into());
    assert!(export_commands::export_queue_add_project(project, broken, None).is_err());

    export_commands::export_queue_wait_idle();
    let items = export_commands::export_queue_list();
    for id in &ids {
        let item = items
            .iter()
            .find(|i| &i.id == id)
            .expect("the item is listed");
        assert_eq!(item.status, QueueStatus::Done, "{item:?}");
        assert!(item.bytes.unwrap_or(0) > 0);
    }
    assert_eq!(
        items.iter().find(|i| i.id == ids[1]).unwrap().label,
        "Sound"
    );
    let ranged = support::probe_output(&range).unwrap();
    assert!((ranged.duration - 3.0).abs() < 0.1, "{}", ranged.duration);
    assert!(stream(&wav, "v", "codec_name").is_none());
    assert_eq!(
        support::probe_output(&gif).unwrap().video_codec.as_deref(),
        Some("gif")
    );
}
