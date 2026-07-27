//! What the preview's zero-copy JPEG path is worth, and whether it is right.
//!
//! ```bash
//! cd src-tauri && cargo run --release --example preview_zerocopy
//! cargo run --release --example preview_zerocopy -- --rounds 7
//! ```
//!
//! Three phases, in this order on purpose.
//!
//! **1. Can the JPEG encoder read a linear imported surface at all?** On the
//! Raptor Lake iGPU this repository is developed on, **no** — and finding that
//! out took a row-ramp probe, so the probe stays. One DMA-BUF holding linear
//! NV12 whose luma *is* the row number is handed to three consumers, and the
//! decoded row means say which source row each output row came from. The video
//! encoder and an ordinary upload read it exactly; the JPEG encoder returns the
//! mean of each 32-row block, which is what reading linear memory as tiled looks
//! like. So the DMA-BUF, the strides, the modifier and libavutil's import are
//! all correct — the same buffer, in the same process, encodes perfectly through
//! `h264_vaapi` — and it is the fixed-function JPEG engine that will not have it.
//!
//! `preview::zerocopy::encoder_can_read_linear` runs a cut-down version of that
//! probe once per process and switches the path off where it fails, which is why
//! the preview on this machine still reads frames back and still looks right.
//!
//! **2. Correctness, at widths that are not multiples of 64.** This is second
//! because the failure it looks for is the one that has already cost this
//! project an afternoon. `render::nv12::ROW_ALIGN` exists because a VAAPI
//! *encoder* reads a plane with the pitch it wanted rather than the pitch it was
//! handed, so an unaligned stride encodes as displaced vertical strips —
//! silently, on hardware, and only at some resolutions. The *importer* honours
//! the pitch, so a round-trip test proves nothing: `export::hwframes`' own tests
//! passed at 1440x1080 while every real export at that size was destroyed.
//!
//! So this phase encodes the same composited frame three ways — libjpeg-turbo,
//! the hardware encoder fed a CPU conversion, and the hardware encoder fed the
//! compositor's own memory — decodes all three, and compares them **per pixel**.
//! 1440, 1360, 700 and 394 are 32, 16, 60 and 10 short of a multiple of 64.
//!
//! **3. Speed, both arms in one process.** A "before" taken from a build that no
//! longer exists is not a measurement, so the two paths are run interleaved,
//! same machine, same minute, same clip, and the ratio is the answer. The
//! absolute milliseconds are only worth what the load average makes them worth.
//!
//! Needs ffmpeg on PATH the first time, to build a clip under
//! `target/preview-zerocopy/`, and a GPU with a VAAPI JPEG entrypoint. Both
//! missing are a skip with a reason rather than a failure — this is a
//! measurement tool, not a test.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use chukcut_lib::modules::media::MediaSourceProvider;
use chukcut_lib::modules::preview::encoder::{
    decode_jpeg_rgb, encode_jpeg, encode_preview_jpeg, encode_preview_jpeg_dmabuf,
    hardware_available, psnr_rgb, Backend,
};
use chukcut_lib::modules::preview::vaapi::VaapiJpegEncoder;
use chukcut_lib::modules::preview::zerocopy::{self, PreviewRing};
use chukcut_lib::modules::preview::DEFAULT_JPEG_QUALITY;
use chukcut_lib::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_lib::modules::render::source::YuvRange;
use chukcut_lib::modules::render::{Compositor, Nv12Layout, RenderContext};

/// The clip is 1920x1080; every canvas below is a crop of that aspect or a
/// deliberate mismatch, and all of them are even, because NV12 has no odd edge.
///
/// The first four are the sizes the brief names: 32, 16, 60 and 10 bytes short
/// of a multiple of 64 respectively. The last two are the aligned controls — if
/// an unaligned width scores badly and these do not, the stride is the reason.
const SIZES: [(u32, u32); 6] = [
    (1440, 1080),
    (1360, 768),
    (700, 394),
    (394, 700),
    (1920, 1080),
    (1088, 1920),
];

/// Frames per timed round, and rounds. Enough that one slow frame does not
/// decide the answer; short enough that the whole run is under a minute.
const FRAMES: usize = 24;

fn main() {
    let mut rounds = 5usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--rounds" => rounds = args.next().and_then(|v| v.parse().ok()).unwrap_or(rounds),
            "--help" | "-h" => {
                println!("preview_zerocopy [--rounds N]");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let Some(ctx) = chukcut_lib::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    if !hardware_available() {
        eprintln!("skipping: no hardware JPEG encoder, so there is no zero-copy path to measure");
        return;
    }
    let clip = match ensure_clip() {
        Ok(clip) => clip,
        Err(why) => {
            eprintln!("skipping: {why}");
            return;
        }
    };

    println!("preview_zerocopy");
    println!("  clip     {}", clip.display());
    println!(
        "  machine  load {}, hardware JPEG yes, dmabuf import {}",
        load(),
        if ctx.can_import_dmabuf() { "yes" } else { "no" },
    );
    println!("  quality  {DEFAULT_JPEG_QUALITY}");
    println!(
        "  verdict  the JPEG encoder {} read the compositor's own memory, so the preview {}",
        if zerocopy::encoder_can_read_linear(&ctx) { "CAN" } else { "CANNOT" },
        if zerocopy::encoder_can_read_linear(&ctx) {
            "hands it over"
        } else {
            "reads frames back to the CPU"
        },
    );
    println!();

    row_ramp(&ctx);
    verify(&ctx, &clip);
    speed(&ctx, &clip, rounds);
}

// ---------------------------------------------------------------------------
// 1. Does the encoder read linear memory?
// ---------------------------------------------------------------------------

/// One DMA-BUF, three consumers, and a picture whose luma is its row number.
///
/// Row means are the right statistic and a PSNR is not: the failure this looks
/// for replaces each row with the mean of its 32-row block, which a PSNR
/// reports as "different" without saying how. Reading `0→15 31→15 32→47` off
/// this table is what identified the fault in twenty minutes after two hours of
/// looking at scrambled JPEGs.
fn row_ramp(ctx: &Arc<RenderContext>) {
    use chukcut_lib::modules::export::hwframes::{HwDeviceContext, HwFramesContext, Nv12Dmabuf};
    use chukcut_lib::modules::render::dmabuf::{ExportableBuffer, DRM_FORMAT_MOD_LINEAR};
    use ffmpeg_next as ffmpeg;
    use ffmpeg::format::Pixel;

    heading("1. one linear buffer, three consumers: which row came back where");

    let (width, height) = (640u32, 480u32);
    let (w, h) = (width as usize, height as usize);
    let layout = Nv12Layout::for_size(width, height);
    let total = layout.total_bytes();

    let Some(exported) = ExportableBuffer::new(ctx, total as u64, "row ramp") else {
        println!("  this device cannot export DMA-BUF memory");
        println!();
        return;
    };

    // Luma is the row number; chroma is a flat 128, so a colour fault cannot be
    // mistaken for a layout one.
    let mut picture = vec![128u8; total];
    for row in 0..h {
        picture[row * layout.y_stride..][..w].fill(row as u8);
    }
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: total as u64,
        usage: wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: true,
    });
    staging
        .slice(..)
        .get_mapped_range_mut()
        .expect("staging")
        .copy_from_slice(&picture);
    staging.unmap();
    let mut enc = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_buffer_to_buffer(&staging, 0, exported.buffer(), 0, total as u64);
    ctx.queue().submit(Some(enc.finish()));
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");

    let described = Nv12Dmabuf {
        fd: exported.fd(),
        size: total,
        width,
        height,
        y_offset: 0,
        y_stride: layout.y_stride,
        uv_offset: layout.uv_offset(),
        uv_stride: layout.uv_stride,
        modifier: DRM_FORMAT_MOD_LINEAR,
    };

    const ROWS: [usize; 8] = [0, 1, 31, 32, 63, 64, 128, 255];
    println!(
        "  {:<34}{}",
        "output row",
        ROWS.iter()
            .map(|r| format!("{r:>5}"))
            .collect::<Vec<_>>()
            .join("")
    );
    println!(
        "  {:<34}{}",
        "what it should be",
        ROWS.iter()
            .map(|r| format!("{:>5}", r % 256))
            .collect::<Vec<_>>()
            .join("")
    );

    // The reference: the same picture uploaded into a surface the driver
    // allocated itself, which is what the shipped preview does.
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let level = ((i / w) % 256) as u8;
            [level, level, level, 255]
        })
        .collect();
    let uploaded = VaapiJpegEncoder::open(width, height, 95).and_then(|mut e| e.encode(&rgba));
    match uploaded {
        Ok(bytes) => {
            let (rgb, ..) = decode_jpeg_rgb(&bytes).expect("decode");
            println!("  {:<34}{}", "mjpeg_vaapi, uploaded", row_means(&rgb, w, &ROWS));
        }
        Err(e) => println!("  mjpeg_vaapi upload failed: {e}"),
    }

    // The subject.
    let imported =
        VaapiJpegEncoder::open(width, height, 95).and_then(|mut e| e.encode_dmabuf(&described));
    match imported {
        Ok((bytes, surface)) => {
            let (rgb, ..) = decode_jpeg_rgb(&bytes).expect("decode");
            println!(
                "  {:<34}{}",
                "mjpeg_vaapi, this DMA-BUF",
                row_means(&rgb, w, &ROWS)
            );
            drop(surface);
        }
        Err(e) => println!("  mjpeg_vaapi import failed: {e}"),
    }

    // The control that decides whose fault it is: the *same* buffer, through
    // the encoder the export uses.
    match h264_row_means(&described, width, height, &ROWS) {
        Ok(row) => println!("  {:<34}{row}", "h264_vaapi, this DMA-BUF"),
        Err(e) => println!("  h264_vaapi control failed: {e}"),
    }

    println!();
    println!(
        "  A row of 32-row block means — 0→15, 31→15, 32→47 — is linear memory read as\n  \
         though it were 32-row tiled. If `h264_vaapi` reads the same buffer correctly and\n  \
         `mjpeg_vaapi` does not, nothing about the buffer is wrong and there is nothing on\n  \
         this side of the boundary to fix."
    );
    println!();

    /// The same imported surface through `h264_vaapi`, decoded back.
    fn h264_row_means(
        described: &Nv12Dmabuf<'_>,
        width: u32,
        height: u32,
        rows: &[usize],
    ) -> Result<String, String> {
        let codec = ffmpeg::encoder::find_by_name("h264_vaapi").ok_or("no h264_vaapi")?;
        // SAFETY: the same allocation `preview::vaapi` documents — a context
        // from `avcodec_alloc_context3` carries the encoder's own private
        // option defaults, and `Context::wrap` takes ownership of it.
        let ptr = unsafe { ffmpeg::ffi::avcodec_alloc_context3(codec.as_ptr()) };
        let mut encoder = unsafe { ffmpeg::codec::context::Context::wrap(ptr, None) }
            .encoder()
            .video()
            .map_err(|e| e.to_string())?;
        encoder.set_width(width);
        encoder.set_height(height);
        encoder.set_format(Pixel::VAAPI);
        encoder.set_time_base(ffmpeg::Rational::new(1, 30));
        // Every frame an IDR, so one packet decodes on its own.
        encoder.set_gop(1);
        let device = HwDeviceContext::shared_vaapi().map_err(|e| e.to_string())?;
        let pool = HwFramesContext::create(device, Pixel::VAAPI, Pixel::NV12, width, height, 8)
            .map_err(|e| e.to_string())?;
        pool.attach_to_encoder(&mut encoder).map_err(|e| e.to_string())?;
        let mut encoder = encoder
            .open_as_with(codec, ffmpeg::Dictionary::new())
            .map_err(|e| e.to_string())?;

        let mut surface = pool
            .import_nv12_dmabuf(described)
            .map_err(|e| e.to_string())?;
        surface.set_pts(Some(0));
        encoder.send_frame(&surface).map_err(|e| e.to_string())?;
        encoder.send_eof().map_err(|e| e.to_string())?;
        let mut packet = ffmpeg::Packet::empty();
        encoder.receive_packet(&mut packet).map_err(|e| e.to_string())?;

        let decoder_codec =
            ffmpeg::decoder::find(ffmpeg::codec::Id::H264).ok_or("no h264 decoder")?;
        // SAFETY: as above.
        let ptr = unsafe { ffmpeg::ffi::avcodec_alloc_context3(decoder_codec.as_ptr()) };
        let mut decoder = unsafe { ffmpeg::codec::context::Context::wrap(ptr, None) }
            .decoder()
            .video()
            .map_err(|e| e.to_string())?;
        decoder.send_packet(&packet).map_err(|e| e.to_string())?;
        decoder.send_eof().map_err(|e| e.to_string())?;
        let mut decoded = ffmpeg::util::frame::Video::empty();
        decoder
            .receive_frame(&mut decoded)
            .map_err(|e| e.to_string())?;

        let stride = decoded.stride(0);
        let data = decoded.data(0);
        Ok(rows
            .iter()
            .filter(|&&row| row < height as usize)
            .map(|&row| {
                let sum: u64 = data[row * stride..row * stride + width as usize]
                    .iter()
                    .map(|&v| v as u64)
                    .sum();
                format!("{:>5}", sum / width as u64)
            })
            .collect::<Vec<_>>()
            .join(""))
    }
}

/// Mean luma of each named row of a decoded RGB picture.
fn row_means(rgb: &[u8], w: usize, rows: &[usize]) -> String {
    rows.iter()
        .map(|&row| {
            let sum: u64 = (0..w).map(|col| rgb[(row * w + col) * 3] as u64).sum();
            format!("{:>5}", sum / w as u64)
        })
        .collect::<Vec<_>>()
        .join("")
}

// ---------------------------------------------------------------------------
// 1. Correctness
// ---------------------------------------------------------------------------

/// One comparison of two decoded pictures.
struct Delta {
    psnr: f64,
    /// The worst single channel disagreement anywhere in the frame. A stride
    /// the encoder read differently puts this at 200-plus; quantisation puts it
    /// in the tens on a hard edge and nowhere else.
    max: u8,
    /// Fraction of channel samples differing by more than 8, which is about
    /// where a difference stops being quantisation noise.
    over_8: f64,
}

fn compare(a: &[u8], b: &[u8]) -> Delta {
    let n = a.len().min(b.len());
    let mut max = 0u8;
    let mut over = 0usize;
    for i in 0..n {
        let d = a[i].abs_diff(b[i]);
        max = max.max(d);
        if d > 8 {
            over += 1;
        }
    }
    Delta {
        psnr: psnr_rgb(a, b),
        max,
        over_8: over as f64 / n.max(1) as f64,
    }
}

/// Per-channel mean and spread of a decoded RGB buffer.
///
/// Enough to tell the three ways this usually goes wrong apart without opening
/// an image viewer: a flat picture has a spread near zero, a range mismatch
/// moves the means together, and a plane swap moves R and B past each other.
fn describe(rgb: &[u8]) -> String {
    let mut sum = [0f64; 3];
    let mut sq = [0f64; 3];
    let n = (rgb.len() / 3) as f64;
    for px in rgb.chunks_exact(3) {
        for c in 0..3 {
            sum[c] += px[c] as f64;
            sq[c] += (px[c] as f64) * (px[c] as f64);
        }
    }
    let mut out = String::new();
    for c in 0..3 {
        let mean = sum[c] / n;
        let sd = (sq[c] / n - mean * mean).max(0.0).sqrt();
        out.push_str(&format!("{:>6.1}±{:<5.1} ", mean, sd));
    }
    out
}

fn verify(ctx: &Arc<RenderContext>, clip: &Path) {
    heading("2. the same frame, three ways, compared per pixel");
    println!(
        "  {:>11}  {:>6}  {:>28}  {:>28}",
        "size", "stride", "zero-copy vs libjpeg-turbo", "zero-copy vs the copying GPU"
    );

    for size in SIZES {
        let layout = Nv12Layout::for_size(size.0, size.1);
        let project = single_clip(clip, size);
        let compositor = Compositor::new(Arc::clone(ctx));
        let sources = MediaSourceProvider::from_project(&project);
        let at: Micros = 500_000;

        // Both arms have to see the *same* composition, so the RGBA is taken
        // once and the zero-copy pass renders the same instant. Two different
        // frames would differ by more than any encoder does.
        let Ok(rgba) = compositor.render_frame(&project, at, size, &sources) else {
            println!("  {:>11}  the frame did not render", label(size));
            continue;
        };
        let Ok(software) = encode_jpeg(&rgba, size.0, size.1, DEFAULT_JPEG_QUALITY) else {
            println!("  {:>11}  libjpeg-turbo refused it", label(size));
            continue;
        };
        let Ok((copying, backend)) = encode_preview_jpeg(&rgba, size.0, size.1, DEFAULT_JPEG_QUALITY)
        else {
            println!("  {:>11}  the copying hardware path refused it", label(size));
            continue;
        };

        let Some(zero) = encode_zero_copy(ctx, &compositor, &project, &sources, at, size) else {
            println!("  {:>11}  the zero-copy path was not available", label(size));
            continue;
        };

        let (sw, ..) = decode_jpeg_rgb(&software).expect("decode the software JPEG");
        let (cp, ..) = decode_jpeg_rgb(&copying).expect("decode the copying JPEG");
        let (zc, ..) = decode_jpeg_rgb(&zero).expect("decode the zero-copy JPEG");

        // `CHUKCUT_DUMP=<dir>` writes the three files out. A PSNR says two
        // pictures differ; only looking at them says how, and "how" is the
        // difference between a stride bug, a range bug and a plane swap.
        if let Ok(dir) = std::env::var("CHUKCUT_DUMP") {
            let _ = std::fs::create_dir_all(&dir);
            let stem = label(size);
            let _ = std::fs::write(format!("{dir}/{stem}-software.jpg"), &software);
            let _ = std::fs::write(format!("{dir}/{stem}-copying.jpg"), &copying);
            let _ = std::fs::write(format!("{dir}/{stem}-zerocopy.jpg"), &zero);
            for (name, pixels) in [("software", &sw), ("copying", &cp), ("zerocopy", &zc)] {
                println!("      {name:>9}: {}", describe(pixels));
            }
        }

        let against_software = compare(&sw, &zc);
        let against_copying = compare(&cp, &zc);
        println!(
            "  {:>11}  {:>6}  {:>10.1} dB max {:>3} >8 {:>5.2}%  {:>10.1} dB max {:>3} >8 {:>5.2}%{}",
            label(size),
            layout.y_stride,
            against_software.psnr,
            against_software.max,
            against_software.over_8 * 100.0,
            against_copying.psnr,
            against_copying.max,
            against_copying.over_8 * 100.0,
            if backend == Backend::Vaapi { "" } else { "  (the copying arm fell back to the CPU)" },
        );
    }

    println!();
    println!(
        "  A stride the encoder read differently gives about 19 dB and a max delta in the\n  \
         hundreds — that is what an unaligned width scored before `ROW_ALIGN` existed. A\n  \
         limited-range mismatch gives about 27 dB against libjpeg-turbo with a max delta\n  \
         near 16 everywhere, including in flat black. Above 30 dB with a small `>8` is the\n  \
         same picture, quantised twice."
    );
    println!();
}

/// Composite one frame straight into exported memory and encode it there.
///
/// A miniature of what `preview::server::render_one` does, with the ring built
/// and thrown away around one frame — which is fine here and would not be in the
/// server, where the allocation is the expensive part.
fn encode_zero_copy(
    ctx: &Arc<RenderContext>,
    compositor: &Compositor,
    project: &Project,
    sources: &MediaSourceProvider,
    at: Micros,
    size: (u32, u32),
) -> Option<Vec<u8>> {
    let ring = Arc::new(PreviewRing::new(ctx, size)?);
    let claim = ring.claim()?;
    let layout = Nv12Layout::for_size(size.0, size.1);
    compositor
        .render_nv12_into_range(
            project,
            at,
            size,
            sources,
            claim.ring().buffer(claim.index()),
            YuvRange::Full,
        )
        .ok()?;
    let encoded = {
        let described = claim.ring().describe(claim.index(), &layout);
        encode_preview_jpeg_dmabuf(&described, DEFAULT_JPEG_QUALITY)
    };
    let (bytes, surface) = encoded?;
    claim.release(Some(surface));
    Some(bytes)
}

// ---------------------------------------------------------------------------
// 2. Speed
// ---------------------------------------------------------------------------

fn speed(ctx: &Arc<RenderContext>, clip: &Path, rounds: usize) {
    heading("3. what a whole preview frame costs, both arms interleaved");
    println!(
        "  {:>11}  {:>26}  {:>26}  {:>8}",
        "size", "readback + convert + upload", "straight from the compositor", "ratio"
    );

    for size in [(1920u32, 1080u32), (1088, 1920), (1440, 1080)] {
        let project = single_clip(clip, size);
        // One compositor and one decoder *per arm*, so neither inherits the
        // other's warm texture cache — and both are warmed before the clock
        // starts.
        let copying = Arm::new(ctx, &project);
        let zero = Arm::new(ctx, &project);

        let ring = PreviewRing::new(ctx, size).map(Arc::new);
        if ring.is_none() {
            println!("  {:>11}  this device cannot export DMA-BUF memory", label(size));
            continue;
        }

        let _ = copying
            .compositor
            .render_frame(&project, 0, size, &copying.sources);
        let _ = one_zero_copy_frame(&zero, &project, ring.as_ref().unwrap(), size, 0);

        let (mut copy_best, mut zero_best) = (f64::MAX, f64::MAX);
        for round in 0..rounds {
            copy_best = copy_best.min(time(FRAMES, |n| {
                let at = instant(round, n);
                let rgba = copying
                    .compositor
                    .render_frame(&project, at, size, &copying.sources)
                    .ok()?;
                encode_preview_jpeg(&rgba, size.0, size.1, DEFAULT_JPEG_QUALITY)
                    .ok()
                    .map(|_| ())
            }));
            zero_best = zero_best.min(time(FRAMES, |n| {
                let at = instant(round, n);
                one_zero_copy_frame(&zero, &project, ring.as_ref().unwrap(), size, at)
            }));
        }

        println!(
            "  {:>11}  {:>17.2} ms {:>5.0} fps  {:>17.2} ms {:>5.0} fps  {:>7.2}x",
            label(size),
            copy_best,
            1000.0 / copy_best,
            zero_best,
            1000.0 / zero_best,
            copy_best / zero_best,
        );
    }

    println!();
    println!(
        "  Serial: composite, then encode, with nothing overlapping — one frame's latency,\n  \
         which is what a seek pays and what `chukcut-bench --filter preview-frame` reports.\n  \
         The server overlaps the two on separate threads, so its throughput is better than\n  \
         either column. Best of {rounds} rounds of {FRAMES} frames; the ratio survives a busy\n  \
         machine and the milliseconds do not."
    );
    println!();
}

struct Arm {
    compositor: Compositor,
    sources: MediaSourceProvider,
}

impl Arm {
    fn new(ctx: &Arc<RenderContext>, project: &Project) -> Self {
        Self {
            compositor: Compositor::new(Arc::clone(ctx)),
            sources: MediaSourceProvider::from_project(project),
        }
    }
}

/// Distinct instants walking forward. One repeated instant would measure the
/// provider's texture cache instead of a frame.
fn instant(round: usize, n: usize) -> Micros {
    ((round * FRAMES + n) as Micros) * 33_333 % 2_900_000
}

fn one_zero_copy_frame(
    arm: &Arm,
    project: &Project,
    ring: &Arc<PreviewRing>,
    size: (u32, u32),
    at: Micros,
) -> Option<()> {
    let claim = ring.claim()?;
    let layout = Nv12Layout::for_size(size.0, size.1);
    arm.compositor
        .render_nv12_into_range(
            project,
            at,
            size,
            &arm.sources,
            claim.ring().buffer(claim.index()),
            YuvRange::Full,
        )
        .ok()?;
    let encoded = {
        let described = claim.ring().describe(claim.index(), &layout);
        encode_preview_jpeg_dmabuf(&described, DEFAULT_JPEG_QUALITY)
    };
    let (_, surface) = encoded?;
    claim.release(Some(surface));
    Some(())
}

/// Milliseconds per frame over `frames`, or `NAN` if any of them failed.
fn time(frames: usize, mut body: impl FnMut(usize) -> Option<()>) -> f64 {
    let started = Instant::now();
    for n in 0..frames {
        if body(n).is_none() {
            return f64::NAN;
        }
    }
    started.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

// ---------------------------------------------------------------------------
// Apparatus
// ---------------------------------------------------------------------------

fn heading(text: &str) {
    println!("── {text} {}", "─".repeat(72usize.saturating_sub(text.len())));
}

fn label(size: (u32, u32)) -> String {
    format!("{}x{}", size.0, size.1)
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or("?")
        .to_string()
}

fn single_clip(path: &Path, canvas: (u32, u32)) -> Project {
    const SPAN: Micros = 3_000_000;
    let mut project = Project::new(
        "preview zerocopy",
        CanvasConfig {
            width: canvas.0,
            height: canvas.1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    let material_id = new_id();
    project.materials.videos.push(VideoMaterial {
        id: material_id.clone(),
        path: path.to_string_lossy().into_owned(),
        width: 1920,
        height: 1080,
        duration: SPAN,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(Segment {
        id: new_id(),
        material_id,
        target_range: TimeRange::new(0, SPAN),
        source_range: TimeRange::new(0, SPAN),
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

/// A 1080p H.264 clip in the 5–15 Mbit/s band ordinary delivery occupies.
///
/// Light noise over `testsrc2`, half-second GOPs — the same fixture shape the
/// benchmark suite uses, and for the reason recorded in `benches/fixtures.rs`:
/// a near-incompressible source buries a software decoder while the
/// fixed-function one barely notices, which has reversed a conclusion here
/// before.
fn ensure_clip() -> Result<PathBuf, String> {
    let dir = target_dir().join("preview-zerocopy");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let out = dir.join("h264_1920x1080.mp4");
    if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(out);
    }
    if !Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return Err("ffmpeg is not on PATH, so there is no clip to measure against".into());
    }

    println!("generating {} …", out.display());
    let staging = out.with_extension("partial.mp4");
    let output = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-nostdin"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=1920x1080:rate=30:duration=3,noise=alls=6:allf=t+u",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-b:v",
            "8M",
            "-g",
            "15",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&staging)
        .output()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::rename(&staging, &out).map_err(|e| format!("cannot rename the clip: {e}"))?;
    Ok(out)
}

fn target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
}
