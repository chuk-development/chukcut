//! Two groups: what a preview frame costs to *encode*, and what a whole preview
//! frame costs.
//!
//! The second one is the number that decides whether playback is smooth, and it
//! is the reason this suite exists. Every other figure in the repository is a
//! stage; this is the sum, measured through the same calls
//! `preview::server::render_one` makes, and reported against the frame budget
//! at 24, 30 and 60 fps.
//!
//! ## What "whole frame" includes and what it does not
//!
//! Included: decoding each visible source, uploading it, compositing, reading
//! the result back to system memory, and JPEG-encoding it. That is everything
//! between "the playhead moved" and "there are bytes to hand the webview".
//!
//! Not included: the ring buffer, the pacing clock, the custom-protocol
//! response, and the webview's own decode and paint. Those are real, and two of
//! them have caused bugs (`docs/STATUS.md`, "Never treat a webview frame request
//! as a seek"), but none of them is measurable without a running app and a
//! GUI — which this binary is required not to need. **So the number here is a
//! floor on the frame cost, not the whole of it.**
//!
//! ## Why serial, when the app overlaps
//!
//! `preview/server.rs` runs the JPEG encode on a thread that overlaps the next
//! frame's compositing, so the app's *throughput* is better than the sum
//! measured here. The sum is still the right thing to report: it is the latency
//! of one frame, it is what a seek pays with nothing to overlap, and it is the
//! only version of the number that decomposes.

use std::sync::Arc;

use chukcut_lib::modules::media::MediaSourceProvider;
use chukcut_lib::modules::preview::encoder::{
    encode_jpeg, encode_preview_jpeg, hardware_available,
};
use chukcut_lib::modules::preview::vaapi::VaapiJpegEncoder;
use chukcut_lib::modules::preview::DEFAULT_JPEG_QUALITY;
use chukcut_lib::modules::project::{
    new_id, CanvasConfig, Micros, Project, Segment, TimeRange, Track, TrackKind, Transform,
    VideoMaterial,
};
use chukcut_lib::modules::render::{Compositor, RenderContext};

use crate::fixtures::Fixtures;
use crate::harness::{interleave, rounds, Measurement};

pub const ENCODE_GROUP: &str = "preview-encode";
pub const FRAME_GROUP: &str = "preview-frame";

/// The sizes the encode group covers.
///
/// 540×960 is the old reduced playback proxy, kept because it is the size the
/// preview falls back to on a machine where the native frame does not fit the
/// budget. The other two are the two orientations at native resolution.
const SIZES: [(u32, u32); 3] = [(540, 960), (1080, 1920), (1920, 1080)];

/// A frame with the kind of detail real footage has.
///
/// A smooth gradient would flatter every entropy coder equally and produce
/// numbers several times better than anything the editor will ever see.
fn synthetic(width: u32, height: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let noise = (x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) as u8;
            data.extend_from_slice(&[
                ((x * 255 / width.max(1)) as u8).wrapping_add(noise / 4),
                ((y * 255 / height.max(1)) as u8).wrapping_add(noise / 8),
                noise,
                255,
            ]);
        }
    }
    data
}

pub struct Budget {
    pub runs: usize,
    pub frames: usize,
    pub rounds: usize,
}

// ---------------------------------------------------------------------------
// Group 3: the encoders
// ---------------------------------------------------------------------------

pub fn run_encode(budget: &Budget) -> Vec<Measurement> {
    let mut out = Vec::new();
    let quality = DEFAULT_JPEG_QUALITY;
    let hardware = hardware_available();

    for (width, height) in SIZES {
        let rgba = synthetic(width, height);
        let size = format!("{width}x{height}");

        let mut portable = || {
            let mut buffer = Vec::with_capacity(rgba.len() / 8);
            jpeg_encoder::Encoder::new(&mut buffer, quality)
                .encode(
                    &rgba,
                    width as u16,
                    height as u16,
                    jpeg_encoder::ColorType::Rgba,
                )
                .expect("jpeg-encoder");
        };
        let mut turbo = || {
            encode_jpeg(&rgba, width, height, quality).expect("libjpeg-turbo");
        };

        if !hardware {
            let samples = interleave(budget.runs, &mut [&mut portable, &mut turbo]);
            out.push(Measurement::ms(
                ENCODE_GROUP,
                format!("{size} jpeg-encoder"),
                samples[0].clone(),
            ));
            out.push(Measurement::ms(
                ENCODE_GROUP,
                format!("{size} libjpeg-turbo"),
                samples[1].clone(),
            ));
            out.push(Measurement::skip(
                ENCODE_GROUP,
                format!("{size} VAAPI"),
                "no hardware JPEG encoder on this machine",
            ));
            continue;
        }

        // Built once and reused, which is what the server does too: rebuilding
        // it per frame would be measuring `avcodec_open2`.
        let mut encoder = match VaapiJpegEncoder::open(width, height, quality) {
            Ok(encoder) => encoder,
            Err(error) => {
                let samples = interleave(budget.runs, &mut [&mut portable, &mut turbo]);
                out.push(Measurement::ms(
                    ENCODE_GROUP,
                    format!("{size} jpeg-encoder"),
                    samples[0].clone(),
                ));
                out.push(Measurement::ms(
                    ENCODE_GROUP,
                    format!("{size} libjpeg-turbo"),
                    samples[1].clone(),
                ));
                out.push(Measurement::skip(
                    ENCODE_GROUP,
                    format!("{size} VAAPI"),
                    format!("the driver refused a {size} JPEG encoder: {error}"),
                ));
                continue;
            }
        };
        let samples = {
            let mut vaapi = || {
                encoder.encode(&rgba).expect("vaapi encode");
            };
            interleave(budget.runs, &mut [&mut portable, &mut turbo, &mut vaapi])
        };
        let stages = encoder.stages();

        out.push(Measurement::ms(
            ENCODE_GROUP,
            format!("{size} jpeg-encoder"),
            samples[0].clone(),
        ));
        out.push(Measurement::ms(
            ENCODE_GROUP,
            format!("{size} libjpeg-turbo"),
            samples[1].clone(),
        ));
        out.push(
            Measurement::ms(ENCODE_GROUP, format!("{size} VAAPI"), samples[2].clone()).with_note(
                // The breakdown is the useful part: the fixed-function encoder
                // is a fraction of its own path, and the rest is carrying
                // pixels to a chip that already had them.
                format!(
                    "convert {:.2} + upload {:.2} + encode {:.2} ms",
                    stages.convert_micros as f64 / 1000.0,
                    stages.upload_micros as f64 / 1000.0,
                    stages.encode_micros as f64 / 1000.0
                ),
            ),
        );
    }

    // What the server actually pays: dispatch, the fallback check and the lock.
    let (width, height) = (1080u32, 1920);
    let rgba = synthetic(width, height);
    let mut backend = None;
    let samples = {
        let mut through = || {
            let (_, used) =
                encode_preview_jpeg(&rgba, width, height, DEFAULT_JPEG_QUALITY).expect("encode");
            backend = Some(used);
        };
        interleave(budget.runs, &mut [&mut through])
    };
    out.push(
        Measurement::ms(
            ENCODE_GROUP,
            format!("{width}x{height} through encode_preview_jpeg"),
            samples[0].clone(),
        )
        .with_note(format!(
            "dispatched to {}",
            backend.map(|b| b.label()).unwrap_or("nothing")
        )),
    );

    out
}

// ---------------------------------------------------------------------------
// Group 4: the whole frame
// ---------------------------------------------------------------------------

/// One clip filling the canvas, which is the shape of a preview frame in the
/// common case and the cheapest one — a multi-layer timeline costs what the
/// composite group says on top of this.
fn single_clip(fixture: &crate::fixtures::Fixture) -> Project {
    let mut project = Project::new(
        "preview bench",
        CanvasConfig {
            width: fixture.width & !1,
            height: fixture.height & !1,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    const SPAN: Micros = 3_000_000;
    let material_id = new_id();
    project.materials.videos.push(VideoMaterial {
        id: material_id.clone(),
        path: fixture.path.to_string_lossy().into_owned(),
        width: fixture.width,
        height: fixture.height,
        duration: SPAN,
        fps: 30.0,
        has_audio: true,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "Video 1");
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

pub fn run_frame(ctx: &Arc<RenderContext>, media: &Fixtures, budget: &Budget) -> Vec<Measurement> {
    let mut out = Vec::new();
    let compositor = Compositor::new(Arc::clone(ctx));

    for size in crate::fixtures::SIZES {
        let label = format!("{}x{}", size.0, size.1);
        let Some(fixture) = media.workhorse(size) else {
            out.push(Measurement::skip(
                FRAME_GROUP,
                format!("{label} whole frame"),
                "no fixture at this size; ffmpeg could not generate one",
            ));
            continue;
        };
        let project = single_clip(fixture);
        let render_size = (project.canvas.width, project.canvas.height);
        let sources = MediaSourceProvider::from_project(&project);

        // One untimed frame: it opens the decoder, seeks to the first keyframe,
        // allocates the render target and, on the hardware path, the whole
        // VAAPI JPEG surface pool.
        let warm = compositor.render_frame(&project, 0, render_size, &sources);
        if let Err(error) = warm {
            out.push(Measurement::skip(
                FRAME_GROUP,
                format!("{label} whole frame"),
                format!("the first frame did not render: {error}"),
            ));
            continue;
        }
        compositor.reset_stats();

        let mut backend = None;
        let mut jpeg_bytes = 0usize;
        let result = rounds::<String>(budget.rounds, |round| {
            let started = std::time::Instant::now();
            for n in 0..budget.frames {
                // Distinct instants, walking forward at the timeline rate:
                // this is playback, and it is the access pattern the decoder's
                // forward-decode window is tuned for. Re-rendering one instant
                // would measure the provider's texture cache instead.
                let at = ((round * budget.frames + n) as Micros) * 33_333 % 2_900_000;
                let rgba = compositor
                    .render_frame(&project, at, render_size, &sources)
                    .map_err(|e| e.to_string())?;
                let (bytes, used) =
                    encode_preview_jpeg(&rgba, render_size.0, render_size.1, DEFAULT_JPEG_QUALITY)
                        .map_err(|e| e.to_string())?;
                backend = Some(used);
                jpeg_bytes = bytes.len();
            }
            Ok(started.elapsed().as_secs_f64() * 1000.0 / budget.frames.max(1) as f64)
        });

        match result {
            Ok(samples) => {
                let stats = compositor.stats();
                let median = {
                    let mut sorted = samples.clone();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    sorted[sorted.len() / 2]
                };
                out.push(
                    Measurement::ms(FRAME_GROUP, format!("{label} whole frame"), samples)
                        .with_note(format!(
                        "sources {:.2} + composite {:.2} + readback {:.2} ms, JPEG on {} ({} KB)",
                        stats.per_frame(stats.sources_ns) / 1e6,
                        stats.per_frame(stats.composite_ns) / 1e6,
                        stats.per_frame(stats.readback_ns) / 1e6,
                        backend.map(|b| b.label()).unwrap_or("nothing"),
                        jpeg_bytes / 1024,
                    )),
                );
                out.push(budget_row(&label, median));
                compositor.reset_stats();
            }
            Err(error) => out.push(Measurement::skip(
                FRAME_GROUP,
                format!("{label} whole frame"),
                error,
            )),
        }

        // The same frame again, composited straight into a VA surface the media
        // driver allocated, so the readback, the CPU RGBA→NV12 pass and the
        // upload — 60% of the row above — do not happen at all. Immediately
        // after the copying arm and in the same process on purpose: a "before"
        // taken from a build that no longer exists is not a measurement.
        out.extend(drawn_frame(
            ctx,
            &compositor,
            &project,
            render_size,
            &label,
            budget,
        ));
    }

    out
}

/// The whole-frame row for the zero-copy path the preview takes when the
/// hardware allows it.
///
/// Skipped with a reason rather than omitted where it does not run, because
/// "this machine cannot" is the interesting half of the finding — see
/// `docs/research/preview-zerocopy-jpeg.md`.
fn drawn_frame(
    ctx: &Arc<RenderContext>,
    compositor: &Compositor,
    project: &Project,
    size: (u32, u32),
    label: &str,
    budget: &Budget,
) -> Vec<Measurement> {
    use chukcut_lib::modules::preview::encoder::encode_preview_jpeg_va_surface;
    use chukcut_lib::modules::preview::vasurface::{self, SurfaceRing};
    use chukcut_lib::modules::render::source::YuvRange;

    let name = format!("{label} whole frame, drawn into the encoder's surface");
    if !vasurface::encoder_reads_its_own_surface(ctx) {
        return vec![Measurement::skip(
            FRAME_GROUP,
            name,
            "this machine's JPEG encoder does not read a surface the compositor drew into",
        )];
    }
    let Some(ring) = SurfaceRing::new(ctx, size).map(Arc::new) else {
        return vec![Measurement::skip(
            FRAME_GROUP,
            name,
            "this device cannot draw into a VAAPI-allocated NV12 surface",
        )];
    };
    // A fresh provider, so this arm does not inherit the copying arm's warm
    // texture cache.
    let sources = MediaSourceProvider::from_project(project);

    let one = |at: Micros| -> Result<usize, String> {
        let mut claim = ring.claim().ok_or("no free surface")?;
        compositor
            .render_nv12_into_planes(
                project,
                at,
                size,
                &sources,
                ring.luma(claim.index()),
                ring.chroma(claim.index()),
                // Full range: a JPEG carries no range tag. See `shaders/yuv.wgsl`.
                YuvRange::Full,
            )
            .map_err(|e| e.to_string())?;
        let bytes = encode_preview_jpeg_va_surface(claim.surface_mut(), DEFAULT_JPEG_QUALITY)
            .ok_or("the hardware JPEG encoder refused the surface")?;
        claim.release();
        Ok(bytes.len())
    };

    if let Err(error) = one(0) {
        return vec![Measurement::skip(FRAME_GROUP, name, error)];
    }
    compositor.reset_stats();

    let mut jpeg_bytes = 0usize;
    let result = rounds::<String>(budget.rounds, |round| {
        let started = std::time::Instant::now();
        for n in 0..budget.frames {
            let at = ((round * budget.frames + n) as Micros) * 33_333 % 2_900_000;
            jpeg_bytes = one(at)?;
        }
        Ok(started.elapsed().as_secs_f64() * 1000.0 / budget.frames.max(1) as f64)
    });

    match result {
        Ok(samples) => {
            let stats = compositor.stats();
            let median = {
                let mut sorted = samples.clone();
                sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
                sorted[sorted.len() / 2]
            };
            let out = vec![
                Measurement::ms(FRAME_GROUP, name, samples).with_note(format!(
                    "sources {:.2} + composite {:.2} + NV12 into the surface {:.2} ms, no readback ({} KB)",
                    stats.per_frame(stats.sources_ns) / 1e6,
                    stats.per_frame(stats.composite_ns) / 1e6,
                    stats.per_frame(stats.nv12_ns) / 1e6,
                    jpeg_bytes / 1024,
                )),
                budget_row(&format!("{label} drawn"), median),
            ];
            compositor.reset_stats();
            out
        }
        Err(error) => vec![Measurement::skip(FRAME_GROUP, name, error)],
    }
}

/// The frame cost expressed as a fraction of the budget at each rate.
///
/// Reported as its own row rather than as prose because it is the thing a
/// reader actually wants: 13.7 ms means nothing until it is next to 33.3.
fn budget_row(label: &str, median_ms: f64) -> Measurement {
    let text = [24.0f64, 30.0, 60.0]
        .iter()
        .map(|fps| {
            let budget = 1000.0 / fps;
            format!(
                "{fps:.0} fps: {:.0}% of {budget:.1} ms{}",
                median_ms / budget * 100.0,
                if median_ms > budget { " OVER" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Measurement::new(
        FRAME_GROUP,
        format!("{label} headroom"),
        "fps",
        crate::harness::Direction::Higher,
        vec![1000.0 / median_ms.max(f64::EPSILON)],
    )
    .with_note(text)
}
