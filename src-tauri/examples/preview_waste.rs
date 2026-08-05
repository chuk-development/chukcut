//! What the preview spends on pixels nobody ever sees, measured both ways.
//!
//! Every number here is an A/B in one process, on the same machine, in the same
//! minute, against the same clip — because the alternative is a "before" taken
//! from a build that no longer exists, which is not a measurement.
//!
//! ```bash
//! cd src-tauri && cargo run --release --example preview_waste
//! ```
//!
//! Four questions, in order:
//!
//! 1. **Render size.** A 1080p project shown in a 700 px panel: what does a
//!    frame cost at the canvas, and what does it cost at the size the screen
//!    can actually display? Both through the same calls `render_one` makes —
//!    decode, upload, composite, read back, JPEG.
//! 2. **The quality ladder.** The same frame at each rung, so the thing
//!    playback buys by getting softer is a number and not a hope.
//! 3. **Late encodes.** A renderer that cannot keep up, driven by an injected
//!    clock so "cannot keep up" is exact, with and without
//!    `experiment::set_skip_late_encodes`.
//! 4. **Duplicate renders.** Several frame requests for one frame that is not
//!    in the ring yet, with and without `experiment::set_dedupe_scrubs`. This
//!    one is a *check* as much as a measurement: if the duplicate does not
//!    happen, there is nothing to fix and the honest answer is to say so.
//!
//! It needs ffmpeg on PATH the first time, to build a clip under
//! `target/preview-waste/`, and a GPU. Both missing are a skip with a reason
//! rather than a failure — this is a measurement tool, not a test.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chukcut_lib::modules::media::MediaSourceProvider;
use chukcut_lib::modules::preview::clock::{frame_time, ManualSource};
use chukcut_lib::modules::preview::encoder::encode_preview_jpeg;
use chukcut_lib::modules::preview::server::experiment;
use chukcut_lib::modules::preview::{
    preview_size, Counts, Ladder, PreviewEvent, PreviewOptions, PreviewServer, Summary, Viewport,
    DEFAULT_CAPACITY, PROBE, RUNGS,
};
use chukcut_lib::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_lib::modules::render::{Compositor, RenderContext};

/// The panel the player occupies in the default layout, in CSS pixels, and the
/// two device-pixel ratios that cover every screen this runs on.
const PANEL: (u32, u32) = (700, 394);

/// Frames per timed round, and how many rounds. Enough that one slow frame does
/// not decide the answer; short enough that the whole run is under a minute.
const FRAMES: usize = 24;
const ROUNDS: usize = 5;

fn main() {
    let Some(ctx) = chukcut_lib::modules::gpu::render_context() else {
        eprintln!("skipping: no GPU adapter on this machine");
        return;
    };
    let clip = match ensure_clip() {
        Ok(clip) => clip,
        Err(why) => {
            eprintln!("skipping: {why}");
            return;
        }
    };

    let project = single_clip(&clip);
    let canvas = (project.canvas.width, project.canvas.height);
    println!("clip     {}", clip.display());
    println!("canvas   {}x{}", canvas.0, canvas.1);
    println!("panel    {}x{} CSS pixels", PANEL.0, PANEL.1);

    render_size(&ctx, &project);
    quality_ladder(&ctx, &project);
    late_encodes(&project);
    duplicate_renders(&project);
}

// ---------------------------------------------------------------------------
// 1. Render size
// ---------------------------------------------------------------------------

fn render_size(ctx: &Arc<RenderContext>, project: &Project) {
    heading("1. render size: the canvas against the panel");

    let canvas = (project.canvas.width, project.canvas.height);
    let cases: Vec<Case> = vec![
        Case::new(
            "canvas (what shipped)",
            preview_size(canvas, None, None, false),
            88,
        ),
        Case::new(
            &format!("panel @1x ({}x{})", PANEL.0, PANEL.1),
            preview_size(canvas, Some(Viewport::new(PANEL.0, PANEL.1)), None, false),
            88,
        ),
        Case::new(
            &format!("panel @2x ({}x{})", PANEL.0 * 2, PANEL.1 * 2),
            preview_size(
                canvas,
                Some(Viewport::new(PANEL.0 * 2, PANEL.1 * 2)),
                None,
                false,
            ),
            88,
        ),
        Case::new(
            "settings cap 960",
            preview_size(
                canvas,
                Some(Viewport::new(PANEL.0, PANEL.1)),
                Some(960),
                false,
            ),
            88,
        ),
    ];

    let full = canvas.0 as f64 * canvas.1 as f64;
    let measured = measure(ctx, project, cases);
    let baseline = measured.first().map(|(_, ms)| *ms).unwrap_or(f64::NAN);
    for (case, ms) in &measured {
        let pixels = case.size.0 as f64 * case.size.1 as f64;
        println!(
            "  {:<26} {:>9}  {:>5.1}% of the canvas's pixels  {:>6.2} ms/frame  \
             {:>5.1} fps  {:>4.2}x",
            case.label,
            format!("{}x{}", case.size.0, case.size.1),
            pixels / full * 100.0,
            ms,
            1000.0 / ms,
            baseline / ms,
        );
    }
    println!(
        "  (ms/frame is decode + upload + composite + readback + JPEG, serially, \
         as a seek pays it)"
    );
}

// ---------------------------------------------------------------------------
// 2. The quality ladder
// ---------------------------------------------------------------------------

fn quality_ladder(ctx: &Arc<RenderContext>, project: &Project) {
    heading("2. the quality ladder");

    let canvas = (project.canvas.width, project.canvas.height);
    // Both, because the ladder is worth very different amounts at the two, and
    // the difference is the whole reason the panel size came first: what a
    // small frame costs is mostly the decode, which no rung can reduce.
    let starts = [
        (
            "panel",
            preview_size(canvas, Some(Viewport::new(PANEL.0, PANEL.1)), None, false),
        ),
        ("canvas", preview_size(canvas, None, None, false)),
    ];

    let cases: Vec<Case> = starts
        .iter()
        .flat_map(|(where_from, full)| {
            (0..RUNGS.len()).map(move |rung| {
                let mut ladder = Ladder::new();
                for _ in 0..rung {
                    let _ = ladder.dropped(1);
                }
                Case::new(
                    &format!("{where_from} rung {rung} (q{})", ladder.quality(88)),
                    ladder.size(*full),
                    ladder.quality(88),
                )
            })
        })
        .collect();

    let measured = measure(ctx, project, cases);
    let mut baseline = f64::NAN;
    for (index, (case, ms)) in measured.iter().enumerate() {
        if index % RUNGS.len() == 0 {
            baseline = *ms;
        }
        println!(
            "  {:<26} {:>9}  {:>6.2} ms/frame  {:>5.1} fps  {:>4.2}x",
            case.label,
            format!("{}x{}", case.size.0, case.size.1),
            ms,
            1000.0 / ms,
            baseline / ms,
        );
    }
    println!("  (rung 0 is what a paused frame always gets, whatever playback is doing)");
}

/// One render size and quality to be measured.
struct Case {
    label: String,
    size: (u32, u32),
    quality: u8,
}

impl Case {
    fn new(label: &str, size: (u32, u32), quality: u8) -> Self {
        Self {
            label: label.to_string(),
            size,
            quality,
        }
    }
}

/// Time every case, interleaved, and keep the best round of each.
///
/// **Interleaved and not one after another**, for the reason
/// `benches/harness.rs` interleaves: a machine that gets busier during the run
/// would otherwise charge the whole drift to whichever case ran last, and the
/// answer would depend on the order the cases are listed in. Each case keeps
/// its own compositor and decoder so that every one of them walks the clip
/// forwards, which is the access pattern the decoder is built for; the best
/// round is reported because the minimum is the one statistic a background
/// compile cannot inflate.
fn measure(ctx: &Arc<RenderContext>, project: &Project, cases: Vec<Case>) -> Vec<(Case, f64)> {
    struct Arm {
        compositor: Compositor,
        sources: MediaSourceProvider,
        size: (u32, u32),
        best: f64,
    }

    let mut arms: Vec<Arm> = cases
        .iter()
        .map(|case| Arm {
            compositor: Compositor::new(Arc::clone(ctx)),
            sources: MediaSourceProvider::from_project(project),
            size: ctx.clamp_size(case.size),
            best: f64::MAX,
        })
        .collect();

    // Untimed: opens each decoder, allocates each render target and, on the
    // hardware path, the whole VAAPI JPEG surface pool for that size.
    for arm in &mut arms {
        let _ = arm
            .compositor
            .render_frame(project, 0, arm.size, &arm.sources);
    }

    for round in 0..ROUNDS {
        for (arm, case) in arms.iter_mut().zip(cases.iter()) {
            let started = Instant::now();
            for n in 0..FRAMES {
                // Distinct instants walking forward. One repeated instant would
                // measure the provider's texture cache instead of a frame.
                let at = ((round * FRAMES + n) as Micros) * 33_333 % 2_900_000;
                let Ok(rgba) = arm
                    .compositor
                    .render_frame(project, at, arm.size, &arm.sources)
                else {
                    arm.best = f64::NAN;
                    continue;
                };
                if encode_preview_jpeg(&rgba, arm.size.0, arm.size.1, case.quality).is_err() {
                    arm.best = f64::NAN;
                }
            }
            let ms = started.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
            arm.best = arm.best.min(ms);
        }
    }

    cases
        .into_iter()
        .zip(arms.into_iter().map(|arm| arm.best))
        .collect()
}

// ---------------------------------------------------------------------------
// 3. Encodes spent on frames that were already too late
// ---------------------------------------------------------------------------

/// Playback on a clock that runs faster than the renderer can keep up with.
///
/// The clock is injected and advanced by hand, so "the renderer is behind" is
/// an exact statement rather than a hope about machine load: every step moves
/// the playhead by more frames than the renderer can have produced.
fn late_encodes(project: &Project) {
    heading("3. JPEGs spent on frames the playhead had already passed");

    // Two regimes, because they answer different questions. The first is the
    // one the owner is actually in — a renderer a little slower than the rate
    // it is asked for — and the honest answer there may well be "this changes
    // nothing". The second is the pathological one, where it is the difference
    // between a busy encode thread and an idle one.
    for (label, frames_per_step, step_ms) in
        [("1.5x over budget", 1i64, 22u64), ("8x over budget", 3, 12)]
    {
        println!("  {label}:");
        for skip in [false, true] {
            experiment::set_skip_late_encodes(skip);
            let (counts, summary) = drive_behind(project, frames_per_step, step_ms);
            println!(
                "    skip_late_encodes={:<5} composited {:>3}  encoded {:>3}  \
                 discarded {:>3}  dropped {:>3}  shown {:>3}  encode thread busy {:>6.1} ms",
                skip,
                counts.composites,
                counts.encodes,
                summary.discarded,
                summary.dropped,
                summary.shown,
                counts.encode_ns as f64 / 1e6,
            );
        }
    }
    experiment::set_skip_late_encodes(true);
    println!(
        "  (`discarded` is the new counter: composited, then thrown away without a JPEG. \
         It is deliberately not folded into `dropped`, which counts frames never started.\n\
         \x20  Two in a row are never discarded, which is why `encoded` never falls to zero.)"
    );
}

/// Run playback with the clock deliberately outrunning the renderer, and hand
/// back what the probe and the stats saw.
fn drive_behind(project: &Project, frames_per_step: i64, step_ms: u64) -> (Counts, Summary) {
    let time = Arc::new(ManualSource::new());
    let server = PreviewServer::with_time_source(DEFAULT_CAPACITY, Arc::clone(&time) as _);
    server.set_source_provider(Arc::new(MediaSourceProvider::from_project(project)));

    let info = server.start(
        Arc::new(project.clone()),
        PreviewOptions::default(),
        0,
        channel(),
    );
    // Let the first frame land, so the decoder is warm and the measurement is
    // about pacing rather than about opening a file.
    let _ = server
        .cache()
        .wait(info.session, 0, Duration::from_secs(10));

    PROBE.reset();
    server.play().expect("a session is open");

    // Twenty steps of `frames_per_step` timeline frames each, `step_ms` apart.
    // A step shorter than one frame takes to composite is a renderer that
    // cannot be anything but behind, and by exactly how much is arithmetic
    // rather than luck, because the clock only moves when this loop says so.
    for step in 1..=20i64 {
        time.set(frame_time(step * frames_per_step, 30.0));
        std::thread::sleep(Duration::from_millis(step_ms));
    }
    // Drained before the pause, because pausing emits the last summary and
    // resets the window on its way out.
    let counts = PROBE.snapshot();
    let summary = server
        .stats()
        .finish(Instant::now())
        .expect("playback rendered something");
    server.pause().expect("a session is open");
    server.stop();
    (counts, summary)
}

// ---------------------------------------------------------------------------
// 4. The same frame rendered twice
// ---------------------------------------------------------------------------

/// Does a frame request that arrives while its frame is being rendered start a
/// second render of it?
///
/// The webview asks again whenever the handler answers a miss, and it answers a
/// miss after 60 ms — which a cold seek exceeds. Both arms are printed even if
/// the answer is "it never happened", because a negative result is worth more
/// than a cache nobody needs.
fn duplicate_renders(project: &Project) {
    heading("4. the same frame composited more than once");

    for dedupe in [false, true] {
        experiment::set_dedupe_scrubs(dedupe);
        let jobs = ask_for_one_frame_repeatedly(project);
        println!("  dedupe_scrubs={dedupe:<5}  scrub renders of one frame: {jobs}");
    }
    experiment::set_dedupe_scrubs(true);
}

fn ask_for_one_frame_repeatedly(project: &Project) -> u64 {
    let server = PreviewServer::with_capacity(DEFAULT_CAPACITY);
    server.set_source_provider(Arc::new(MediaSourceProvider::from_project(project)));
    let info = server.start(
        Arc::new(project.clone()),
        PreviewOptions::default(),
        0,
        channel(),
    );
    // Warm, then seek somewhere the decoder has to work for, so the render is
    // long enough for a second request to arrive during it. The probe is reset
    // *before* the seek, so the seek's own render is in the count: one is the
    // right answer, and anything above it is a frame rendered twice.
    let _ = server
        .cache()
        .wait(info.session, 0, Duration::from_secs(10));
    PROBE.reset();
    let info = server.seek(2_500_000).expect("a session is open");

    let url = format!("{}/{}", info.frame_url, info.frame);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let server = &server;
            let url = url.clone();
            scope.spawn(move || {
                let _ = server.serve_uri(&url);
            });
        }
    });
    // A duplicate render is queued *behind* the one in flight, so it has not
    // happened yet when the last request returns. Wait for the frame, then give
    // any second render of it time to finish and be counted.
    let _ = server
        .cache()
        .wait(info.session, info.frame, Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(400));
    let jobs = PROBE.snapshot().scrub_jobs;
    server.stop();
    jobs
}

// ---------------------------------------------------------------------------
// Fixtures and plumbing
// ---------------------------------------------------------------------------

fn channel() -> tauri::ipc::Channel<PreviewEvent> {
    tauri::ipc::Channel::new(|_| Ok(()))
}

fn heading(text: &str) {
    println!("\n{text}\n{}", "-".repeat(text.len()));
}

/// One clip filling the canvas: the shape of a preview frame in the common
/// case, and the cheapest one.
fn single_clip(path: &Path) -> Project {
    const SPAN: Micros = 3_000_000;
    let mut project = Project::new(
        "preview waste",
        CanvasConfig {
            width: 1920,
            height: 1080,
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
/// The bitrate is not a detail: `benches/fixtures.rs` records that a
/// near-incompressible source reversed a conclusion in `docs/STATUS.md`,
/// because noise buries a software decoder while the fixed-function one barely
/// notices. Light noise over `testsrc2`, half-second GOPs, same as the suite.
fn ensure_clip() -> Result<PathBuf, String> {
    let dir = target_dir().join("preview-waste");
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
    std::fs::rename(&staging, &out).map_err(|e| format!("cannot publish the clip: {e}"))?;
    Ok(out)
}

fn target_dir() -> PathBuf {
    // The example runs from `src-tauri`, and `CARGO_MANIFEST_DIR` is stable
    // wherever it is invoked from.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
}
