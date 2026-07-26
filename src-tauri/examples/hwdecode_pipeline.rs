//! What a decoded frame costs once it has to become a *texture*, and whether
//! the fast way to get there produces the same picture.
//!
//! `decode_bench` measures the decoder on its own and stops at the DMA-BUF
//! descriptors. This measures what the editor actually does: ask
//! `media::MediaSourceProvider` for a frame, get a `SourceFrame` back, and
//! composite it. That is where the hardware-decode exercise either pays off or
//! does not, because the provider is what chooses between copying the surface
//! through system memory and importing it as two textures.
//!
//! ```bash
//! cargo run --release --example hwdecode_pipeline -- --dir target/bench-media
//! cargo run --release --example hwdecode_pipeline -- --dir target/bench-media --verify
//! ```
//!
//! **Both decoders are measured in one process, back to back, per file.**
//! `docs/STATUS.md` keeps having to point out that run-to-run spread on this
//! machine is wider than most of the effects being measured, so two separate
//! runs of a benchmark are not comparable and interleaved ones are. That is
//! what `MediaSourceProvider::from_project_with` exists for; the application
//! itself never forces a decoder.
//!
//! Medians of a sequential walk, first frame discarded — the same discipline
//! `decode_bench` documents and for the same reasons. The first frame carries
//! the file open, the seek, and on the hardware path the whole VA surface pool.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use chukcut_lib::modules::media::decoder::Acceleration;
use chukcut_lib::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_lib::modules::project::document::{
    CanvasConfig, MaterialKind, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_lib::modules::render::{
    Compositor, CompositorConfig, RenderContext, SourceFrame, SourceProvider, SourceRequest,
    YuvMatrix,
};

const MATERIAL: &str = "clip";
const SEGMENT: &str = "seg";

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut frames = 90usize;
    let mut rounds = 3usize;
    let mut verify = false;
    let mut dir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(frames),
            "--rounds" => rounds = args.next().and_then(|v| v.parse().ok()).unwrap_or(rounds),
            "--dir" => dir = args.next().map(PathBuf::from),
            "--verify" => verify = true,
            other => files.push(PathBuf::from(other)),
        }
    }

    if let Some(dir) = &dir {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                !name.starts_with("probe_")
                    && !name.starts_with("bars_")
                    && matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("mp4" | "webm" | "mkv" | "mov")
                    )
            })
            .collect();
        found.sort();
        files.extend(found);
    }

    if files.is_empty() {
        eprintln!(
            "usage: hwdecode_pipeline [--dir DIR] [FILE...] [--frames N] [--rounds N] [--verify]"
        );
        std::process::exit(2);
    }

    let Some(ctx) = RenderContext::try_new().map(Arc::new) else {
        eprintln!("no GPU adapter; nothing here can run");
        std::process::exit(1);
    };
    println!(
        "device: {} ({:?}), can import DMA-BUF: {}",
        ctx.adapter_info().name,
        ctx.adapter_info().backend,
        ctx.can_import_dmabuf()
    );

    if verify {
        println!("\n## Does an imported surface composite to the same picture?\n");
        for file in &files {
            verify_pixels(&ctx, file);
        }
        return;
    }

    println!("\n## Decode to texture, and a whole composited frame\n");
    println!(
        "`to texture` is one `SourceProvider::frame` call: the decode plus whatever it takes to \
         reach something the compositor can bind — an upload on the software path, an import on \
         the hardware one. `whole frame` is that plus compositing and the readback, which is \
         everything the preview does short of the JPEG encode.\n"
    );
    println!("| file | size | decoder | to texture | whole frame | composite | readback |");
    println!("|---|---|---|---:|---:|---:|---:|");
    for file in &files {
        for forced in [Acceleration::Software, Acceleration::Vaapi] {
            match measure(&ctx, file, forced, frames, rounds) {
                Ok(row) => println!("{row}"),
                Err(error) => println!(
                    "| {} | — | {} | — | — | — | {error} |",
                    name(file),
                    label(forced)
                ),
            }
        }
    }
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn label(acceleration: Acceleration) -> &'static str {
    match acceleration {
        Acceleration::Software => "software → RGBA",
        _ => "hardware → imported",
    }
}

/// A one-clip project at the clip's own resolution.
///
/// The canvas matches the source so nothing is scaled: a fit that shrank the
/// picture would measure the sampler rather than the decode, and would make the
/// pixel comparison meaningless.
fn one_clip_project(path: &Path, width: u32, height: u32, fps: f64, duration: i64) -> Project {
    let mut project = Project::new(
        "bench",
        CanvasConfig {
            width,
            height,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        fps,
    );
    project.materials.videos.push(VideoMaterial {
        id: MATERIAL.into(),
        path: path.to_string_lossy().to_string(),
        width,
        height,
        duration,
        fps,
        has_audio: false,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(Segment {
        id: SEGMENT.into(),
        material_id: MATERIAL.into(),
        target_range: TimeRange::new(0, duration),
        source_range: TimeRange::new(0, duration),
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

fn request_at<'a>(at: i64, size: (u32, u32)) -> SourceRequest<'a> {
    SourceRequest {
        material_id: MATERIAL,
        kind: MaterialKind::Video,
        source_time: at,
        segment_id: SEGMENT,
        max_size: size,
    }
}

/// Coded size, frame rate and duration.
fn describe(path: &Path) -> Result<(u32, u32, f64, i64), String> {
    let probe = chukcut_lib::modules::media::probe(path).map_err(|e| e.to_string())?;
    let video = probe.video.ok_or_else(|| "no video stream".to_string())?;
    let fps = if video.fps > 1.0 { video.fps } else { 30.0 };
    Ok((
        video.width,
        video.height,
        fps,
        probe.duration.max(1_000_000),
    ))
}

fn measure(
    ctx: &Arc<RenderContext>,
    file: &Path,
    forced: Acceleration,
    frames: usize,
    rounds: usize,
) -> Result<String, String> {
    let (width, height, fps, duration) = describe(file)?;
    let size = (width, height);
    let project = one_clip_project(file, width, height, fps, duration);
    let step = ((1_000_000.0 / fps) as i64).max(1);

    let compositor = Compositor::with_config(Arc::clone(ctx), CompositorConfig::default());
    let provider = MediaSourceProvider::from_project_with(&project, Some(forced));

    let mut texture_runs = Vec::new();
    let mut whole_runs = Vec::new();
    let mut imported = false;

    for round in 0..rounds {
        provider.clear();
        if round == 1 {
            // Round zero warms the driver's shader cache and the texture pool.
            compositor.reset_stats();
        }

        let mut samples = Vec::with_capacity(frames);
        for i in 0..frames {
            let started = Instant::now();
            let frame = provider
                .frame(ctx, &request_at(i as i64 * step, size))
                .map_err(|e| e.to_string())?;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            // Checked on every frame, not the first: `VideoDecoder::is_hardware`
            // only becomes true once a frame has actually come back as a
            // surface, so frame zero of a hardware decode still goes the
            // copying way. That is one frame per file and it is the one being
            // discarded anyway.
            imported |= frame.as_ref().is_some_and(SourceFrame::is_planar);
            if i == 0 {
                continue;
            }
            samples.push(elapsed);
        }
        texture_runs.push(median(&mut samples));

        provider.clear();
        let mut samples = Vec::with_capacity(frames);
        for i in 0..frames {
            let started = Instant::now();
            compositor
                .render(&project, i as i64 * step, size, &provider)
                .map_err(|e| e.to_string())?;
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            if i == 0 {
                continue;
            }
            samples.push(elapsed);
        }
        whole_runs.push(median(&mut samples));
    }

    let stats = compositor.stats();
    // Report what happened, not what was asked for. A hardware request that
    // quietly fell back to copying would otherwise be recorded as a hardware
    // measurement, which is how a benchmark comes to prove the opposite of what
    // it says.
    let decoder = if forced == Acceleration::Software {
        label(forced).to_string()
    } else if imported {
        format!("**{}**", label(forced))
    } else {
        format!("{} (not imported!)", label(forced))
    };

    Ok(format!(
        "| {} | {}x{} | {} | {:.2} ms | {:.2} ms | {:.2} ms | {:.2} ms |",
        name(file),
        width,
        height,
        decoder,
        median(&mut texture_runs),
        median(&mut whole_runs),
        stats.per_frame(stats.composite_ns) / 1e6,
        stats.per_frame(stats.readback_ns) / 1e6,
    ))
}

fn median(values: &mut [f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// A provider that hands on somebody else's frames with the colour matrix
/// deliberately replaced.
///
/// This is the control this particular change needs. The interesting claim is
/// not "the import works" — `dmabuf_import.rs` established that — but "the
/// matrix came from the file". If forcing BT.601 onto a BT.709 file scored the
/// same as reading the file, the matrix would be being ignored and the
/// agreement would be luck.
struct ForcedMatrix<'a> {
    inner: &'a dyn SourceProvider,
    matrix: YuvMatrix,
}

impl SourceProvider for ForcedMatrix<'_> {
    fn frame(
        &self,
        ctx: &RenderContext,
        request: &SourceRequest<'_>,
    ) -> anyhow::Result<Option<SourceFrame>> {
        Ok(self.inner.frame(ctx, request)?.map(|mut frame| {
            frame.matrix = self.matrix;
            frame
        }))
    }
}

fn verify_pixels(ctx: &Arc<RenderContext>, file: &Path) {
    let Ok((width, height, fps, duration)) = describe(file) else {
        println!("- {}: cannot probe\n", name(file));
        return;
    };
    let size = (width, height);
    let project = one_clip_project(file, width, height, fps, duration);
    let compositor = Compositor::with_config(Arc::clone(ctx), CompositorConfig::default());
    let step = ((1_000_000.0 / fps) as i64).max(1);
    // Not the first frame: a keyframe at the head of the file is the easiest
    // frame in it, and the interesting question is about the ones between.
    let at = step * 30;

    // What the file says its chroma is, straight from the decoder. Printed
    // rather than assumed, because "the matrix came from the file" is half of
    // what this checks and a guessed BT.709 would look identical to a declared
    // one in the numbers below.
    let colour = VideoDecoder::open_with(file, Acceleration::Vaapi)
        .and_then(|mut decoder| decoder.seek_and_map(at))
        .map(|mapped| format!("{:?}, {:?}", mapped.color_space, mapped.color_range));
    let colour = match colour {
        Ok(colour) => colour,
        Err(error) => {
            println!("- {}: no hardware decode ({error})\n", name(file));
            return;
        }
    };

    let hardware = MediaSourceProvider::from_project_with(&project, Some(Acceleration::Vaapi));
    let software = MediaSourceProvider::from_project_with(&project, Some(Acceleration::Software));
    let render = |provider: &dyn SourceProvider, at: i64| {
        compositor
            .render(&project, at, size, provider)
            .ok()
    };

    let (Some(hw), Some(sw), Some(sw_next)) = (
        render(&hardware, at),
        render(&software, at),
        render(&software, at + step),
    ) else {
        println!("- {}: a render failed\n", name(file));
        return;
    };

    // Was it actually imported? A hardware decoder whose surface will not
    // import falls back to copying — silently and correctly — and then this
    // whole comparison is software against software and proves nothing.
    let planar = hardware
        .frame(ctx, &request_at(at, size))
        .ok()
        .flatten()
        .is_some_and(|frame| frame.is_planar());

    let forced = ForcedMatrix {
        inner: &hardware,
        matrix: YuvMatrix::Bt601,
    };
    let wrong_matrix = render(&forced, at);

    println!("### {}  ({width}x{height}, {colour})\n", name(file));
    if !planar {
        println!(
            "  **The provider did not import a surface for this file**, so nothing below is a \
             hardware measurement.\n"
        );
    }
    println!("| comparison | mean channel difference | worst |");
    println!("|---|---:|---:|");
    let (mean, worst) = compare(&hw.data, &sw.data);
    println!("| hardware vs software, same instant | **{mean:.2}** | {worst} |");
    if let Some(wrong) = wrong_matrix {
        let (mean, worst) = compare(&wrong.data, &sw.data);
        println!("| control: BT.601 forced on the same surface | {mean:.2} | {worst} |");
    }
    let (mean, worst) = compare(&sw_next.data, &sw.data);
    println!("| control: software, one frame apart | {mean:.2} | {worst} |");
    println!();
}

/// Mean and worst absolute difference over R, G and B.
///
/// Alpha is ignored: both paths write 255 and including it would dilute the
/// mean by a quarter, which would make a bad result look like a passing one.
fn compare(a: &[u8], b: &[u8]) -> (f64, u8) {
    let mut total = 0f64;
    let mut worst = 0u8;
    let mut count = 0usize;
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        for channel in 0..3 {
            let delta = pa[channel].abs_diff(pb[channel]);
            total += delta as f64;
            worst = worst.max(delta);
            count += 1;
        }
    }
    (total / count.max(1) as f64, worst)
}
