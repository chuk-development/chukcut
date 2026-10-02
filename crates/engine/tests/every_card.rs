//! The basic edit, on every GPU path this machine has.
//!
//! Import a clip, cut it, play it, scrub it, export it — and check every frame
//! that comes out is the frame that should. Run once per decode path (software,
//! VAAPI on Intel and AMD, NVDEC on NVIDIA) and once per encoder (software and
//! every hardware encoder that passes its trial encode). A path this machine
//! does not have is skipped with a printed line, so the same suite is the
//! acceptance test on a laptop with an Intel iGPU and on a desktop with an
//! NVIDIA card.
//!
//! The counter fixture writes its own frame index into its pixels, which is
//! what turns "the export looks fine" into an assertion: a cut that lands one
//! frame off, a seek that returns the frame before, a hardware path that drops
//! the last GOP — each reads back as the wrong number.

mod support;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportProgress, ExportRequest, FnSink,
};
use chukcut_engine::modules::media::decoder::Acceleration;
use chukcut_engine::modules::media::hwdecode::{self, HwBackend};
use chukcut_engine::modules::media::{HwCodec, MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, TimeRange, Track, TrackKind,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};

use support::{material_for, probe_output, read_counter_rgba, segment, COUNTER_FPS};

const SECOND: Micros = 1_000_000;

/// Every decode path for H.264 here, software first.
fn decode_paths() -> Vec<Acceleration> {
    let mut paths = vec![Acceleration::Software];
    if hwdecode::supports_on(HwCodec::H264, HwBackend::Vaapi) {
        paths.push(Acceleration::Vaapi);
    }
    if hwdecode::supports_on(HwCodec::H264, HwBackend::Cuda) {
        paths.push(Acceleration::Cuda);
    }
    if paths.len() == 1 {
        eprintln!(
            "{}: no hardware H.264 decode here; only software is exercised",
            support::test_name()
        );
    }
    paths
}

/// The counter clip, cut three ways on one track:
///
/// | timeline | shows source frames |
/// |---|---|
/// | 0 – 1 s | 0 – 29 |
/// | 1 – 2 s | 60 – 89 (a jump cut forward) |
/// | 2 – 2.5 s | 10 – 24 (a jump back) |
fn cut_project() -> Option<Project> {
    let media = support::media().ok()?;
    let mut project = Project::new(
        "every card",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        COUNTER_FPS,
    );
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).ok()?);

    let mut track = Track::new(TrackKind::Video, "V1");
    for (start, source, duration) in [
        (0, 0, SECOND),
        (SECOND, 2 * SECOND, SECOND),
        (2 * SECOND, SECOND / 3, SECOND / 2),
    ] {
        let mut piece = segment("counter", start, duration);
        piece.source_range = TimeRange::new(source, duration);
        track.segments.push(piece);
    }
    project.tracks.push(track);
    Some(project)
}

/// Timeline frame `k` of [`cut_project`] → the source frame it must show.
fn expected_source_frame(k: u64) -> u64 {
    match k {
        0..=29 => k,
        30..=59 => 60 + (k - 30),
        _ => 10 + (k - 60),
    }
}

const CUT_FRAMES: u64 = 75;

/// A moment safely inside timeline frame `k`, away from both its edges.
fn inside_frame(k: u64) -> Micros {
    (k as f64 * 1_000_000.0 / COUNTER_FPS).round() as Micros + 16_000
}

fn render_counter(
    compositor: &Compositor,
    project: &Project,
    sources: &MediaSourceProvider,
    k: u64,
) -> Option<u64> {
    let size = (project.canvas.width, project.canvas.height);
    let rgba = compositor
        .render_frame(project, inside_frame(k), size, sources)
        .expect("render a frame");
    read_counter_rgba(&rgba, size.0, size.1)
}

#[test]
fn every_decode_path_plays_a_cut_timeline_frame_for_frame() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let project = cut_project().expect("the cut project");

    for path in decode_paths() {
        let compositor = Compositor::new(Arc::clone(&ctx));
        let sources = MediaSourceProvider::from_project_with(&project, Some(path));
        // Playback: every frame in order, across both cuts.
        for k in 0..CUT_FRAMES {
            assert_eq!(
                render_counter(&compositor, &project, &sources, k),
                Some(expected_source_frame(k)),
                "{path:?}: timeline frame {k} during playback"
            );
        }
    }
}

#[test]
fn every_decode_path_lands_on_the_right_frame_when_scrubbing() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let project = cut_project().expect("the cut project");

    // Backwards, across cuts, onto cut edges, and repeated — the order a hand
    // on the playhead produces, and the one that exercises every seek.
    let order = [70, 5, 45, 31, 29, 30, 59, 60, 0, 74, 74, 12, 61, 44];
    for path in decode_paths() {
        let compositor = Compositor::new(Arc::clone(&ctx));
        let sources = MediaSourceProvider::from_project_with(&project, Some(path));
        for k in order {
            assert_eq!(
                render_counter(&compositor, &project, &sources, k),
                Some(expected_source_frame(k)),
                "{path:?}: scrubbing to timeline frame {k}"
            );
        }
    }
}

#[test]
fn every_decode_path_shows_the_same_picture_as_software() {
    let media = require_media!();
    let ctx = require_gpu!();
    let project = support::single_clip_project(
        (320, 240),
        30.0,
        material_for("quadrants", &media.quadrants).expect("quadrants material"),
        SECOND,
    );
    let size = (320, 240);
    let compositor = Compositor::new(Arc::clone(&ctx));
    let software = MediaSourceProvider::from_project_with(&project, Some(Acceleration::Software));
    let reference = compositor
        .render_frame(&project, SECOND / 2, size, &software)
        .expect("software render");

    for path in decode_paths().into_iter().skip(1) {
        let sources = MediaSourceProvider::from_project_with(&project, Some(path));
        let got = compositor
            .render_frame(&project, SECOND / 2, size, &sources)
            .expect("hardware render");
        let mean = reference
            .iter()
            .zip(&got)
            .map(|(a, b)| a.abs_diff(*b) as u64)
            .sum::<u64>() as f64
            / reference.len() as f64;
        // Chroma subsampling at the quadrant edges and an sRGB round trip cost
        // about one step on average; a wrong colour matrix, a swapped chroma
        // order or a range mismatch costs tens.
        assert!(
            mean < 2.0,
            "{path:?} composites {mean:.2} away from software on average"
        );
    }
}

/// Export `project` with `hardware` (an encoder id, `None` for software).
fn export(project: &Project, hardware: Option<String>, name: &str) -> std::path::PathBuf {
    let ctx = support::gpu().expect("a GPU");
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("every_card");
    std::fs::create_dir_all(&dir).expect("scratch directory");
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);

    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware,
        include_audio: false,
        range: None,
    };
    let settings = resolve_settings(project, &request).expect("resolve the export settings");
    let compositor = Arc::new(Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    ));
    // The production provider: whatever decode path this machine picks.
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(project));
    let job = ExportJob {
        job_id: name.into(),
        project: project.clone(),
        settings,
        compositor,
        sources,
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let sink = FnSink(|_: ExportProgress| {});
    let outcome = run_export(&job, &sink).expect("the export finishes");
    assert!(!outcome.cancelled);
    path
}

#[test]
fn a_cut_timeline_exports_frame_for_frame_on_every_encoder() {
    let _ = require_media!();
    let _ = require_gpu!();
    let project = cut_project().expect("the cut project");

    let mut encoders: Vec<(String, Option<String>)> = vec![("software".into(), None)];
    for encoder in chukcut_engine::modules::export::hwaccel::detect() {
        if encoder.usable {
            encoders.push((encoder.id.clone(), Some(encoder.id.clone())));
        }
    }
    if encoders.len() == 1 {
        eprintln!(
            "{}: no usable hardware encoder here; only software is exercised",
            support::test_name()
        );
    }

    for (label, hardware) in encoders {
        let path = export(&project, hardware, &format!("cut_{label}.mp4"));
        let probed = probe_output(&path).expect("probe the export");
        assert_eq!(
            probed.decoded_frames, CUT_FRAMES,
            "{label}: the export holds {} frames, the timeline {CUT_FRAMES}",
            probed.decoded_frames
        );

        let mut decoder = VideoDecoder::open(&path).expect("open the export");
        for k in [0, 1, 29, 30, 31, 59, 60, 61, 74] {
            let frame = decoder
                .seek_and_decode(inside_frame(k))
                .expect("decode the export");
            assert_eq!(
                read_counter_rgba(&frame.data, frame.width, frame.height),
                Some(expected_source_frame(k)),
                "{label}: exported frame {k}"
            );
        }
        let _ = std::fs::remove_file(&path);
    }
}
