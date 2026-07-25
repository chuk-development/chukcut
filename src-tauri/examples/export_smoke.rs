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
//!
//! Options, all optional and all before or after the paths:
//!
//! - `--hardware` — encode on the GPU, using the first encoder `hwaccel`
//!   reports usable. `--hardware=vaapi_h264` names one.
//! - `--size=1920x1080` — override the canvas, so the same timeline can be
//!   measured at landscape HD and at 1080×1920 without editing this file.
//! - `--clip=4` — seconds taken from each input; longer runs give a steadier
//!   frames-per-second number.
//! - `--no-audio` — skip the mix, to time the video path alone.
//! - `--encode-only` — feed the encoder a synthetic frame instead of a
//!   composited one, so the number that comes out is the encoder's and not the
//!   decoder's. The whole-export figure is what a user experiences; this one is
//!   what tells you whether the encoder is the thing worth optimising.
//! - `--no-gpu-nv12` — read the composited frame back as RGBA and let swscale
//!   convert it, the way the export worked before the compute pass existed.
//! - `--no-zero-copy` — do the NV12 conversion on the GPU but still read it
//!   back and upload it, rather than handing the encoder the compositor's own
//!   memory.
//!
//! Those two flags select the three tiers the export can run at, and the runs
//! that produced the table in `docs/STATUS.md` are one of each.
//!
//! Every run prints a `BREAKDOWN` line: the per-frame cost of decoding sources,
//! compositing, and reading the result back. That decomposition is the point —
//! a whole-export figure alone cannot say which stage a change moved.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_lib::modules::audio::FileAudioSource;
use chukcut_lib::modules::export::job::{self, ExportJob, ExportRequest};
use chukcut_lib::modules::export::{hwaccel, ExportProgress};
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

    let mut positional: Vec<String> = Vec::new();
    let mut hardware: Option<Option<String>> = None;
    let mut size: Option<(u32, u32)> = None;
    let mut clip_seconds = 2.0f64;
    let mut include_audio = true;
    let mut encode_only = false;
    let mut gpu_nv12 = true;
    let mut zero_copy = true;

    for arg in std::env::args().skip(1) {
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        match flag {
            "--hardware" => hardware = Some(value),
            "--size" => {
                let value = value.unwrap_or_default();
                let Some((w, h)) = value.split_once(['x', 'X']) else {
                    eprintln!("--size wants WIDTHxHEIGHT, e.g. --size=1920x1080");
                    std::process::exit(2);
                };
                size = Some((w.parse().unwrap_or(0), h.parse().unwrap_or(0)));
            }
            "--clip" => clip_seconds = value.and_then(|v| v.parse().ok()).unwrap_or(2.0),
            "--no-audio" => include_audio = false,
            "--encode-only" => encode_only = true,
            "--no-gpu-nv12" => gpu_nv12 = false,
            "--no-zero-copy" => zero_copy = false,
            other if other.starts_with("--") => {
                eprintln!("unknown option {other}");
                std::process::exit(2);
            }
            _ => positional.push(arg),
        }
    }

    if positional.len() < 2 {
        eprintln!(
            "usage: export_smoke [--hardware[=id]] [--size=WxH] [--clip=SECONDS] [--no-audio] \
             <output.mp4> <video>..."
        );
        std::process::exit(2);
    }
    let output = positional.remove(0);
    let inputs = positional;

    // Which hardware encoder, if any. Resolved before anything expensive so a
    // machine with no usable GPU says so immediately instead of after the mix.
    let hardware_id = match hardware {
        None => None,
        Some(Some(id)) => Some(id),
        Some(None) => {
            let detected = hwaccel::detect();
            for encoder in &detected {
                println!(
                    "hardware: {} ({}) available={} usable={}{}",
                    encoder.id,
                    encoder.encoder_name,
                    encoder.available,
                    encoder.usable,
                    encoder
                        .note
                        .as_ref()
                        .map(|n| format!(" — {n}"))
                        .unwrap_or_default()
                );
            }
            let Some(first) = detected.iter().find(|e| e.usable) else {
                eprintln!("no usable hardware encoder on this machine");
                std::process::exit(1);
            };
            Some(first.id.clone())
        }
    };
    if let Some(id) = &hardware_id {
        println!("using hardware encoder {id}");
    } else {
        println!("using the software encoder");
    }

    // Two seconds of each clip by default, taken from one second in — so the
    // result is a real cut rather than a copy, and a wrong source range shows up
    // as the wrong picture rather than as nothing.
    let clip: Micros = (clip_seconds * 1_000_000.0) as Micros;
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
            target_range: TimeRange::new(playhead, clip),
            source_range: TimeRange::new(SOURCE_IN.min(info.duration / 2), clip),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        playhead += clip;
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

    let overrides = size.map(|(width, height)| chukcut_lib::modules::export::ExportOverrides {
        width: Some(width),
        height: Some(height),
        ..Default::default()
    });

    let request = ExportRequest {
        output_path: output.clone(),
        preset_id: None,
        overrides,
        hardware: hardware_id,
        include_audio,
    };
    let settings = job::resolve_settings(&project, &request)?;
    println!(
        "encoding {} frames to {} ({}x{}) with {} ({:?})",
        settings.total_frames,
        settings.output_path.display(),
        settings.video.width,
        settings.video.height,
        settings.video.encoder_name,
        settings.video.accel
    );

    if encode_only {
        return encoder_only(&settings);
    }

    job::set_gpu_color_convert(gpu_nv12);
    job::set_zero_copy(zero_copy);
    println!(
        "frame path: {}",
        match (gpu_nv12, zero_copy) {
            (false, _) => "RGBA readback, then swscale and an upload on the CPU",
            (true, false) => "NV12 on the GPU, read back, then an upload",
            (true, true) => "NV12 on the GPU into memory the encoder reads directly",
        }
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
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "\ndone: {} frames in {:.2}s -> {}",
        outcome.frames,
        elapsed,
        outcome.output_path.display()
    );
    // The headline number, on one line and easy to grep out of a log, because
    // this is what the hardware-encode work is measured against.
    println!(
        "RESULT encoder={} accel={:?} size={}x{} frames={} seconds={:.2} fps={:.1}",
        export.settings.video.encoder_name,
        export.settings.video.accel,
        export.settings.video.width,
        export.settings.video.height,
        outcome.frames,
        elapsed,
        outcome.frames as f64 / elapsed.max(f64::EPSILON)
    );
    // The decomposition. Without it a change to the export is a single number
    // moving and nobody can say which of decode, composite or readback moved
    // it — which is precisely the hole `docs/research/zero-copy-encode.md` was
    // written around.
    let stats = export.compositor.stats();
    println!(
        "BREAKDOWN frames={} sources={:.2}ms composite={:.2}ms readback={:.2}ms \
         (wait={:.2}ms unpad={:.2}ms) nv12={:.2}ms",
        stats.frames,
        stats.per_frame(stats.sources_ns) / 1e6,
        stats.per_frame(stats.composite_ns) / 1e6,
        stats.per_frame(stats.readback_ns) / 1e6,
        stats.per_frame(stats.readback_wait_ns) / 1e6,
        stats.per_frame(stats.readback_unpad_ns) / 1e6,
        stats.per_frame(stats.nv12_ns) / 1e6,
    );
    let writer = outcome.writer;
    println!(
        "ENCODESIDE frames={} prepare={:.2}ms submit={:.2}ms",
        writer.frames,
        writer.per_frame(writer.prepare_ns) / 1e6,
        writer.per_frame(writer.submit_ns) / 1e6,
    );
    println!("verify with: ffprobe -hide_banner {}", output);
    Ok(())
}

/// Time the encoder with the compositor taken out of the picture.
///
/// The frames are synthetic but not flat: a moving diagonal gradient, so the
/// encoder has real residuals to code and the number is not the one you get
/// from encoding a still image. Everything after `write_video_frame` is the
/// production path — the same scaler, the same hardware upload, the same
/// muxer, the same flush.
fn encoder_only(settings: &chukcut_lib::modules::export::ExportSettings) -> anyhow::Result<()> {
    use chukcut_lib::modules::export::MediaWriter;

    let (width, height) = settings.size();
    // No audio track: nothing here writes samples, and a stream with no packets
    // would make the ffprobe check downstream complain about the wrong thing.
    let mut writer = MediaWriter::create(&settings.output_path, &settings.video, None)?;

    // Built before the clock starts. Painting a 1080×1920 gradient in scalar
    // Rust costs several milliseconds a frame, which is the same order as the
    // encode we are trying to measure; cycling through a handful of prepared
    // buffers keeps that out of the number.
    //
    // The pattern is a *smooth* pair of ramps that translates from frame to
    // frame, not a wrapping modulo pattern. That matters: a modulo gradient is
    // wall-to-wall high-frequency edges, which is close to worst case for x264
    // and nearly free for a fixed-function encoder, so it would flatter the
    // hardware path by a factor of two. Smooth ramps with real motion are much
    // closer to what a timeline actually produces.
    const PATTERNS: u64 = 8;
    let patterns: Vec<Vec<u8>> = (0..PATTERNS)
        .map(|n| {
            let shift = (n * 17) as usize;
            let span = width as usize + 256;
            let mut rgba = vec![0u8; width as usize * height as usize * 4];
            for y in 0..height as usize {
                let row = y * width as usize * 4;
                let green = (y * 255 / height.max(1) as usize) as u8;
                for x in 0..width as usize {
                    let pixel = row + x * 4;
                    rgba[pixel] = ((x + shift) * 255 / span) as u8;
                    rgba[pixel + 1] = green;
                    rgba[pixel + 2] = 255 - green / 2;
                    rgba[pixel + 3] = 255;
                }
            }
            rgba
        })
        .collect();

    let started = std::time::Instant::now();
    for index in 0..settings.total_frames {
        writer.write_video_frame(&patterns[(index % PATTERNS) as usize], index)?;
    }
    writer.finish()?;
    let elapsed = started.elapsed().as_secs_f64();

    println!(
        "RESULT encode-only encoder={} accel={:?} size={}x{} frames={} seconds={:.2} fps={:.1}",
        settings.video.encoder_name,
        settings.video.accel,
        width,
        height,
        settings.total_frames,
        elapsed,
        settings.total_frames as f64 / elapsed.max(f64::EPSILON)
    );
    Ok(())
}
