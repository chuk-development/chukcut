//! End-to-end export test, without the UI.
//!
//! Builds a small two-clip timeline from real media, cuts a piece out of each,
//! and exports it — through the same compositor, encoder and audio mixer the
//! app uses. Then it says what it produced so `ffprobe` can be pointed at it.
//!
//! This exists because "the export module has tests" and "the export produces a
//! playable file" are different claims, and only the second one matters to
//! someone trying to cut a video.
//!
//! ```text
//! cargo run --release --example export_smoke -- out.mp4 clip1.mp4 clip2.mkv
//! ```

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_lib::modules::audio::FileAudioSource;
use chukcut_lib::modules::export::job::{self, ExportJob, ExportRequest};
use chukcut_lib::modules::export::ExportProgress;
use chukcut_lib::modules::media::{probe, MediaSourceProvider};
use chukcut_lib::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_lib::modules::render::{Compositor, CompositorConfig, RenderContext};

/// Prints progress as it arrives, so a hang is visible rather than silent.
struct Printer;

impl job::ProgressSink for Printer {
    fn send(&self, progress: ExportProgress) {
        println!(
            "  [{:>5.1}%] {:?} frame {}/{} — {:.1} fps{}",
            progress.fraction * 100.0,
            progress.stage,
            progress.frame,
            progress.total_frames,
            progress.fps,
            progress
                .message
                .as_ref()
                .map(|m| format!(" — {m}"))
                .unwrap_or_default()
        );
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("chukcut=info,warn")
        .init();

    let mut args = std::env::args().skip(1);
    let output = args.next().unwrap_or_else(|| {
        eprintln!("usage: export_smoke <output.mp4> <video>...");
        std::process::exit(2);
    });
    let inputs: Vec<String> = args.collect();
    if inputs.is_empty() {
        eprintln!("usage: export_smoke <output.mp4> <video>...");
        std::process::exit(2);
    }

    // Two seconds of each clip, taken from one second in — so the result is a
    // real cut rather than a copy, and a wrong source range shows up as the
    // wrong picture rather than as nothing.
    const CLIP: Micros = 2_000_000;
    const SOURCE_IN: Micros = 1_000_000;

    let mut project = Project::new("export smoke", CanvasConfig::default(), 30.0);
    let mut video_track = Track::new(TrackKind::Video, "Video 1");
    let mut playhead: Micros = 0;

    for path in &inputs {
        let info = probe(path)?;
        let video = info
            .video
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{path} has no video stream"))?;

        // The first clip decides the canvas, exactly as the app does on import.
        if playhead == 0 {
            project.canvas.width = video.display_width & !1;
            project.canvas.height = video.display_height & !1;
            if video.fps > 0.0 {
                project.fps = video.fps;
            }
            println!(
                "canvas {}x{} @ {:.3} fps (from {path})",
                project.canvas.width, project.canvas.height, project.fps
            );
        }

        let material_id = new_id();
        project.materials.videos.push(VideoMaterial {
            id: material_id.clone(),
            path: path.clone(),
            width: video.width,
            height: video.height,
            duration: info.duration,
            fps: video.fps,
            has_audio: info.has_audio,
            rotation: video.rotation,
        });

        video_track.segments.push(Segment {
            id: new_id(),
            material_id,
            target_range: TimeRange::new(playhead, CLIP),
            source_range: TimeRange::new(SOURCE_IN.min(info.duration / 2), CLIP),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        playhead += CLIP;
    }

    project.tracks.push(video_track);

    for issue in project.validate() {
        println!("validate: {:?} {}", issue.severity, issue.message);
    }
    println!(
        "timeline: {} clips, {:.2}s total",
        project.tracks[0].segments.len(),
        project.duration() as f64 / 1_000_000.0
    );

    let context = Arc::new(
        RenderContext::try_new().ok_or_else(|| anyhow::anyhow!("no usable GPU adapter"))?,
    );
    println!(
        "GPU: {} ({:?})",
        context.adapter_info().name,
        context.adapter_info().backend
    );

    let compositor = Arc::new(Compositor::with_config(
        Arc::clone(&context),
        CompositorConfig {
            // An export that silently drops a clip is worse than one that
            // fails: the hole is only found after the upload.
            strict_sources: true,
            ..Default::default()
        },
    ));

    let request = ExportRequest {
        output_path: output.clone(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: true,
    };
    let settings = job::resolve_settings(&project, &request)?;
    println!(
        "encoding {} frames to {} ({}x{})",
        settings.total_frames,
        settings.output_path.display(),
        settings.video.width,
        settings.video.height
    );

    let sources = Arc::new(MediaSourceProvider::from_project(&project));
    let export = ExportJob {
        job_id: "smoke".into(),
        project,
        settings,
        compositor,
        sources,
        audio: Arc::new(FileAudioSource),
        cancel: Arc::new(AtomicBool::new(false)),
    };

    let started = std::time::Instant::now();
    let outcome = job::run_export(&export, &Printer)?;
    println!(
        "\ndone: {} frames in {:.2}s -> {}",
        outcome.frames,
        started.elapsed().as_secs_f64(),
        outcome.output_path.display()
    );
    println!("verify with: ffprobe -hide_banner {}", output);
    Ok(())
}
