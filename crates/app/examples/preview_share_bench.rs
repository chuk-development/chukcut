//! The preview's two ways into GPUI, measured without a window.
//!
//! ```bash
//! cargo run --release -p chukcut --example preview_share_bench
//! cargo run --release -p chukcut --example preview_share_bench -- --seconds 3
//! ```
//!
//! Plays a clip at 30 fps in real time through the engine's `FramePlayer`,
//! once per arm, and does on a second wgpu device exactly what GPUI's renderer
//! does with each frame it is handed:
//!
//! - **readback** — the frame comes back as BGRA bytes; GPUI copies them,
//!   creates a texture the size of the frame and `write_texture`s into it
//!   (`WgpuAtlas::upload_texture`, which is what a `RenderImage` costs).
//! - **shared** — the frame stays in exported GPU memory; GPUI imports each
//!   allocation once (`gpui_wgpu::import_external_buffer`, the patched
//!   renderer's own code) and records one `copy_buffer_to_texture` per frame.
//!
//! The second device is a wgpu 29 device on the adapter GPUI prefers — the
//! window's real one is not reachable without a display. Drawing the texture
//! afterwards costs the same in both arms and is left out.
//!
//! Reported per arm: frames shown of those due, the UI thread's cost per
//! frame, the player's start-to-ready latency, and the process's CPU time
//! over wall time (100 % = one core), which includes decoding.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chukcut_engine::modules::preview::player::{FramePixels, FramePlayer, PlayerRequest, Sharing};
use chukcut_engine::modules::project::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
    SAMPLE_SLACK,
};
use chukcut_engine::modules::render::{RenderContext, SharedFrame};
use gpui::ExternalBufferInfo;
use gpui_wgpu::wgpu;

const FPS: f64 = 30.0;
const CLIP_SECONDS: f64 = 12.0;
/// The app's tick (`crates/app/src/theme.rs`).
const TICK: Duration = Duration::from_millis(8);

fn main() {
    let seconds: f64 = std::env::args()
        .skip_while(|a| a != "--seconds")
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(6.0);
    let Some(ctx) = chukcut_engine::modules::gpu::render_context() else {
        eprintln!("skip: no GPU adapter");
        return;
    };
    let Some(gpui) = gpui_device() else {
        eprintln!("skip: no adapter for the GPUI stand-in");
        return;
    };
    println!(
        "engine: {} | GPUI stand-in: {} | load {}",
        ctx.adapter_info().name,
        gpui.2,
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .unwrap_or("?")
    );

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/preview-share-bench");
    let clips = [
        ("1080p", "1920x1080", (1920, 1080)),
        ("4K", "3840x2160", (3840, 2160)),
    ];
    println!(
        "{:<7} {:<9} {:>7} {:>8} {:>10} {:>10} {:>10} {:>9}",
        "clip", "arm", "shown", "of due", "UI ms/fr", "UI max", "latency", "CPU"
    );
    for (label, size_arg, size) in clips {
        let clip = match generate(&dir, label, size_arg) {
            Ok(clip) => clip,
            Err(error) => {
                eprintln!("skip {label}: {error}");
                continue;
            }
        };
        let project = Arc::new(project(&clip, size));
        for shared in [false, true] {
            let row = play(&ctx, &gpui, &project, size, shared, seconds);
            println!(
                "{:<7} {:<9} {:>7} {:>8} {:>10.2} {:>10.2} {:>8.1}ms {:>8.0}%",
                label,
                row.arm,
                row.shown,
                row.due,
                row.ui_ms,
                row.ui_max_ms,
                row.latency_ms,
                row.cpu_percent
            );
        }
    }
}

struct Row {
    arm: &'static str,
    shown: u64,
    due: u64,
    ui_ms: f64,
    ui_max_ms: f64,
    latency_ms: f64,
    cpu_percent: f64,
}

fn play(
    ctx: &Arc<RenderContext>,
    gpui: &(wgpu::Device, wgpu::Queue, String),
    project: &Arc<Project>,
    size: (u32, u32),
    shared: bool,
    seconds: f64,
) -> Row {
    let player = FramePlayer::with_context(Arc::clone(ctx));
    player.use_shared_frames(shared);
    let request = |time: Micros, playing| PlayerRequest {
        project: Arc::clone(project),
        generation: 1,
        time: time + SAMPLE_SLACK,
        size,
        playing,
    };
    // Warm up: the first frame opens the decoder and allocates everything.
    player.request(request(0, false));
    let until = Instant::now() + Duration::from_secs(20);
    while player.take(SAMPLE_SLACK).is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    let arm = match player.sharing() {
        Sharing::Shared => "shared",
        Sharing::Readback => "readback",
        Sharing::Unavailable => "fallback",
    };

    let mut ui = Ui::default();
    let mut shown = 0u64;
    let mut ui_total = Duration::ZERO;
    let mut ui_max = Duration::ZERO;
    let cpu_before = cpu_seconds();
    let started = Instant::now();
    let length = Duration::from_secs_f64(seconds);
    while started.elapsed() < length {
        let tick = Instant::now();
        let clock = started.elapsed().as_micros() as Micros;
        player.request(request(clock, true));
        if let Some(frame) = player.take(clock) {
            let t = Instant::now();
            match frame.pixels {
                FramePixels::Bgra(bytes) => ui.upload(gpui, &bytes, size),
                FramePixels::Shared(frame) => ui.copy(gpui, frame),
            }
            let spent = t.elapsed();
            ui_total += spent;
            ui_max = ui_max.max(spent);
            shown += 1;
        }
        // GPUI's renderer submits every frame, which runs wgpu's callbacks.
        let _ = gpui.0.poll(wgpu::PollType::Poll);
        if let Some(rest) = TICK.checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    let wall = started.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - cpu_before;
    let stats = player.stats();
    drop(player);
    let _ = gpui.0.poll(wgpu::PollType::wait_indefinitely());
    Row {
        arm,
        shown,
        due: (seconds * FPS) as u64,
        ui_ms: ui_total.as_secs_f64() * 1e3 / shown.max(1) as f64,
        ui_max_ms: ui_max.as_secs_f64() * 1e3,
        latency_ms: stats.latency_ms,
        cpu_percent: cpu / wall * 100.0,
    }
}

/// What GPUI's renderer does with a frame, on GPUI's wgpu.
#[derive(Default)]
struct Ui {
    /// The previous frame's texture, dropped when the next arrives — as
    /// `cx.drop_image` does for a `RenderImage`.
    texture: Option<wgpu::Texture>,
    imports: HashMap<u64, wgpu::Buffer>,
}

impl Ui {
    /// `WgpuAtlas::upload_texture` for a frame bigger than an atlas page.
    fn upload(
        &mut self,
        (device, queue, _): &(wgpu::Device, wgpu::Queue, String),
        bgra: &[u8],
        size: (u32, u32),
    ) {
        let copy = bgra.to_vec();
        let texture = self.texture(device, size);
        queue.write_texture(
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
            extent(size),
        );
        queue.submit(None);
        self.texture = Some(texture);
    }

    /// `ExternalBuffers::prepare` for one surface.
    fn copy(
        &mut self,
        (device, queue, _): &(wgpu::Device, wgpu::Queue, String),
        frame: SharedFrame,
    ) {
        let info = info(&frame);
        let texture = match self.texture.take() {
            Some(texture) if (texture.width(), texture.height()) == (info.width, info.height) => {
                texture
            }
            _ => self.texture(device, (info.width, info.height)),
        };
        let buffer = self.imports.entry(info.allocation_id).or_insert_with(|| {
            gpui_wgpu::import_external_buffer(device, &info).expect("same-GPU import")
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(info.bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            extent((info.width, info.height)),
        );
        queue.submit(Some(encoder.finish()));
        // Hold the engine's buffer until the copy has run, as the renderer
        // does.
        queue.on_submitted_work_done(move || drop(frame));
        self.texture = Some(texture);
    }

    fn texture(&self, device: &wgpu::Device, size: (u32, u32)) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bench frame"),
            size: extent(size),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }
}

fn extent((width, height): (u32, u32)) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

/// The app's `player::external_buffer`, minus the owner and the flag.
fn info(frame: &SharedFrame) -> ExternalBufferInfo {
    let buffer = &frame.buffer;
    ExternalBufferInfo {
        allocation_id: buffer.id(),
        frame_id: frame.frame_id,
        fd: buffer.fd(),
        buffer_size: buffer.buffer_size(),
        usage: buffer.usage(),
        allocation_size: buffer.allocation_size(),
        memory_type_index: buffer.memory_type_index(),
        device_uuid: buffer.identity().device_uuid,
        driver_uuid: buffer.identity().driver_uuid,
        width: frame.width,
        height: frame.height,
        bytes_per_row: frame.bytes_per_row,
    }
}

/// A wgpu 29 device on the adapter GPUI would pick.
fn gpui_device() -> Option<(wgpu::Device, wgpu::Queue, String)> {
    gpui_adapters().into_iter().find_map(|adapter| {
        let name = adapter.get_info().name;
        let (device, queue) = block_on(adapter.request_device(&Default::default())).ok()?;
        Some((device, queue, name))
    })
}

/// The adapters in the order GPUI's `WgpuContext` tries them with no
/// compositor hint (Vulkan and GL; discrete, integrated, other, virtual, CPU;
/// Vulkan first within each). It takes the first that opens.
fn gpui_adapters() -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let mut adapters = block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    adapters.sort_by_key(|adapter| {
        let info = adapter.get_info();
        let kind = match info.device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::Other => 2,
            wgpu::DeviceType::VirtualGpu => 3,
            wgpu::DeviceType::Cpu => 4,
        };
        (kind, u8::from(info.backend != wgpu::Backend::Vulkan))
    });
    adapters
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
        std::thread::yield_now();
    }
}

/// User plus system CPU time of this process, in seconds.
fn cpu_seconds() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Fields after the parenthesised command name; utime and stime are the
    // 14th and 15th of the whole line, in clock ticks of 1/100 s.
    let rest = stat.rsplit_once(')').map(|(_, r)| r).unwrap_or("");
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let ticks = |i: usize| {
        fields
            .get(i)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (ticks(11) + ticks(12)) / 100.0
}

fn project(clip: &Path, (w, h): (u32, u32)) -> Project {
    let mut project = Project::new(
        "preview share bench",
        CanvasConfig {
            width: w,
            height: h,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        FPS,
    );
    let span = (CLIP_SECONDS * 1e6) as Micros;
    project.materials.videos.push(VideoMaterial {
        id: "clip".into(),
        path: clip.to_string_lossy().into_owned(),
        width: w,
        height: h,
        duration: span,
        fps: FPS,
        has_audio: false,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(Segment {
        id: "clip".into(),
        material_id: "clip".into(),
        target_range: TimeRange::new(0, span),
        source_range: TimeRange::new(0, span),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    });
    project.tracks.push(track);
    project
}

/// An H.264 test clip with light noise, generated once.
fn generate(dir: &Path, label: &str, size: &str) -> Result<PathBuf, String> {
    let out = dir.join(format!("{label}.mp4"));
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(out);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    println!("generating {} …", out.display());
    let staging = out.with_extension("partial.mp4");
    let filter =
        format!("testsrc2=size={size}:rate=30:duration={CLIP_SECONDS},noise=alls=6:allf=t+u");
    let output = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-nostdin", "-f", "lavfi", "-i"])
        .arg(&filter)
        .args([
            "-c:v", "libx264", "-preset", "veryfast", "-b:v", "12M", "-g", "15",
        ])
        .args(["-pix_fmt", "yuv420p"])
        .arg(&staging)
        .output()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    std::fs::rename(&staging, &out).map_err(|e| format!("cannot publish the clip: {e}"))?;
    Ok(out)
}
