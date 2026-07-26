//! The whole export, end to end, on each path the machine can take.
//!
//! Four configurations, and the reason there are four rather than two is the
//! single most useful thing `docs/STATUS.md` records about the export: the
//! encoder stopped being the bottleneck two optimisations ago. Comparing
//! "software" against "hardware" alone reports 1.5×, which is true and
//! misleading — the gain is in what the frame does on its way to the encoder,
//! not in the encoder.
//!
//! | | colour conversion | what crosses the bus |
//! |---|---|---|
//! | tier 1 | swscale, on the CPU | 8 MB RGBA back, 3 MB NV12 up |
//! | tier 2 | compute shader | 3 MB NV12 back, 3 MB up |
//! | tier 3 | compute shader | nothing |
//!
//! Every configuration exports the same timeline, with audio, through
//! `export::job::run_export` — the same call the app's export command makes. A
//! benchmark that drove the encoder directly would miss the mixer, the muxer
//! and the flush, and the flush is where the classic truncated-export bug
//! lives.
//!
//! The tiers are process-global switches (`job::set_gpu_color_convert`,
//! `job::set_zero_copy`) rather than per-job settings, so this group is
//! deliberately not parallel with anything and restores both switches when it
//! is done.
//!
//! ## Take this group with `--filter export`
//!
//! This is the one group in the suite that is measurably affected by what ran
//! before it in the same process, and it was found the hard way. In a full run
//! it goes last, after the decode, composite and preview groups have had the
//! iGPU busy for a minute, and in that position tier 3 measured **34 fps** —
//! *slower* than tier 2, which is impossible on the mechanism, since the only
//! difference between them is whether the NV12 buffer is DMA-BUF-exported. Run
//! on its own on the same quiet machine, tier 3 measures **66–68 fps** and the
//! ordering is monotonic as it should be.
//!
//! The tell was in the breakdown, which is the argument for always printing one:
//! the *decode* stage had gone from 10.4 to 19.4 ms per frame, and decoding is
//! the one thing the tier flag cannot touch. The iGPU shares its power and
//! thermal budget with the CPU cores, so a group that leaves the GPU hot slows
//! down whatever runs next.
//!
//! Nothing here can fix that — somebody has to run last. So: read the *ratios*
//! between the tiers from a full run, and take the absolute figures from
//! `--filter export`.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_lib::modules::audio::FileAudioSource;
use chukcut_lib::modules::export::job::{self, ExportJob, ExportRequest};
use chukcut_lib::modules::export::{hwaccel, ExportProgress};
use chukcut_lib::modules::media::MediaSourceProvider;
use chukcut_lib::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_lib::modules::render::{Compositor, CompositorConfig, RenderContext};

use crate::fixtures::Fixtures;
use crate::harness::{rounds, Measurement};

pub const GROUP: &str = "export";

/// Progress goes nowhere. The suite has to run unattended, and an export
/// printing a percentage every frame would bury the table.
struct Silent;

impl job::ProgressSink for Silent {
    fn send(&self, _progress: ExportProgress) {}
}

pub struct Budget {
    /// Seconds of timeline exported. Longer is steadier; `docs/STATUS.md`
    /// advises at least eight for a number worth quoting, which does not fit a
    /// five-minute default run.
    pub seconds: f64,
    pub rounds: usize,
}

/// Two clips cut out of the same source, because a single clip never exercises
/// the segment boundary and the boundary is where the compositor swaps sources.
fn timeline(fixture: &crate::fixtures::Fixture, seconds: f64) -> Project {
    let mut project = Project::new(
        "export bench",
        CanvasConfig {
            width: fixture.width & !1,
            height: fixture.height & !1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    let half: Micros = (seconds * 500_000.0) as Micros;
    let material_id = new_id();
    project.materials.videos.push(VideoMaterial {
        id: material_id.clone(),
        path: fixture.path.to_string_lossy().into_owned(),
        width: fixture.width,
        height: fixture.height,
        duration: 4_000_000,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
    });

    let mut track = Track::new(TrackKind::Video, "Video 1");
    for (index, source_start) in [0i64, 2_000_000].iter().enumerate() {
        track.segments.push(Segment {
            id: new_id(),
            material_id: material_id.clone(),
            target_range: TimeRange::new(index as Micros * half, half),
            source_range: TimeRange::new(*source_start, half),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
    }
    project.tracks.push(track);
    project
}

pub fn run(ctx: &Arc<RenderContext>, media: &Fixtures, budget: &Budget) -> Vec<Measurement> {
    let mut out = Vec::new();
    let size = (1920u32, 1080);
    let Some(fixture) = media.workhorse(size) else {
        out.push(Measurement::skip(
            GROUP,
            "1920x1080 software",
            "no fixture; ffmpeg could not generate one",
        ));
        return out;
    };
    let project = timeline(fixture, budget.seconds);
    let output = fixture
        .path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("bench_export_out.mp4");

    // One compositor, shared with every other GPU group. Opening a second
    // device would not merely be wasteful: `docs/STATUS.md` records concurrent
    // Vulkan instances crashing this driver.
    let compositor = Arc::new(Compositor::with_config(
        Arc::clone(ctx),
        CompositorConfig {
            // An export that silently drops a clip is worse than one that
            // fails, and a benchmark that silently drops a clip reports a
            // wonderful number for rendering nothing.
            strict_sources: true,
            ..Default::default()
        },
    ));

    let hardware = hwaccel::detect();
    let usable_h264 = hardware
        .iter()
        .find(|e| e.usable && e.encoder_name.contains("h264"))
        .map(|e| e.id.clone());

    let mut configurations: Vec<(&str, Option<String>, bool, bool)> = vec![
        // (label, hardware encoder id, gpu nv12, zero copy)
        ("software (libx264)", None, true, true),
    ];
    match &usable_h264 {
        Some(id) => {
            configurations.push(("VAAPI tier 1: CPU swscale + upload", Some(id.clone()), false, false));
            configurations.push(("VAAPI tier 2: GPU NV12, read back", Some(id.clone()), true, false));
            configurations.push(("VAAPI tier 3: zero-copy", Some(id.clone()), true, true));
        }
        None => {
            let reason = if hardware.is_empty() {
                "this build of FFmpeg has no VAAPI H.264 encoder".to_string()
            } else {
                // An encoder being present in the build says nothing about
                // whether the driver can drive it, which is why `hwaccel`
                // answers by opening the device and encoding a frame.
                hardware
                    .iter()
                    .find(|e| e.encoder_name.contains("h264"))
                    .and_then(|e| e.note.clone())
                    .unwrap_or_else(|| "the driver refused every hardware encoder".to_string())
            };
            for tier in [
                "VAAPI tier 1: CPU swscale + upload",
                "VAAPI tier 2: GPU NV12, read back",
                "VAAPI tier 3: zero-copy",
            ] {
                out.push(Measurement::skip(GROUP, tier, reason.clone()));
            }
        }
    }

    for (label, hardware_id, gpu_nv12, zero_copy) in configurations {
        job::set_gpu_color_convert(gpu_nv12);
        job::set_zero_copy(zero_copy);

        let request = ExportRequest {
            output_path: output.to_string_lossy().into_owned(),
            preset_id: None,
            overrides: None,
            hardware: hardware_id.clone(),
            include_audio: true,
        };
        let settings = match job::resolve_settings(&project, &request) {
            Ok(settings) => settings,
            Err(error) => {
                out.push(Measurement::skip(GROUP, label, error.to_string()));
                continue;
            }
        };
        let total_frames = settings.total_frames;

        compositor.reset_stats();
        let mut breakdown = String::new();
        let samples = rounds::<String>(budget.rounds, |_| {
            let export = ExportJob {
                job_id: "bench".into(),
                project: project.clone(),
                settings: settings.clone(),
                compositor: Arc::clone(&compositor),
                sources: Arc::new(MediaSourceProvider::from_project(&project)),
                audio: Arc::new(FileAudioSource),
                cancel: Arc::new(AtomicBool::new(false)),
            };
            compositor.reset_stats();
            let started = std::time::Instant::now();
            let outcome = job::run_export(&export, &Silent).map_err(|e| e.to_string())?;
            let elapsed = started.elapsed().as_secs_f64();
            if outcome.frames != total_frames {
                // A short export is the classic un-flushed-encoder bug, and it
                // is also very fast. Refusing to report it is the point.
                return Err(format!(
                    "wrote {} of {total_frames} frames",
                    outcome.frames
                ));
            }
            let stats = compositor.stats();
            breakdown = format!(
                "sources {:.1} + composite {:.1} + readback {:.1} + nv12 {:.1} ms, \
                 encoder prepare {:.1} + submit {:.1} ms",
                stats.per_frame(stats.sources_ns) / 1e6,
                stats.per_frame(stats.composite_ns) / 1e6,
                stats.per_frame(stats.readback_ns) / 1e6,
                stats.per_frame(stats.nv12_ns) / 1e6,
                outcome.writer.per_frame(outcome.writer.prepare_ns) / 1e6,
                outcome.writer.per_frame(outcome.writer.submit_ns) / 1e6,
            );
            Ok(outcome.frames as f64 / elapsed.max(f64::EPSILON))
        });

        match samples {
            Ok(samples) => out.push(Measurement::fps(GROUP, label, samples).with_note(breakdown)),
            Err(error) => out.push(Measurement::skip(GROUP, label, error)),
        }
    }

    // Both switches default to on in the app; leaving them off would silently
    // change what a later group measures.
    job::set_gpu_color_convert(true);
    job::set_zero_copy(true);
    let _ = std::fs::remove_file(&output);

    out
}
