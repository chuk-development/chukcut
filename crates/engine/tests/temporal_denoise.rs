//! Reduce noise in its Temporal mode, through the real decoder: it removes
//! more noise than the spatial pass on footage whose noise changes every
//! frame, and an export shows the frame the preview showed.
//!
//! The clip is generated: testsrc2 with ffmpeg's temporal noise on top,
//! encoded losslessly so the noise is the generator's, not the codec's.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::fx::catalog;
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{CanvasConfig, Micros, Project, Track, TrackKind};
use chukcut_engine::modules::project::effects::{EffectMaterial, EffectValue};
use chukcut_engine::modules::render::{
    Compositor, CompositorConfig, Frame, RenderContext, SourceProvider,
};

use support::{material_for, segment};

const SIZE: (u32, u32) = (320, 240);
const FRAME: f64 = 1_000_000.0 / 30.0;

fn dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("temporal-denoise");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One second of noisy test pattern, or `None` without ffmpeg.
fn noisy_clip() -> Option<PathBuf> {
    let path = dir().join("noisy.mp4");
    if path.exists() {
        return Some(path);
    }
    let made = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=320x240:rate=30:duration=1")
        .args(["-vf", "noise=alls=28:allf=t", "-c:v", "libx264", "-qp", "0"])
        .args(["-pix_fmt", "yuv444p"])
        .arg(&path)
        .status()
        .ok()?;
    made.success().then_some(path)
}

/// The clip on a canvas its size, with Reduce noise in `mode` (0 spatial,
/// 1 temporal), or no effect at all.
fn project(file: &Path, mode: Option<f32>) -> Project {
    let mut p = Project::new(
        "temporal denoise",
        CanvasConfig {
            width: SIZE.0,
            height: SIZE.1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    p.materials
        .videos
        .push(material_for("noisy", file).expect("probe"));
    let mut clip = segment("noisy", 0, 1_000_000);
    if let Some(mode) = mode {
        let mut e = EffectMaterial::new(catalog::DENOISE);
        for (name, value) in [("strength", 70.0), ("detail", 40.0), ("mode", mode)] {
            e.params.insert(name.into(), EffectValue::Number(value));
        }
        clip.extras.push(e.id.clone());
        p.materials.effects.push(e);
    }
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(clip);
    p.tracks.push(track);
    p
}

/// Frames `0..=last` rendered in order, as playback renders them; the last.
fn play_to(compositor: &Compositor, project: &Project, last: u64) -> Frame {
    let sources = MediaSourceProvider::from_project(project);
    let mut frame = None;
    for n in 0..=last {
        let time = (n as f64 * FRAME).round() as Micros + 10;
        frame = Some(
            compositor
                .render(project, time, SIZE, &sources as &dyn SourceProvider)
                .expect("render"),
        );
    }
    frame.expect("at least one frame")
}

/// How much fine detail (noise, mostly) is left: the mean distance of each
/// pixel's green from the mean of its 3x3 neighbourhood.
fn grain(data: &[u8], width: u32, height: u32) -> f64 {
    let g = |x: u32, y: u32| data[((y * width + x) * 4 + 1) as usize] as f64;
    let mut sum = 0.0;
    let mut n = 0.0;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let mut mean = 0.0;
            for dy in 0..3 {
                for dx in 0..3 {
                    mean += g(x + dx - 1, y + dy - 1);
                }
            }
            sum += (g(x, y) - mean / 9.0).abs();
            n += 1.0;
        }
    }
    sum / n
}

#[test]
fn temporal_mode_removes_more_noise_than_the_spatial_pass() {
    let ctx = require_gpu!();
    let Some(file) = noisy_clip() else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    let compositor = Compositor::new(ctx);
    let measure = |mode| {
        let f = play_to(&compositor, &project(&file, mode), 12);
        grain(&f.data, f.width, f.height)
    };
    let (plain, spatial, temporal) = (measure(None), measure(Some(0.0)), measure(Some(1.0)));
    eprintln!("grain: none {plain:.2}, spatial {spatial:.2}, temporal {temporal:.2}");
    assert!(spatial < plain);
    assert!(
        temporal < spatial * 0.85,
        "temporal {temporal:.2} against spatial {spatial:.2}"
    );
}

/// Export `project` and decode its frame 12.
fn exported_frame(ctx: Arc<RenderContext>, project: &Project, name: &str) -> Frame {
    let path = dir().join(format!("{name}.mp4"));
    let _ = std::fs::remove_file(&path);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: None,
    };
    let settings = resolve_settings(project, &request).expect("settings");
    let job = ExportJob {
        job_id: name.into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let sink = FnSink({
        let seen = Arc::new(Mutex::new(0usize));
        move |_| *seen.lock().unwrap() += 1
    });
    run_export(&job, &sink).expect("export");
    let mut decoder = VideoDecoder::open(&path).expect("open the export");
    let decoded = decoder
        .seek_and_decode((12.0 * FRAME).round() as Micros + 16_000)
        .expect("decode");
    let _ = std::fs::remove_file(&path);
    Frame {
        width: decoded.width,
        height: decoded.height,
        data: decoded.data,
    }
}

/// Mean absolute difference of the green channel, per pixel and over 8x8
/// block means (which the encoder's own error mostly averages out of).
fn differences(a: &Frame, b: &Frame) -> (f64, f64) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let g = |f: &Frame, x: u32, y: u32| f.data[((y * f.width + x) * 4 + 1) as usize] as f64;
    let mut pixels = 0.0;
    for y in 0..a.height {
        for x in 0..a.width {
            pixels += (g(a, x, y) - g(b, x, y)).abs();
        }
    }
    let mut blocks = 0.0;
    let mut count = 0.0;
    for by in 0..a.height / 8 {
        for bx in 0..a.width / 8 {
            let mean = |f: &Frame| {
                let mut sum = 0.0;
                for y in 0..8 {
                    for x in 0..8 {
                        sum += g(f, bx * 8 + x, by * 8 + y);
                    }
                }
                sum / 64.0
            };
            blocks += (mean(a) - mean(b)).abs();
            count += 1.0;
        }
    }
    (pixels / (a.width * a.height) as f64, blocks / count)
}

#[test]
fn an_export_shows_the_frame_the_preview_showed() {
    let ctx = require_gpu!();
    let Some(file) = noisy_clip() else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    let temporal = project(&file, Some(1.0));
    let shown = play_to(&Compositor::new(Arc::clone(&ctx)), &temporal, 12);
    let exported = exported_frame(Arc::clone(&ctx), &temporal, "temporal");
    // The same project with the spatial pass only: what a preview/export
    // mismatch in the temporal pass would look like at most.
    let spatial = exported_frame(Arc::clone(&ctx), &project(&file, Some(0.0)), "spatial");
    // And without the effect: how far any export sits from its preview
    // (the encoder, 4:2:0 and limited range), the baseline.
    let plain = project(&file, None);
    let plain_shown = play_to(&Compositor::new(Arc::clone(&ctx)), &plain, 12);
    let (_, baseline) = differences(&plain_shown, &exported_frame(ctx, &plain, "plain"));

    let (same, same_blocks) = differences(&shown, &exported);
    let (other, _) = differences(&shown, &spatial);
    let (preview_grain, export_grain) = (
        grain(&shown.data, SIZE.0, SIZE.1),
        grain(&exported.data, SIZE.0, SIZE.1),
    );
    eprintln!(
        "preview vs export: {same:.2} per pixel, {same_blocks:.2} per block (no effect: \
         {baseline:.2}); vs the spatial export {other:.2}; grain preview {preview_grain:.2}, \
         export {export_grain:.2}"
    );
    assert!(
        same_blocks < baseline + 0.25,
        "block means differ by {same_blocks:.2}, an export without the effect by {baseline:.2}"
    );
    assert!(same < other, "{same:.2} against {other:.2}");
    assert!((preview_grain - export_grain).abs() < 0.6);
}
