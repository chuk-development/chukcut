//! What decoding a frame actually costs, software against hardware.
//!
//! `docs/STATUS.md` says decode is one of the two largest costs in both preview
//! and export, and that number came from a `tracing` line inside the provider.
//! This is the tool that measures it on its own, on real files, so the claim
//! survives without an app running.
//!
//! ```bash
//! cargo run --release --example decode_bench -- --dir target/bench-media
//! cargo run --release --example decode_bench -- clip.mp4 --frames 200
//! ```
//!
//! Reported as **medians**, not means, and over a sequential walk rather than a
//! seek storm. Both choices matter:
//!
//! - Run-to-run spread on this machine is wide enough that a mean is decided by
//!   its worst sample; `docs/STATUS.md` records software export varying between
//!   11.3 and 16.9 fps across one session.
//! - A sequential walk is what playback and export both do. Timing a seek per
//!   frame measures the demuxer, not the decoder.
//!
//! The first frame is discarded from every measurement. It carries the codec
//! context's lazy initialisation, the first keyframe, and on the hardware path
//! the whole VAAPI surface pool allocation, and including it makes short runs
//! look two to three times worse than they are.

use std::path::{Path, PathBuf};
use std::time::Instant;

use chukcut_lib::modules::media::decoder::Acceleration;
use chukcut_lib::modules::media::{hwdecode, VideoDecoder};

fn main() {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut frames = 120usize;
    let mut rounds = 3usize;
    let mut dir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(frames),
            "--rounds" => rounds = args.next().and_then(|v| v.parse().ok()).unwrap_or(rounds),
            "--dir" => dir = args.next().map(PathBuf::from),
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
            "usage: decode_bench [--dir DIR] [FILE...] [--frames N] [--rounds N]\n\
             \n\
             Nothing to measure. Point it at a directory of clips or name them."
        );
        std::process::exit(2);
    }

    report_capabilities();

    println!("\n## Decode cost per frame, median of {rounds} runs of {frames} sequential frames\n");
    println!(
        "The three columns are the three things a caller can ask for, and the gaps between \
         them are where the time goes: `→DMA-BUF` is the decode alone, `hw →RGBA` adds the \
         download out of GPU memory and the swscale pass, `sw →RGBA` is both on the CPU.\n"
    );
    println!(
        "| file | codec | size | sw →RGBA | hw →RGBA | hw →DMA-BUF | dmabuf vs sw | on the GPU |"
    );
    println!("|---|---|---|---:|---:|---:|---:|---|");

    for file in &files {
        match measure(file, frames, rounds) {
            Ok(row) => println!("{row}"),
            Err(error) => println!("| {} | — | — | — | — | — | {error} |", name(file)),
        }
    }

    println!("\n## DMA-BUF export of a decoded surface\n");
    for file in &files {
        describe_dmabuf(file);
    }
}

fn report_capabilities() {
    println!("## What this machine hardware-decodes\n");
    println!("| codec | in build | declares VAAPI | decodes a real frame | note |");
    println!("|---|---|---|---|---|");
    for support in hwdecode::capabilities() {
        println!(
            "| {} | {} | {} | {} | {} |",
            support.codec.label(),
            yes(support.in_build),
            yes(support.declares_vaapi),
            yes(support.usable),
            support.note.as_deref().unwrap_or("")
        );
    }
}

fn yes(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn measure(file: &Path, frames: usize, rounds: usize) -> Result<String, String> {
    let info = chukcut_lib::modules::media::probe(file).map_err(|e| e.to_string())?;
    let video = info.video.ok_or_else(|| "no video stream".to_string())?;
    let step = (1_000_000.0 / video.fps.max(1.0)) as i64;

    let software = median(rounds, || {
        walk(file, Acceleration::Software, frames, step, Want::Rgba)
    })?;
    let hardware = median(rounds, || {
        walk(file, Acceleration::Vaapi, frames, step, Want::Rgba)
    });
    let mapped = median(rounds, || {
        walk(file, Acceleration::Vaapi, frames, step, Want::Dmabuf)
    });

    let (hardware_cell, note) = match &hardware {
        Ok(hardware) => (format!("{hardware:.2} ms"), {
            let mut decoder =
                VideoDecoder::open_with(file, Acceleration::Vaapi).map_err(|e| e.to_string())?;
            decoder.seek_and_decode(0).map_err(|e| e.to_string())?;
            if decoder.is_hardware() {
                "yes".to_string()
            } else {
                "no — fell back".to_string()
            }
        }),
        Err(error) => ("—".to_string(), error.clone()),
    };
    let (mapped_cell, speedup) = match &mapped {
        Ok(mapped) => (
            format!("{mapped:.2} ms"),
            format!("{:.1}×", software / mapped),
        ),
        Err(_) => ("—".to_string(), "—".to_string()),
    };

    Ok(format!(
        "| {} | {} | {}x{} | {software:.2} ms | {hardware_cell} | {mapped_cell} | {speedup} | {note} |",
        name(file),
        video.codec,
        video.width,
        video.height,
    ))
}

/// What the caller wants out of each decoded frame.
#[derive(Clone, Copy)]
enum Want {
    /// Tightly packed RGBA in system memory, which is what the compositor is
    /// fed today.
    Rgba,
    /// DMA-BUF handles onto the surface, which is what it should be fed.
    Dmabuf,
}

/// Milliseconds per frame over a sequential walk, first frame excluded.
fn walk(
    file: &Path,
    acceleration: Acceleration,
    frames: usize,
    step: i64,
    want: Want,
) -> Result<f64, String> {
    let mut decoder = VideoDecoder::open_with(file, acceleration).map_err(|e| e.to_string())?;
    let pull = |decoder: &mut VideoDecoder, at: i64| -> Result<(), String> {
        match want {
            Want::Rgba => decoder.seek_and_decode(at).map(drop),
            // The mapped frame is dropped immediately, which returns the
            // surface to the pool — the same thing a consumer does once its
            // texture is built, so the pool pressure is representative.
            Want::Dmabuf => decoder.seek_and_map(at).map(drop),
        }
        .map_err(|e| e.to_string())
    };

    // The first frame pays for the keyframe, the codec context and — on the
    // hardware path — the surface pool. It is real cost, but it is paid once
    // per file and not once per frame, so averaging it in describes nothing.
    pull(&mut decoder, step / 2)?;

    let started = Instant::now();
    for n in 1..frames {
        pull(&mut decoder, n as i64 * step + step / 2)?;
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0 / (frames.saturating_sub(1)).max(1) as f64)
}

fn median<E>(rounds: usize, mut run: impl FnMut() -> Result<f64, E>) -> Result<f64, E> {
    let mut samples = Vec::with_capacity(rounds);
    for _ in 0..rounds.max(1) {
        samples.push(run()?);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));
    Ok(samples[samples.len() / 2])
}

/// Print exactly what a VA surface looks like once exported, because the shape
/// is the whole argument about which zero-copy route to take.
fn describe_dmabuf(file: &Path) {
    let mut decoder = match VideoDecoder::open_with(file, Acceleration::Vaapi) {
        Ok(decoder) => decoder,
        Err(error) => {
            println!("- **{}**: {error}", name(file));
            return;
        }
    };

    let started = Instant::now();
    let mapped = match decoder.seek_and_map(500_000) {
        Ok(mapped) => mapped,
        Err(error) => {
            println!("- **{}**: {error}", name(file));
            return;
        }
    };
    let first_map = started.elapsed().as_secs_f64() * 1000.0;

    // The first map includes the seek. Time a few more from frames the decoder
    // already has in hand, which is what a playing timeline does.
    let mut repeats = Vec::new();
    for n in 1..6 {
        let started = Instant::now();
        if decoder.seek_and_map(500_000 + n * 33_333).is_ok() {
            repeats.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    repeats.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let steady = repeats.get(repeats.len() / 2).copied().unwrap_or(f64::NAN);

    let planes = mapped.dmabuf.planes();
    println!(
        "- **{}**: {} object(s), {} plane(s), first map {first_map:.2} ms, \
         steady {steady:.2} ms/frame (decode included), importable: {}",
        name(file),
        mapped.dmabuf.objects(),
        planes.len(),
        mapped.dmabuf.is_importable()
    );
    for (index, plane) in planes.iter().enumerate() {
        println!(
            "  - plane {index}: fourcc `{}` {}x{}, modifier `{:#018x}`, offset {}, pitch {}, object {} bytes",
            plane.fourcc_name(),
            plane.width,
            plane.height,
            plane.modifier,
            plane.offset,
            plane.pitch,
            plane.object_size
        );
    }
}
