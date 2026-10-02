//! What one preview frame costs to encode, on each encoder this machine has.
//!
//! The preview serves every frame as a JPEG, so this number is a direct term
//! in the playback frame budget — 33.3 ms at 30 fps. It is the measurement
//! behind the resolution decisions in `preview/session.rs`, so it lives in the
//! repository rather than in a session transcript.
//!
//! ```text
//! cargo run --release --example preview_jpeg_bench
//! cargo run --release --example preview_jpeg_bench -- --runs 15
//! ```
//!
//! Reports the **median** of the runs, because run-to-run spread on this
//! machine is wide enough that a single measurement is worthless — see the
//! same warning in `docs/STATUS.md` about the export numbers.
//!
//! It also decodes both encoders' output for the same frame and reports the
//! PSNR between them. That is the check that matters: a hardware encoder that
//! is fast and produces a differently-coloured picture is not a win, and a
//! full-range/limited-range mismatch is the specific way this one goes wrong.

use std::time::Instant;

use chukcut_engine::modules::preview::encoder::{
    decode_jpeg_rgb, encode_jpeg, encode_preview_jpeg, hardware_available, psnr_rgb, Backend,
};
use chukcut_engine::modules::preview::vaapi::VaapiJpegEncoder;

/// A frame with the kind of detail real footage has.
///
/// A smooth gradient would flatter every entropy coder equally and produce
/// numbers three times better than anything the editor will ever see.
fn frame(width: u32, height: u32) -> Vec<u8> {
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

/// Milliseconds, as median and best-of.
///
/// Both, because they answer different questions and this machine makes the
/// difference large. The median is what a frame typically costs with whatever
/// else is running; the minimum is what the work itself costs, since every
/// source of noise here — thermal throttling, a rayon worker busy elsewhere,
/// the scheduler — only ever adds time. When the two are far apart, the
/// machine was busy and neither number should be quoted alone.
#[derive(Debug, Clone, Copy)]
struct Timing {
    median: f64,
    best: f64,
}

fn summarise(mut samples: Vec<f64>) -> Timing {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Timing {
        median: samples[samples.len() / 2],
        best: samples[0],
    }
}

/// Run every candidate once per round, in the same order, `runs` times.
///
/// Not "all the runs of A, then all the runs of B". Doing it that way was
/// tried and it produced a table where the same encoder measured 8 ms in one
/// arrangement and 31 ms in another, because whichever candidate went second
/// paid for the heat the first one generated. Interleaving does not remove the
/// noise, it just stops it landing on one column.
fn interleave(runs: usize, candidates: &mut [&mut dyn FnMut() -> usize]) -> Vec<(Timing, usize)> {
    // One untimed round first: the first call of each opens a device,
    // allocates a pool and warms a cache, none of which the thirtieth frame of
    // playback pays for.
    let mut bytes: Vec<usize> = candidates.iter_mut().map(|body| body()).collect();
    let mut samples: Vec<Vec<f64>> = vec![Vec::with_capacity(runs); candidates.len()];

    for _ in 0..runs {
        for (index, body) in candidates.iter_mut().enumerate() {
            let started = Instant::now();
            bytes[index] = body();
            samples[index].push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }

    samples
        .into_iter()
        .zip(bytes)
        .map(|(s, b)| (summarise(s), b))
        .collect()
}

fn main() -> anyhow::Result<()> {
    let mut runs = 9usize;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runs" => runs = args.next().and_then(|v| v.parse().ok()).unwrap_or(runs),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }

    let quality = 88u8;
    let sizes = [(540u32, 960u32), (1080, 1920), (1920, 1080)];

    let hardware = hardware_available();
    println!("{runs} interleaved runs at quality 88, milliseconds as median/best\n");
    if !hardware {
        println!("no hardware JPEG encoder on this machine; the VAAPI columns are blank\n");
    }
    println!(
        "{:>11} {:>14} {:>14} {:>14} {:>8} {:>8}",
        "size", "jpeg-encoder", "libjpeg-turbo", "vaapi", "speedup", "PSNR"
    );

    for (width, height) in sizes {
        let rgba = frame(width, height);

        // Each candidate hands back the byte count so the table can show what
        // the three encoders cost in bandwidth as well as in time.
        let mut old = || {
            let mut out = Vec::with_capacity(rgba.len() / 8);
            jpeg_encoder::Encoder::new(&mut out, quality)
                .encode(
                    &rgba,
                    width as u16,
                    height as u16,
                    jpeg_encoder::ColorType::Rgba,
                )
                .expect("jpeg-encoder");
            out.len()
        };

        let mut software_jpeg = Vec::new();
        let turbo = |bytes: &mut Vec<u8>| {
            *bytes = encode_jpeg(&rgba, width, height, quality).expect("software encode");
            bytes.len()
        };

        if !hardware {
            let mut turbo = || turbo(&mut software_jpeg);
            let out = interleave(runs, &mut [&mut old, &mut turbo]);
            println!(
                "{:>11} {:>14} {:>14} {:>14} {:>8} {:>8}",
                format!("{width}x{height}"),
                format!("{:.1}/{:.1}", out[0].0.median, out[0].0.best),
                format!("{:.1}/{:.1}", out[1].0.median, out[1].0.best),
                "-",
                "-",
                "-"
            );
            continue;
        }

        // Built once and reused, which is what the server does too: rebuilding
        // it per frame would be measuring `avcodec_open2`.
        let mut encoder = VaapiJpegEncoder::open(width, height, quality)?;
        let mut hardware_jpeg = Vec::new();
        let out = {
            let mut turbo = || turbo(&mut software_jpeg);
            let mut vaapi = || {
                hardware_jpeg = encoder.encode(&rgba).expect("vaapi encode");
                hardware_jpeg.len()
            };
            interleave(runs, &mut [&mut old, &mut turbo, &mut vaapi])
        };

        let (software_rgb, _, _) = decode_jpeg_rgb(&software_jpeg)?;
        let (hardware_rgb, _, _) = decode_jpeg_rgb(&hardware_jpeg)?;
        let db = psnr_rgb(&software_rgb, &hardware_rgb);
        let stages = encoder.stages();

        println!(
            "{:>11} {:>14} {:>14} {:>14} {:>7.1}x {:>7.1}",
            format!("{width}x{height}"),
            format!("{:.1}/{:.1}", out[0].0.median, out[0].0.best),
            format!("{:.1}/{:.1}", out[1].0.median, out[1].0.best),
            format!("{:.1}/{:.1}", out[2].0.median, out[2].0.best),
            out[1].0.best / out[2].0.best,
            db
        );
        println!(
            "{:>11}   {} / {} / {} KB   |   convert {:.2}, upload {:.2}, encode {:.2} ms",
            "",
            out[0].1 / 1024,
            out[1].1 / 1024,
            out[2].1 / 1024,
            stages.convert_micros as f64 / 1000.0,
            stages.upload_micros as f64 / 1000.0,
            stages.encode_micros as f64 / 1000.0,
        );
    }

    // What the server actually pays, dispatch, fallback check and lock and all.
    println!();
    let (width, height) = (1080u32, 1920);
    let rgba = frame(width, height);
    let mut backend = Backend::Software;
    let mut through = || {
        let (bytes, used) = encode_preview_jpeg(&rgba, width, height, quality).expect("encode");
        backend = used;
        bytes.len()
    };
    let out = interleave(runs, &mut [&mut through]);
    println!(
        "through encode_preview_jpeg at {width}x{height}: {:.2} ms median, {:.2} best, \
         on {} ({} KB)",
        out[0].0.median,
        out[0].0.best,
        backend.label(),
        out[0].1 / 1024
    );
    if backend == Backend::Software && hardware {
        println!("  (fell back to software, which is a bug or an odd frame size)");
    }

    // The cost of a size or quality change, which the server pays whenever the
    // session changes. Worth knowing: if it were tens of milliseconds the
    // encoder would have to be cached per quality rather than rebuilt.
    if hardware {
        let started = Instant::now();
        let _ = VaapiJpegEncoder::open(width, height, 94)?;
        println!(
            "opening a {width}x{height} hardware encoder: {:.2} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    Ok(())
}
