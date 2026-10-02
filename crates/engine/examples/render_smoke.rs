//! End-to-end smoke test for the engine, without the UI.
//!
//! Builds a project from real media files, lays them out on a timeline, renders
//! frames through the GPU compositor, and writes them as PNGs. If this works,
//! the decode → provider → compositor path is sound and any remaining problem
//! is in the webview or the IPC layer — which is worth knowing before opening a
//! window and guessing.
//!
//! ```text
//! cargo run --example render_smoke -- clip1.mp4 clip2.mkv
//! ```
//!
//! Writes to `target/smoke/`.

use std::path::PathBuf;
use std::sync::Arc;

use chukcut_engine::modules::media::{probe, MediaSourceProvider};
use chukcut_engine::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_engine::modules::render::Compositor;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("chukcut=debug,warn")
        .init();

    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: cargo run --example render_smoke -- <video>...");
        std::process::exit(2);
    }

    // A vertical canvas, because that is what this editor is for.
    let mut project = Project::new("smoke", CanvasConfig::default(), 30.0);
    let mut track = Track::new(TrackKind::Video, "Video 1");

    // Each clip contributes two seconds, laid end to end.
    const CLIP: Micros = 2_000_000;
    let mut playhead: Micros = 0;

    for path in &paths {
        let info = probe(path)?;
        let video = info
            .video
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{path} has no video stream"))?;

        println!(
            "{path}\n  {}x{} display {}x{} @ {:.3} fps, rotation {}°, {:.2}s, {}",
            video.width,
            video.height,
            video.display_width,
            video.display_height,
            video.fps,
            video.rotation,
            info.duration as f64 / 1_000_000.0,
            info.format
        );

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

        // Start a second in, so a black leader frame cannot be mistaken for a
        // working render.
        track.segments.push(Segment {
            id: new_id(),
            material_id,
            target_range: TimeRange::new(playhead, CLIP),
            source_range: TimeRange::new(1_000_000.min(info.duration / 2), CLIP),
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

    project.tracks.push(track);

    let issues = project.validate();
    for issue in &issues {
        println!("validate: {:?} {}", issue.severity, issue.message);
    }

    let context = chukcut_engine::modules::gpu::render_context()
        .ok_or_else(|| anyhow::anyhow!("no usable GPU adapter"))?;
    println!(
        "GPU: {} ({:?}, {:?})",
        context.adapter_info().name,
        context.adapter_info().backend,
        context.adapter_info().device_type
    );

    let compositor = Compositor::new(Arc::clone(&context));
    let sources = MediaSourceProvider::from_project(&project);

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("smoke");
    std::fs::create_dir_all(&out)?;

    // Sample across the whole timeline, including one frame inside each clip
    // and one right on a cut.
    let size = (project.canvas.width, project.canvas.height);
    let duration = project.duration();
    let samples = 6;

    let started = std::time::Instant::now();
    for i in 0..samples {
        let time = duration * i as Micros / samples as Micros;
        let frame_started = std::time::Instant::now();
        let rgba = compositor.render_frame(&project, time, size, &sources)?;
        let elapsed = frame_started.elapsed();

        let path = out.join(format!("frame_{i:02}.png"));
        image::RgbaImage::from_raw(size.0, size.1, rgba)
            .ok_or_else(|| anyhow::anyhow!("frame buffer has the wrong length"))?
            .save(&path)?;

        println!(
            "t={:>8.3}s  {:>6.1} ms  {}",
            time as f64 / 1_000_000.0,
            elapsed.as_secs_f64() * 1000.0,
            path.display()
        );
    }

    println!(
        "\n{samples} scattered frames at {}x{} in {:.2}s",
        size.0,
        size.1,
        started.elapsed().as_secs_f64()
    );

    // The number above is a *scrub* number: each sample jumps far enough to
    // force a seek and a long decode-forward from the preceding keyframe. What
    // decides whether playback is watchable is the sequential cost, where the
    // decoder already holds the previous frame. Measure both, and at the proxy
    // resolution the preview actually uses rather than at full canvas size.
    let proxy = (size.0 * 960 / size.1.max(1), 960);
    let proxy = (proxy.0.max(2) & !1, proxy.1);
    let step = (1_000_000.0 / project.fps) as Micros;
    let sequential = 30;

    let started = std::time::Instant::now();
    let mut worst = 0.0_f64;
    for i in 0..sequential {
        let frame_started = std::time::Instant::now();
        let _ = compositor.render_frame(&project, i as Micros * step, proxy, &sources)?;
        worst = worst.max(frame_started.elapsed().as_secs_f64() * 1000.0);
    }
    let elapsed = started.elapsed().as_secs_f64();

    println!(
        "{sequential} sequential frames at {}x{}: {:.1} ms mean, {:.1} ms worst, {:.1} fps",
        proxy.0,
        proxy.1,
        elapsed * 1000.0 / sequential as f64,
        worst,
        sequential as f64 / elapsed
    );
    Ok(())
}
