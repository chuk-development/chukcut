//! The native player's preview pipeline, measured headless on the real GPU.
//!
//! ```bash
//! cargo run --release -p chukcut-engine --example player_bench
//! cargo run --release -p chukcut-engine --example player_bench -- --quick
//! ```
//!
//! The app (`crates/app/src/player.rs`) shows the preview through GPUI, which
//! owns a second wgpu device. Every frame therefore travels: decode → composite
//! → read back → (CPU swizzle to BGRA) → `RenderImage` → GPUI uploads it into
//! its atlas on the UI thread. This binary runs exactly that chain without a
//! window, so it can run on the GPU while somebody else uses the display, and
//! it prints both arms of every change in one process:
//!
//! - **legacy** — what `player.rs` did before the `FramePlayer`: one request
//!   slot, `Compositor::render_frame` (blocking readback), a CPU byte swap.
//! - **player** — `preview::player::FramePlayer`: BGRA swizzled on the GPU into
//!   a tightly packed buffer, a ring of async-mapped readback buffers, decode of
//!   the next frame overlapped with the readback of this one, render-ahead
//!   during playback with late frames skipped.
//!
//! GPUI's own upload (`WgpuAtlas::upload_texture`: copy the bytes, create a
//! texture the size of the frame, `write_texture`) is emulated on the engine's
//! device with the same three calls, because it is the same work and a second
//! Vulkan device in one headless process is a risk this project has already
//! paid for once. It is reported separately: it runs on the UI thread, not the
//! render thread, so it costs smoothness of the UI rather than preview fps.
//!
//! Media is generated once under `crates/engine/target/player-bench/` with
//! ffmpeg: a 1080p H.264 clip, a second 1080p clip, and 4K HEVC and H.264
//! clips, all with light noise so the decoders see realistic bitrates.
//!
//! Projects, per resolution (canvas = the clip's size, 30 fps):
//!
//! - 1 layer: one clip, full frame.
//! - 3 layers: the clip with a grade (contrast, saturation, vignette, grain), a
//!   second clip as a graded picture-in-picture, and a title.
//! - 5 layers: the 3, plus a third clip as a second picture-in-picture and a
//!   title animated letter by letter (the CPU text animator) for the whole span.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};

use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::motion::edit;
use chukcut_engine::modules::preview::clock::{frame_at, frame_time};
use chukcut_engine::modules::project::animation::{TextPreset, TextSlot};
use chukcut_engine::modules::project::document::{
    ColorAdjustMaterial, TextAlign, TextMaterial, VideoMaterial,
};
use chukcut_engine::modules::project::grade::Grade;
use chukcut_engine::modules::project::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, SAMPLE_SLACK,
};
use chukcut_engine::modules::render::{Compositor, RenderContext};

const FPS: f64 = 30.0;
/// How long the generated clips are. Longer than any window measured, so no
/// measurement ever runs off the end of the media.
const CLIP_SECONDS: f64 = 12.0;
/// The app's tick (`crates/app/src/theme.rs`, `TICK`).
const TICK: Duration = Duration::from_millis(8);

struct Options {
    quick: bool,
    only: Option<String>,
}

fn main() {
    let options = Options {
        quick: std::env::args().any(|a| a == "--quick"),
        only: std::env::args()
            .skip_while(|a| a != "--only")
            .nth(1)
            .map(|s| s.to_lowercase()),
    };
    ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Fatal);
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skip: no GPU adapter");
        return;
    };
    let info = ctx.adapter_info();
    println!(
        "GPU: {} ({:?}), load {}",
        info.name,
        info.backend,
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .unwrap_or("?")
    );

    let media = match Media::ensure() {
        Ok(media) => media,
        Err(error) => {
            eprintln!("skip: {error}");
            return;
        }
    };

    let scenarios = [
        Scenario {
            label: "1080p",
            canvas: (1920, 1080),
            render: (1920, 1080),
            main: media.a1080.clone(),
            others: [media.b1080.clone(), media.c1080.clone()],
        },
        Scenario {
            label: "4K",
            canvas: (3840, 2160),
            render: (3840, 2160),
            main: media.a2160_hevc.clone(),
            others: [media.b2160.clone(), media.c1080.clone()],
        },
        // The common case in the app: 4K footage in a viewer a quarter of its
        // size. The readback is small; decode dominates.
        Scenario {
            label: "4K in a 960x540 viewer",
            canvas: (3840, 2160),
            render: (960, 540),
            main: media.a2160_hevc.clone(),
            others: [media.b2160.clone(), media.c1080.clone()],
        },
    ];

    let wanted = |name: &str| options.only.as_deref().is_none_or(|o| name.contains(o));

    if wanted("stages") {
        heading(
            "Per frame, serial, legacy path (ms) — what one frame costs with nothing overlapped",
        );
        println!(
            "{:<24} {:>6} {:>8} {:>9} {:>9} {:>8} {:>8} {:>9} {:>8} {:>8}",
            "scenario",
            "layers",
            "sources",
            "composite",
            "readback",
            "of wait",
            "swizzle",
            "UI upload",
            "total",
            "ceiling"
        );
        for scenario in &scenarios {
            for layers in [1, 3, 5] {
                let project = scenario.project(layers);
                let frames = if options.quick { 20 } else { 60 };
                let row = legacy_stages(&ctx, &project, scenario.render, frames);
                println!(
                    "{:<24} {:>6} {:>8.2} {:>9.2} {:>9.2} {:>8.2} {:>8.2} {:>9.2} {:>8.2} {:>6.0}fps",
                    scenario.label,
                    layers,
                    row.sources,
                    row.composite,
                    row.readback,
                    row.wait,
                    row.swizzle,
                    row.upload,
                    row.total(),
                    1000.0 / row.render_thread()
                );
            }
        }
    }

    if wanted("playback") {
        heading("Playback at 30 fps in real time, 4 s of timeline — frames shown, of 120");
        println!(
            "{:<24} {:>6} {:>8} {:>9} {:>8} {:>10} {:>10}",
            "scenario", "layers", "arm", "shown", "fps", "lag (fr)", "max gap"
        );
        let seconds = if options.quick { 2.0 } else { 4.0 };
        for scenario in &scenarios {
            for layers in [1, 3, 5] {
                let project = Arc::new(scenario.project(layers));
                let legacy = LegacyPlayer::new(Arc::clone(&ctx));
                let result = playback(&legacy, &ctx, &project, scenario.render, seconds);
                drop(legacy);
                result.print(scenario.label, layers, "legacy");
            }
        }
    }
}

// --- the legacy path, stage by stage -------------------------------------------------

#[derive(Default)]
struct Stages {
    sources: f64,
    composite: f64,
    readback: f64,
    wait: f64,
    swizzle: f64,
    upload: f64,
}

impl Stages {
    /// What the render thread spends per frame: everything but the UI upload.
    fn render_thread(&self) -> f64 {
        self.sources + self.composite + self.readback + self.swizzle
    }

    fn total(&self) -> f64 {
        self.render_thread() + self.upload
    }
}

fn legacy_stages(
    ctx: &Arc<RenderContext>,
    project: &Project,
    size: (u32, u32),
    frames: usize,
) -> Stages {
    let compositor = Compositor::new(Arc::clone(ctx));
    let sources = MediaSourceProvider::from_project(project);
    // One untimed frame: it opens every decoder and allocates the targets.
    let start = frame_time(30, FPS);
    let _ = compositor.render_frame(project, start + SAMPLE_SLACK, size, &sources);
    compositor.reset_stats();

    let mut swizzle = 0.0;
    let mut upload = 0.0;
    for n in 0..frames as i64 {
        let at = frame_time(31 + n, FPS) + SAMPLE_SLACK;
        let mut pixels = compositor
            .render_frame(project, at, size, &sources)
            .expect("render");
        let started = Instant::now();
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        swizzle += ms(started);
        let started = Instant::now();
        gpui_upload(ctx, &pixels, size);
        upload += ms(started);
    }
    let stats = compositor.stats();
    let per = |ns: u64| stats.per_frame(ns) / 1e6;
    Stages {
        sources: per(stats.sources_ns),
        composite: per(stats.composite_ns),
        readback: per(stats.readback_ns),
        wait: per(stats.readback_wait_ns),
        swizzle: swizzle / frames as f64,
        upload: upload / frames as f64,
    }
}

/// What GPUI does with a new `RenderImage` the first time it is painted
/// (`gpui_wgpu::WgpuAtlas`): copy the bytes, allocate a texture for the tile
/// (a frame is larger than the 1024² default atlas, so every frame gets a new
/// one), and queue a `write_texture`, which copies the bytes once more into a
/// staging buffer.
fn gpui_upload(ctx: &RenderContext, bgra: &[u8], size: (u32, u32)) {
    let copy = bgra.to_vec();
    let extent = wgpu::Extent3d {
        width: size.0,
        height: size.1,
        depth_or_array_layers: 1,
    };
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("emulated gpui atlas tile"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &copy,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.0 * 4),
            rows_per_image: None,
        },
        extent,
    );
    ctx.queue().submit(None);
}

// --- playback in real time --------------------------------------------------------------

/// The two arms, as the app's editor sees them.
trait Arm {
    /// The tick's request: the project at the clock's time.
    fn request(&self, project: &Arc<Project>, time: Micros, size: (u32, u32), playing: bool);
    /// The frame to put on screen now, if a new one is due: its timeline time
    /// and its BGRA bytes.
    fn take(&self, clock: Micros) -> Option<(Micros, Vec<u8>)>;
}

struct PlaybackResult {
    shown: usize,
    expected: usize,
    seconds: f64,
    mean_lag: f64,
    max_gap: i64,
}

impl PlaybackResult {
    fn print(&self, scenario: &str, layers: usize, arm: &str) {
        println!(
            "{:<24} {:>6} {:>8} {:>5}/{:<3} {:>8.1} {:>10.2} {:>10}",
            scenario,
            layers,
            arm,
            self.shown,
            self.expected,
            self.shown as f64 / self.seconds,
            self.mean_lag,
            self.max_gap
        );
    }
}

/// Drive `arm` the way `Editor::tick` does: every 8 ms, read the clock, ask for
/// the frame at it, and put up whatever came back.
fn playback(
    arm: &dyn Arm,
    ctx: &RenderContext,
    project: &Arc<Project>,
    size: (u32, u32),
    seconds: f64,
) -> PlaybackResult {
    let from = frame_time(30, FPS);
    // Warm: the first frame opens the decoders, which a real session paid for
    // when the clip was first shown, not when play was pressed.
    arm.request(project, from + SAMPLE_SLACK, size, false);
    let warm_until = Instant::now() + Duration::from_secs(5);
    while arm.take(from).is_none() && Instant::now() < warm_until {
        std::thread::sleep(Duration::from_millis(2));
    }

    let span = (seconds * 1e6) as Micros;
    let started = Instant::now();
    let mut shown: Vec<i64> = Vec::new();
    let mut lag_sum = 0i64;
    loop {
        let clock = from + started.elapsed().as_micros() as Micros;
        if clock >= from + span {
            break;
        }
        arm.request(project, clock + SAMPLE_SLACK, size, true);
        if let Some((time, bgra)) = arm.take(clock) {
            gpui_upload(ctx, &bgra, size);
            let frame = frame_at(time, FPS);
            if shown.last() != Some(&frame) {
                shown.push(frame);
                lag_sum += frame_at(clock, FPS) - frame;
            }
        }
        std::thread::sleep(TICK);
    }
    arm.request(project, from + span, size, false);

    let max_gap = shown.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
    PlaybackResult {
        shown: shown.len(),
        expected: (seconds * FPS).round() as usize,
        seconds,
        mean_lag: lag_sum as f64 / shown.len().max(1) as f64,
        max_gap,
    }
}

/// `crates/app/src/player.rs` as it was: one slot, latest wins, a blocking
/// readback and a CPU swizzle on the render thread.
struct LegacyPlayer {
    shared: Arc<LegacyShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct LegacyShared {
    request: Mutex<Option<(Arc<Project>, Micros, (u32, u32))>>,
    wake: Condvar,
    result: Mutex<Option<(Micros, Vec<u8>)>>,
    stop: std::sync::atomic::AtomicBool,
}

impl LegacyPlayer {
    fn new(ctx: Arc<RenderContext>) -> Self {
        let shared = Arc::new(LegacyShared::default());
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || {
                let compositor = Compositor::new(ctx);
                let mut sources: Option<MediaSourceProvider> = None;
                loop {
                    let (project, time, size) = {
                        let mut slot = shared.request.lock();
                        loop {
                            if shared.stop.load(std::sync::atomic::Ordering::Acquire) {
                                return;
                            }
                            if let Some(request) = slot.take() {
                                break request;
                            }
                            shared.wake.wait(&mut slot);
                        }
                    };
                    let provider =
                        sources.get_or_insert_with(|| MediaSourceProvider::from_project(&project));
                    let Ok(mut pixels) = compositor.render_frame(&project, time, size, provider)
                    else {
                        continue;
                    };
                    for pixel in pixels.as_chunks_mut::<4>().0 {
                        pixel.swap(0, 2);
                    }
                    *shared.result.lock() = Some((time, pixels));
                }
            })
        };
        Self {
            shared,
            thread: Some(thread),
        }
    }
}

impl Arm for LegacyPlayer {
    fn request(&self, project: &Arc<Project>, time: Micros, size: (u32, u32), _playing: bool) {
        // The editor only asks when the frame number changes.
        let mut slot = self.shared.request.lock();
        *slot = Some((Arc::clone(project), time, size));
        self.shared.wake.notify_one();
    }

    fn take(&self, _clock: Micros) -> Option<(Micros, Vec<u8>)> {
        self.shared.result.lock().take()
    }
}

impl Drop for LegacyPlayer {
    fn drop(&mut self) {
        self.shared
            .stop
            .store(true, std::sync::atomic::Ordering::Release);
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// --- projects ------------------------------------------------------------------------

struct Scenario {
    label: &'static str,
    canvas: (u32, u32),
    render: (u32, u32),
    main: PathBuf,
    others: [PathBuf; 2],
}

impl Scenario {
    fn project(&self, layers: usize) -> Project {
        let (w, h) = self.canvas;
        let mut project = Project::new(
            "player bench",
            CanvasConfig {
                width: w,
                height: h,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            FPS,
        );
        let span = (CLIP_SECONDS * 1e6) as Micros;
        let video = |project: &mut Project, id: &str, path: &Path, size: (u32, u32)| {
            project.materials.videos.push(VideoMaterial {
                id: id.into(),
                path: path.to_string_lossy().into_owned(),
                width: size.0,
                height: size.1,
                duration: span,
                fps: FPS,
                has_audio: false,
                rotation: 0,
            });
        };
        let segment = |id: &str, material: &str, transform: Transform| Segment {
            id: id.into(),
            material_id: material.into(),
            target_range: TimeRange::new(0, span),
            source_range: TimeRange::new(0, span),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform,
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        };

        video(&mut project, "main", &self.main, self.canvas);
        let mut v1 = Track::new(TrackKind::Video, "V1");
        v1.segments
            .push(segment("main-clip", "main", Transform::default()));
        project.tracks.push(v1);
        if layers == 1 {
            return project;
        }

        // The "effects": a grade with vignette and grain on the main clip.
        let mut grade = ColorAdjustMaterial::identity();
        grade.id = "grade".into();
        grade.contrast = 1.15;
        grade.saturation = 1.2;
        grade.grade = Grade {
            grain: 0.3,
            ..Grade::default()
        };
        grade.grade.vignette.amount = 0.4;
        grade.grade.vignette.midpoint = 0.5;
        grade.grade.vignette.feather = 0.5;
        project.materials.color_adjusts.push(grade);
        project.tracks[0].segments[0].extras.push("grade".into());

        let pip = |x: f32, y: f32| Transform {
            position: [x, y],
            scale: [0.4, 0.4],
            ..Transform::default()
        };
        video(&mut project, "second", &self.others[0], self.canvas);
        let mut v2 = Track::new(TrackKind::Video, "V2");
        let mut second = segment("second-clip", "second", pip(0.5, -0.5));
        second.extras.push("grade".into());
        v2.segments.push(second);
        project.tracks.push(v2);

        let title = |id: &str, content: &str, size: f32| TextMaterial {
            id: id.into(),
            content: content.into(),
            font_family: "sans-serif".into(),
            font_size: size,
            color: [1.0, 1.0, 1.0, 1.0],
            bold: true,
            italic: false,
            align: TextAlign::Center,
            stroke_width: 4.0,
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            shadow: None,
            background: None,
            caption: None,
        };
        let scale = h as f32 / 1080.0;
        project
            .materials
            .texts
            .push(title("title", "chukcut player bench", 72.0 * scale));
        let mut t1 = Track::new(TrackKind::Text, "T1");
        t1.segments.push(segment(
            "title-clip",
            "title",
            Transform {
                position: [0.0, 0.7],
                ..Transform::default()
            },
        ));
        project.tracks.push(t1);
        if layers == 3 {
            return project;
        }

        video(&mut project, "third", &self.others[1], (1920, 1080));
        let mut v3 = Track::new(TrackKind::Video, "V3");
        v3.segments
            .push(segment("third-clip", "third", pip(-0.5, -0.5)));
        project.tracks.push(v3);

        project.materials.texts.push(title(
            "animated",
            "every letter moves on its own",
            64.0 * scale,
        ));
        let mut t2 = Track::new(TrackKind::Text, "T2");
        t2.segments.push(segment(
            "animated-clip",
            "animated",
            Transform {
                position: [0.0, 0.2],
                ..Transform::default()
            },
        ));
        project.tracks.push(t2);
        // In-animation over the whole span, so every measured frame runs the
        // text animator rather than the cached still title.
        let mut animator =
            chukcut_engine::modules::motion::catalog::text_preset(TextPreset::Pop).animator;
        animator.duration = span;
        edit::set_text_command(&project, "animated-clip", TextSlot::In, Some(animator))
            .expect("text animator")
            .apply(&mut project)
            .expect("apply text animator");
        project
    }
}

// --- media ---------------------------------------------------------------------------

struct Media {
    a1080: PathBuf,
    b1080: PathBuf,
    c1080: PathBuf,
    a2160_hevc: PathBuf,
    b2160: PathBuf,
}

impl Media {
    fn ensure() -> Result<Self, String> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("player-bench");
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let make = |name: &str, source: &str, size: &str, codec: &[&str]| {
            generate(&dir.join(name), source, size, codec)
        };
        Ok(Self {
            a1080: make("a_h264_1080.mp4", "testsrc2", "1920x1080", &H264)?,
            b1080: make("b_h264_1080.mp4", "testsrc", "1920x1080", &H264)?,
            c1080: make("c_h264_1080.mp4", "smptehdbars", "1920x1080", &H264)?,
            a2160_hevc: make("a_hevc_2160.mp4", "testsrc2", "3840x2160", &HEVC)?,
            b2160: make("b_h264_2160.mp4", "testsrc", "3840x2160", &H264)?,
        })
    }
}

const H264: [&str; 8] = [
    "-c:v", "libx264", "-preset", "veryfast", "-b:v", "10M", "-g", "15",
];
const HEVC: [&str; 10] = [
    "-c:v",
    "libx265",
    "-preset",
    "ultrafast",
    "-b:v",
    "20M",
    "-g",
    "15",
    "-x265-params",
    "log-level=error",
];

fn generate(out: &Path, source: &str, size: &str, codec: &[&str]) -> Result<PathBuf, String> {
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(out.to_path_buf());
    }
    println!("generating {} …", out.display());
    let staging = out.with_extension("partial.mp4");
    let filter =
        format!("{source}=size={size}:rate=30:duration={CLIP_SECONDS},noise=alls=6:allf=t+u");
    let output = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-nostdin", "-f", "lavfi", "-i"])
        .arg(&filter)
        .args(codec)
        .args(["-pix_fmt", "yuv420p"])
        .arg(&staging)
        .output()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg failed for {}: {}",
            out.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::rename(&staging, out).map_err(|e| format!("cannot publish the clip: {e}"))?;
    Ok(out.to_path_buf())
}

// --- small things ---------------------------------------------------------------------

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn heading(text: &str) {
    println!("\n{text}\n{}", "-".repeat(text.chars().count().min(100)));
}
