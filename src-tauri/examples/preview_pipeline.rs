//! Where the preview's frame budget actually goes, on a **running** server.
//!
//! ```bash
//! cd src-tauri
//! cargo run --release --example preview_pipeline -- \
//!     --clip "/path/to/clip.mkv" --seconds 8
//! ```
//!
//! ## Why this exists when `chukcut-bench --filter preview-frame` already runs
//!
//! That group measures one frame's **latency** through the same calls
//! `preview::server::render_one` makes, serially, with nothing else happening:
//! 10.7 ms at 1920×1080. The owner's playback is 11–20 fps, which is 50–90 ms a
//! frame. A latency figure cannot explain a throughput figure, and the gap
//! between the two is the whole question.
//!
//! So this does the thing the suite deliberately does not: it starts a real
//! [`PreviewServer`], with its render thread, its encode thread, its pacer, its
//! ring and its clock, plays real media at wall-clock speed, and reports **how
//! much of each thread's life went where**. Occupancy, not duration — two
//! stages that each fill half of *one* thread are a serial pipeline and their
//! costs add; the same two on different threads overlap and only the larger
//! counts. `modules/preview/probe.rs` is the counter set, and the header on it
//! is the argument.
//!
//! It also stands in for the webview, because the webview is where the last of
//! the frame budget goes and nothing else in this repository touches that path:
//! every position event is turned into a `chukcut-frame://` request on a pool
//! the same size as the real protocol handler's, and the time from "the pacer
//! announced this frame" to "there are bytes for it" is measured. What happens
//! *after* that — WebKit's own JPEG decode, `createImageBitmap`, `drawImage` —
//! is not measurable from Rust and this program does not pretend to.
//!
//! ## Phases
//!
//! 1. **serial** — one Compositor, one provider, frames walked forward, per
//!    stage from `RenderStats`. This is `preview-frame` on the real clip, and it
//!    is the control: if the live phase costs more than this, the difference is
//!    contention and not pixels.
//! 2. **live** — the whole server, playing, with the webview simulated.
//! 3. **live at a reduced proxy** — the same, with `--long-edge`, which prices
//!    the "we render at full canvas resolution into a widget half that size"
//!    question in milliseconds rather than in opinion.
//!
//! Nothing here changes the pipeline. The probe is atomic adds; this file only
//! reads them.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chukcut_lib::modules::media::MediaSourceProvider;
use chukcut_lib::modules::preview::encoder::{encode_preview_jpeg, hardware_available};
use chukcut_lib::modules::preview::probe::PROBE;
use chukcut_lib::modules::preview::vaapi::VaapiJpegEncoder;
use chukcut_lib::modules::preview::{
    frame_url, Counts, PreviewOptions, PreviewServer, DEFAULT_JPEG_QUALITY,
    DEFAULT_READ_AHEAD,
};
use chukcut_lib::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform, VideoMaterial,
};
use chukcut_lib::modules::render::{Compositor, RenderContext, SourceProvider};

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

struct Options {
    clip: String,
    /// Canvases to measure. Both orientations by default, because the JPEG
    /// encoder and the readback are not symmetric in them.
    canvases: Vec<(u32, u32)>,
    seconds: f64,
    fps: f64,
    /// Proxy long edge for the third phase. `None` skips it.
    long_edge: Option<u32>,
    /// Skip the serial control phase.
    live_only: bool,
}

fn usage() -> String {
    "preview_pipeline — where a running preview's frame budget goes\n\
     \n\
     usage: cargo run --release --example preview_pipeline -- [options]\n\
     \n\
     --clip <path>        media to play (required)\n\
     --canvas WxH         measure this canvas; repeatable (default 1920x1080 and 1080x1920)\n\
     --seconds <n>        wall seconds of playback per phase (default 8)\n\
     --fps <n>            project frame rate, i.e. the demand (default 24)\n\
     --long-edge <n>      also play at this reduced proxy (default 960; 0 disables)\n\
     --live-only          skip the serial control phase\n\
     -h, --help           this\n"
        .into()
}

fn parse() -> Result<Options, String> {
    let mut options = Options {
        clip: String::new(),
        canvases: Vec::new(),
        seconds: 8.0,
        fps: 24.0,
        long_edge: Some(960),
        live_only: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{key} needs a value"))
        };
        match key.as_str() {
            "--clip" => options.clip = value()?,
            "--canvas" => {
                let raw = value()?;
                let (w, h) = raw
                    .split_once('x')
                    .ok_or_else(|| format!("{raw} is not WxH"))?;
                options.canvases.push((
                    w.parse().map_err(|_| format!("{raw} is not WxH"))?,
                    h.parse().map_err(|_| format!("{raw} is not WxH"))?,
                ));
            }
            "--seconds" => options.seconds = value()?.parse().map_err(|_| "--seconds")?,
            "--fps" => options.fps = value()?.parse().map_err(|_| "--fps")?,
            "--long-edge" => {
                let n: u32 = value()?.parse().map_err(|_| "--long-edge")?;
                options.long_edge = if n == 0 { None } else { Some(n) };
            }
            "--live-only" => options.live_only = true,
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}\n\n{}", usage())),
        }
    }
    if options.clip.is_empty() {
        return Err(format!("--clip is required\n\n{}", usage()));
    }
    if options.canvases.is_empty() {
        options.canvases = vec![(1920, 1080), (1080, 1920)];
    }
    Ok(options)
}

// ---------------------------------------------------------------------------
// The project under test
// ---------------------------------------------------------------------------

/// One clip filling the canvas: the cheapest shape a preview frame has, and the
/// one the owner is looking at. A multi-layer timeline costs what
/// `chukcut-bench --filter composite` says on top of this.
fn single_clip(clip: &str, canvas: (u32, u32), fps: f64, span: Micros) -> Project {
    // Probed, never guessed. `provider::fitted_height` derives the *decode*
    // resolution from the material's declared display size, so declaring the
    // canvas size here rather than the file's would silently measure a
    // different program: a 720x540 file declared as 1920x1080 is never
    // downscaled by swscale, because the fit refuses to upscale.
    let info = chukcut_lib::modules::media::probe(clip).expect("probe the clip");
    let video = info.video.as_ref().expect("the clip has a video stream");
    let mut project = Project::new(
        "preview pipeline",
        CanvasConfig {
            width: canvas.0 & !1,
            height: canvas.1 & !1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        fps,
    );
    let material_id = "clip".to_string();
    project.materials.videos.push(VideoMaterial {
        id: material_id.clone(),
        path: clip.to_string(),
        width: video.display_width,
        height: video.display_height,
        duration: span,
        fps: video.fps,
        has_audio: false,
        rotation: video.rotation,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(Segment {
        id: "seg".into(),
        material_id,
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

// ---------------------------------------------------------------------------
// Phase 1: the serial control
// ---------------------------------------------------------------------------

fn serial_phase(ctx: &Arc<RenderContext>, project: &Project, frames: usize) {
    let compositor = Compositor::new(Arc::clone(ctx));
    let sources = MediaSourceProvider::from_project(project);
    let size = (project.canvas.width, project.canvas.height);
    let interval = (1_000_000.0 / project.fps) as Micros;

    // Untimed: opens the decoder, seeks to the first keyframe, allocates the
    // render target and the VAAPI JPEG surface pool.
    if let Err(error) = compositor.render_frame(project, 0, size, &sources) {
        println!("    serial phase skipped: the first frame did not render: {error}");
        return;
    }
    compositor.reset_stats();

    let mut encode_ns: u64 = 0;
    let mut jpeg_bytes = 0usize;
    let mut backend = None;
    // Kept so the stage split below is measured on a real composited frame
    // rather than on a flat buffer, which converts at a different cost.
    let mut last_rgba: Vec<u8> = Vec::new();
    let started = Instant::now();
    for n in 0..frames {
        let at = n as Micros * interval;
        let rgba = match compositor.render_frame(project, at, size, &sources) {
            Ok(rgba) => rgba,
            Err(error) => {
                println!("    serial phase stopped at frame {n}: {error}");
                break;
            }
        };
        last_rgba = rgba.clone();
        let encoded = Instant::now();
        match encode_preview_jpeg(&rgba, size.0, size.1, DEFAULT_JPEG_QUALITY) {
            Ok((bytes, used)) => {
                jpeg_bytes = bytes.len();
                backend = Some(used);
            }
            Err(error) => {
                println!("    serial phase stopped encoding at frame {n}: {error}");
                break;
            }
        }
        encode_ns += encoded.elapsed().as_nanos() as u64;
    }
    let wall = started.elapsed().as_secs_f64();
    let stats = compositor.stats();
    let n = stats.frames.max(1) as f64;
    let ms = |total: u64| total as f64 / n / 1e6;

    println!(
        "    decode to texture      {:>7.2} ms   (SourceProvider::frame, per visible segment)",
        ms(stats.sources_ns)
    );
    println!(
        "    composite              {:>7.2} ms   (uniforms, draw calls, submit)",
        ms(stats.composite_ns)
    );
    println!(
        "    readback               {:>7.2} ms   ({:.2} blocked in device.poll + {:.2} unpadding)",
        ms(stats.readback_ns),
        ms(stats.readback_wait_ns),
        ms(stats.readback_unpad_ns)
    );
    println!(
        "    JPEG encode            {:>7.2} ms   ({}, {} KB a frame)",
        encode_ns as f64 / n / 1e6,
        backend.map(|b| b.label()).unwrap_or("nothing"),
        jpeg_bytes / 1024
    );
    println!(
        "    ── whole frame, serial {:>7.2} ms   = {:.1} fps ceiling if nothing overlapped",
        wall * 1000.0 / n,
        n / wall
    );

    if hardware_available() && !last_rgba.is_empty() {
        if let Ok(mut encoder) = VaapiJpegEncoder::open(size.0, size.1, DEFAULT_JPEG_QUALITY) {
            // `stages()` reports the *last* encode, so accumulate rather than
            // reading it once: a single sample on this machine is noise.
            let (mut convert, mut upload, mut encode) = (0u64, 0u64, 0u64);
            let rounds = 12u64;
            let mut total = 0u64;
            for _ in 0..rounds {
                let one = Instant::now();
                if encoder.encode(&last_rgba).is_err() {
                    break;
                }
                total += one.elapsed().as_micros() as u64;
                let stages = encoder.stages();
                convert += stages.convert_micros;
                upload += stages.upload_micros;
                encode += stages.encode_micros;
            }
            let per = |total: u64| total as f64 / rounds as f64 / 1000.0;
            println!(
                "       of the JPEG, on the same composited frame: RGBA→NV12 {:.2} + upload {:.2} + encode {:.2} = {:.2} ms measured end to end",
                per(convert),
                per(upload),
                per(encode),
                per(total),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 2: the live server, with a simulated webview
// ---------------------------------------------------------------------------

/// Everything one live run produced.
struct Live {
    wall: f64,
    /// Distinct frames the pacer announced. This is what the frontend was told
    /// to display, and it tracks the clock rather than the renderer.
    shown: u64,
    /// Frames that reached the ring. **This is the honest frame rate**: a
    /// `shown` the renderer never produced is served as a neighbour or not at
    /// all, and the picture does not move.
    rendered: u64,
    counts: Counts,
    /// How many frames beyond the playhead the ring held, sampled every few
    /// milliseconds. **Not `FrameCache::len`** — the ring is direct-mapped and
    /// never clears old slots, so its length reaches capacity and stays there
    /// whatever the renderer is doing. The lead is the thing that says whether
    /// read-ahead is working.
    lead_mean: f64,
    lead_min: i64,
    /// Fraction of samples where the ring was at or behind the playhead, i.e.
    /// where read-ahead had bought nothing at all.
    lead_starved_fraction: f64,
    /// Time from the pacer's announcement to bytes being ready, in ms.
    deliver_ms: Vec<f64>,
}

fn live_phase(
    server: &Arc<PreviewServer>,
    project: Arc<Project>,
    options: &PreviewOptions,
    seconds: f64,
) -> Live {
    let shown = Arc::new(AtomicI64::new(0));
    let base = Arc::new(Mutex::new(String::new()));
    let deliver = Arc::new(Mutex::new(Vec::<f64>::new()));

    // The webview's request threads. Four, the same as
    // `server::FRAME_REQUEST_THREADS`, because that is how many the real
    // protocol handler has and a different number measures a different program.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .thread_name(|i| format!("fake-webview-{i}"))
        .build()
        .expect("build the fake webview pool");
    let pool = Arc::new(pool);

    let channel = {
        let shown = Arc::clone(&shown);
        let base = Arc::clone(&base);
        let deliver = Arc::clone(&deliver);
        let pool = Arc::clone(&pool);
        let server = Arc::clone(server);
        tauri::ipc::Channel::new(move |body| {
            let tauri::ipc::InvokeResponseBody::Json(json) = body else {
                return Ok(());
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                return Ok(());
            };
            if value["type"] != "position" {
                return Ok(());
            }
            let frame = value["frame"].as_i64().unwrap_or(-1);
            shown.fetch_add(1, Ordering::Relaxed);

            // Exactly what `Preview.tsx` does with a position event: build the
            // URL and fetch it. Dispatched rather than run here because the
            // real one runs in the webview, not on the pacer.
            let url = format!("{}/{}", base.lock().expect("base url").clone(), frame);
            if url.starts_with('/') {
                return Ok(());
            }
            let announced = Instant::now();
            let server = Arc::clone(&server);
            let deliver = Arc::clone(&deliver);
            pool.spawn(move || {
                let _response = server.serve_uri(&url);
                deliver
                    .lock()
                    .expect("deliver log")
                    .push(announced.elapsed().as_secs_f64() * 1000.0);
            });
            Ok(())
        })
    };

    let info = server.start(Arc::clone(&project), *options, 0, channel);
    *base.lock().expect("base url") = frame_url(info.session);

    // Let the decoder open and the first frame land before anything is counted;
    // otherwise the phase measures `avformat_open_input`.
    let warm = Instant::now();
    while server.cache().is_empty() && warm.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(200));

    PROBE.reset();
    shown.store(0, Ordering::Relaxed);
    deliver.lock().expect("deliver log").clear();

    // Sample the renderer's lead over the playhead while it plays, every 4 ms,
    // which is `IDLE_TICK`.
    let stop = Arc::new(AtomicBool::new(false));
    let lead_samples = Arc::new(Mutex::new(Vec::<i64>::new()));
    let sampler = {
        let stop = Arc::clone(&stop);
        let lead_samples = Arc::clone(&lead_samples);
        let cache = Arc::clone(server.cache());
        let server = Arc::clone(server);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let due = server.status().frame;
                if let Some(furthest) = cache.frames().last().copied() {
                    lead_samples.lock().expect("lead samples").push(furthest - due);
                }
                std::thread::sleep(Duration::from_millis(4));
            }
        })
    };

    let started = Instant::now();
    server.play().expect("play");
    std::thread::sleep(Duration::from_secs_f64(seconds));
    server.pause().expect("pause");
    let wall = started.elapsed().as_secs_f64();

    stop.store(true, Ordering::Relaxed);
    let _ = sampler.join();

    // Requests in flight when the phase ended would otherwise be counted as
    // never having been answered.
    std::thread::sleep(Duration::from_millis(150));

    let counts = PROBE.snapshot();
    let samples = lead_samples.lock().expect("lead samples").clone();
    let lead_mean = if samples.is_empty() {
        0.0
    } else {
        samples.iter().sum::<i64>() as f64 / samples.len() as f64
    };
    let lead_min = samples.iter().copied().min().unwrap_or(0);
    let lead_starved_fraction = if samples.is_empty() {
        0.0
    } else {
        samples.iter().filter(|n| **n <= 0).count() as f64 / samples.len() as f64
    };

    let deliver_ms = deliver.lock().expect("deliver log").clone();
    Live {
        wall,
        shown: shown.load(Ordering::Relaxed).max(0) as u64,
        rendered: counts.encodes,
        counts,
        lead_mean,
        lead_min,
        lead_starved_fraction,
        deliver_ms,
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn report_live(live: &Live, fps: f64, label: &str) {
    let c = &live.counts;
    let wall_ns = (live.wall * 1e9).max(1.0);
    let pct = |ns: u64| ns as f64 / wall_ns * 100.0;
    let budget = 1000.0 / fps;

    println!("  {label}: {:.2} s of wall clock, demand {fps:.0} fps", live.wall);
    println!(
        "    frames  announced {:>4} ({:>5.1}/s)   reached the ring {:>4} ({:>5.1}/s)   dropped by the pacer {:>4}",
        live.shown,
        live.shown as f64 / live.wall,
        live.rendered,
        live.rendered as f64 / live.wall,
        live.shown as i64 - live.rendered as i64,
    );
    println!(
        "    render thread   composite {:>5.1}% ({:>6.2} ms x{})  inline encode {:>5.1}% ({:>6.2} ms x{})  dispatch {:>4.1}%  waiting {:>5.1}%",
        pct(c.composite_ns),
        Counts::per(c.composite_ns, c.composites),
        c.composites,
        pct(c.inline_encode_ns),
        Counts::per(c.inline_encode_ns, c.inline_encodes),
        c.inline_encodes,
        pct(c.dispatch_ns),
        pct(c.render_wait_ns),
    );
    println!(
        "    encode thread   encoding  {:>5.1}% ({:>6.2} ms x{})  idle {:>5.1}%   ring insert {:.3} ms   {:.1} MB/s of JPEG",
        pct(c.encode_ns),
        Counts::per(c.encode_ns, c.encodes),
        c.encodes,
        pct(c.encode_idle_ns),
        Counts::per(c.insert_ns, c.encodes),
        c.encoded_bytes as f64 / live.wall / 1e6,
    );
    println!(
        "    read-ahead      the ring led the playhead by {:.1} frames on average (cap {}), worst {}; it was at or behind the playhead {:.1}% of the time",
        live.lead_mean,
        DEFAULT_READ_AHEAD,
        live.lead_min,
        live.lead_starved_fraction * 100.0,
    );
    println!(
        "    requests        {} served: {} straight from the ring, {} after waiting, {} a neighbour, {} nothing (204), {} stale (410)",
        c.serve_calls, c.serve_hit, c.serve_wait_hit, c.serve_nearest, c.serve_empty, c.serve_stale,
    );
    println!(
        "                    mean {:.2} ms in serve_uri, of which {:.2} ms blocked waiting for a frame that did not exist",
        Counts::per(c.serve_ns, c.serve_calls),
        Counts::per(c.serve_wait_ns, c.serve_calls),
    );

    let mut deliver = live.deliver_ms.clone();
    deliver.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    if !deliver.is_empty() {
        let mean = deliver.iter().sum::<f64>() / deliver.len() as f64;
        println!(
            "    announced → bytes ready   mean {:.2} ms, median {:.2}, p95 {:.2}, max {:.2}   (budget {budget:.1} ms)",
            mean,
            percentile(&deliver, 0.5),
            percentile(&deliver, 0.95),
            percentile(&deliver, 1.0),
        );
    }
    println!(
        "    channel send    {:.3} ms x{}   ({:.2}% of the wall clock; the webview's own decode and paint are NOT in this number)",
        Counts::per(c.emit_ns, c.emits),
        c.emits,
        pct(c.emit_ns),
    );

    // The claim the whole exercise is about, stated as arithmetic.
    let serial = Counts::per(c.composite_ns, c.composites) + Counts::per(c.encode_ns, c.encodes);
    let pipelined = Counts::per(c.composite_ns, c.composites).max(Counts::per(c.encode_ns, c.encodes));
    println!(
        "    ⇒ composite {:.2} + encode {:.2}: {:.2} ms if serialised ({:.1} fps), {:.2} ms if pipelined ({:.1} fps); observed {:.1} fps",
        Counts::per(c.composite_ns, c.composites),
        Counts::per(c.encode_ns, c.encodes),
        serial,
        1000.0 / serial.max(f64::EPSILON),
        pipelined,
        1000.0 / pipelined.max(f64::EPSILON),
        live.rendered as f64 / live.wall,
    );
}

fn server_capacity() -> usize {
    chukcut_lib::modules::preview::DEFAULT_CAPACITY
}

// ---------------------------------------------------------------------------

fn main() {
    let options = match parse() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    if !std::path::Path::new(&options.clip).exists() {
        eprintln!("no such clip: {}", options.clip);
        std::process::exit(2);
    }

    let Some(ctx) = chukcut_lib::modules::gpu::render_context() else {
        eprintln!("no GPU: this machine cannot run the preview at all");
        std::process::exit(1);
    };

    println!("preview_pipeline");
    println!("  clip        {}", options.clip);
    println!(
        "  machine     load {}, hardware JPEG {}, dmabuf import {}, read-ahead {} frames, ring {} frames",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .split_whitespace()
            .next()
            .unwrap_or("?")
            .to_string(),
        if hardware_available() { "yes" } else { "no" },
        if ctx.can_import_dmabuf() { "yes" } else { "no" },
        DEFAULT_READ_AHEAD,
        server_capacity(),
    );
    println!();

    let span: Micros = 120_000_000;
    let (path, why) = chukcut_lib::modules::preview::decode_path(ctx.can_import_dmabuf());
    let info = chukcut_lib::modules::media::probe(&options.clip).expect("probe the clip");
    if let Some(video) = info.video.as_ref() {
        println!(
            "  source      {}x{} {} at {:.3} fps, {:.0} s; decode path {} ({})",
            video.display_width,
            video.display_height,
            video.codec,
            video.fps,
            info.duration as f64 / 1e6,
            path.label(),
            why,
        );
        println!();
    }

    for canvas in &options.canvases {
        println!("== {}x{} canvas, {} fps ==", canvas.0, canvas.1, options.fps);
        let project = Arc::new(single_clip(&options.clip, *canvas, options.fps, span));

        if !options.live_only {
            println!("  serial, one frame at a time, nothing overlapping:");
            let frames = (options.fps * 2.0) as usize;
            serial_phase(&ctx, &project, frames.max(24));
            println!();
        }

        // One server per canvas, torn down after, so a phase never inherits the
        // previous one's warm decoder or its half-full ring.
        let server = PreviewServer::new();
        let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
        server.set_source_provider(sources);

        let live = live_phase(
            &server,
            Arc::clone(&project),
            &PreviewOptions::default(),
            options.seconds,
        );
        report_live(&live, options.fps, "live, native proxy");

        if let Some(long_edge) = options.long_edge {
            println!();
            // Field assignment rather than a struct literal: `PreviewOptions`
            // gains fields (a viewport, a full-quality flag) as the preview
            // grows, and a literal here would stop compiling every time.
            let mut reduced_options = PreviewOptions::default();
            reduced_options.long_edge = Some(long_edge);
            let reduced = live_phase(
                &server,
                Arc::clone(&project),
                &reduced_options,
                options.seconds,
            );
            report_live(&reduced, options.fps, &format!("live, proxy capped at {long_edge}"));
            println!(
                "    ⇒ capping the proxy moved the composite from {:.2} to {:.2} ms a frame and the rendered rate from {:.1} to {:.1} fps",
                Counts::per(live.counts.composite_ns, live.counts.composites),
                Counts::per(reduced.counts.composite_ns, reduced.counts.composites),
                live.rendered as f64 / live.wall,
                reduced.rendered as f64 / reduced.wall,
            );
        }

        server.stop();
        println!();
    }

    println!("done");
}
