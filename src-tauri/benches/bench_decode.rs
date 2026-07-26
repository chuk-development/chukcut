//! What one frame costs to decode, three ways, four codecs, two orientations,
//! sequentially and by seeking.
//!
//! This is the group `docs/STATUS.md` leans on hardest, because the middle
//! column is counter-intuitive and keeps getting rediscovered: **hardware
//! decode is slower than software when the frame has to end up in system
//! memory.** A VA surface is `Y_TILED`, so the download is a detiling pass over
//! the whole frame, and swscale still has to turn NV12 into RGBA afterwards.
//! Together they cost more than the decode they were supposed to accelerate.
//!
//! The three columns are the three things a caller can ask for, and the gaps
//! between them are where the time goes:
//!
//! - `sw →RGBA` — decode and colour-convert on the CPU. What ships today.
//! - `hw →RGBA` — decode on the GPU, then download and convert. What "turn on
//!   hardware decode" naively means.
//! - `hw →DMA-BUF` — decode on the GPU and stop. What the frame costs when
//!   nothing copies it, and the number the preview and export are waiting on.
//!
//! Random access is measured separately and with fewer samples, because a seek
//! costs an order of magnitude more than a sequential step and would otherwise
//! be the whole run. It is measured at all because backward seeking during
//! playback is a known rough edge (`docs/STATUS.md`, task #9) and nothing was
//! putting a number on it.

use std::path::Path;

use chukcut_lib::modules::media::decoder::Acceleration;
use chukcut_lib::modules::media::{hwdecode, probe, VideoDecoder};

use crate::fixtures::Fixtures;
use crate::harness::{rounds, Measurement};

pub const GROUP: &str = "decode";

/// What the caller wants out of each decoded frame.
#[derive(Clone, Copy, PartialEq)]
enum Want {
    /// Tightly packed RGBA in system memory, which is what the compositor is
    /// fed today.
    Rgba,
    /// DMA-BUF handles onto the surface, which is what it should be fed.
    Dmabuf,
}

pub struct Budget {
    /// Frames in a sequential walk.
    pub frames: usize,
    /// Repeats of each sequential walk.
    pub rounds: usize,
    /// Seeks in a random-access run.
    pub seeks: usize,
    /// Repeats of each random-access run.
    pub seek_rounds: usize,
}

pub fn run(media: &Fixtures, budget: &Budget) -> Vec<Measurement> {
    let mut out = Vec::new();

    for (codec, reason) in &media.missing {
        out.push(Measurement::skip(
            GROUP,
            format!("{codec} — no fixture"),
            reason.clone(),
        ));
    }

    let hardware: Vec<&hwdecode::HwDecodeSupport> = hwdecode::capabilities().iter().collect();

    for clip in &media.clips {
        let label = clip.label();
        let step = frame_step(&clip.path);
        // Printed on the software row of every clip. Without it the table reads
        // as a property of the codec when it is substantially a property of the
        // bitrate — the mistake documented at the top of `fixtures.rs`.
        let bitrate = format!("{:.1} Mbit/s", clip.megabits());

        // Software, sequential. The baseline every other row is read against.
        match rounds(budget.rounds, |_| {
            walk(&clip.path, Acceleration::Software, budget.frames, step, Want::Rgba)
        }) {
            Ok(samples) => out.push(
                Measurement::ms(GROUP, format!("{label} sw →RGBA seq"), samples)
                    .with_note(bitrate.clone()),
            ),
            Err(error) => out.push(Measurement::skip(
                GROUP,
                format!("{label} sw →RGBA seq"),
                error,
            )),
        }

        let hw_note = hardware
            .iter()
            .find(|s| s.codec.label().eq_ignore_ascii_case(&hw_name(clip.codec)))
            .filter(|s| !s.usable)
            .and_then(|s| s.note.clone());

        if let Some(reason) = hw_note {
            out.push(Measurement::skip(
                GROUP,
                format!("{label} hw →RGBA seq"),
                reason.clone(),
            ));
            out.push(Measurement::skip(
                GROUP,
                format!("{label} hw →DMA-BUF seq"),
                reason,
            ));
        } else {
            for (want, suffix) in [(Want::Rgba, "hw →RGBA seq"), (Want::Dmabuf, "hw →DMA-BUF seq")] {
                match rounds(budget.rounds, |_| {
                    walk(&clip.path, Acceleration::Vaapi, budget.frames, step, want)
                }) {
                    Ok(samples) => out.push(Measurement::ms(
                        GROUP,
                        format!("{label} {suffix}"),
                        samples,
                    )),
                    Err(error) => {
                        out.push(Measurement::skip(GROUP, format!("{label} {suffix}"), error))
                    }
                }
            }
        }

        // Random access. Software and the zero-copy hardware path only: the
        // hardware→RGBA seek would measure the same download the sequential row
        // already measured, on top of a seek, and buys nothing for the minutes
        // it costs.
        for (accel, want, suffix) in [
            (Acceleration::Software, Want::Rgba, "sw →RGBA random"),
            (Acceleration::Vaapi, Want::Dmabuf, "hw →DMA-BUF random"),
        ] {
            match rounds(budget.seek_rounds, |round| {
                seek_storm(&clip.path, accel, budget.seeks, step, want, round as u64)
            }) {
                Ok(samples) => out.push(
                    Measurement::ms(GROUP, format!("{label} {suffix}"), samples)
                        .with_note(format!("{} seeks", budget.seeks)),
                ),
                Err(error) => out.push(Measurement::skip(GROUP, format!("{label} {suffix}"), error)),
            }
        }
    }

    out
}

/// `probe`'s codec name mapped onto `HwCodec`'s label, so a missing hardware
/// path can be reported with the reason the probe recorded rather than as a
/// bare failure deep inside the decoder.
fn hw_name(codec: &str) -> String {
    match codec {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "vp9" => "VP9",
        "av1" => "AV1",
        other => other,
    }
    .to_string()
}

/// Microseconds between frames, from the file's own rate. Walking at a fixed
/// 33 ms on a 60 fps clip would decode every second frame and halve the
/// apparent cost.
fn frame_step(path: &Path) -> i64 {
    probe(path)
        .ok()
        .and_then(|info| info.video)
        .map(|v| (1_000_000.0 / v.fps.max(1.0)) as i64)
        .unwrap_or(33_333)
}

/// Milliseconds per frame over a sequential walk, first frame excluded.
///
/// The first frame pays for the keyframe, the codec context and — on the
/// hardware path — the whole VAAPI surface pool. It is real cost, but it is
/// paid once per file rather than once per frame, and including it makes a
/// short run look two to three times worse than it is.
fn walk(
    path: &Path,
    accel: Acceleration,
    frames: usize,
    step: i64,
    want: Want,
) -> Result<f64, String> {
    let mut decoder = VideoDecoder::open_with(path, accel).map_err(|e| e.to_string())?;
    pull(&mut decoder, step / 2, want)?;

    let started = std::time::Instant::now();
    for n in 1..frames {
        pull(&mut decoder, n as i64 * step + step / 2, want)?;
    }
    let per_frame = started.elapsed().as_secs_f64() * 1000.0 / frames.saturating_sub(1).max(1) as f64;
    Ok(per_frame)
}

/// Milliseconds per *seek*, jumping around the file rather than walking it.
///
/// The positions are deterministic — a fixed low-discrepancy sequence seeded by
/// the round — so two runs of the suite seek to the same places and are
/// comparable. A random sequence would put the run-to-run spread inside the
/// measurement instead of around it.
fn seek_storm(
    path: &Path,
    accel: Acceleration,
    seeks: usize,
    step: i64,
    want: Want,
    round: u64,
) -> Result<f64, String> {
    let info = probe(path).map_err(|e| e.to_string())?;
    let duration = info.duration.max(step * 2);
    let mut decoder = VideoDecoder::open_with(path, accel).map_err(|e| e.to_string())?;
    pull(&mut decoder, step / 2, want)?;

    // The golden ratio in fixed point: successive multiples never cluster, so
    // every seek lands in a different GOP and none of them is a no-op.
    const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
    let positions: Vec<i64> = (0..seeks)
        .map(|n| {
            let scrambled = (n as u64 + 1).wrapping_add(round * 7).wrapping_mul(GOLDEN);
            let fraction = (scrambled >> 11) as f64 / (1u64 << 53) as f64;
            ((duration - step) as f64 * fraction) as i64
        })
        .collect();

    let started = std::time::Instant::now();
    for at in &positions {
        pull(&mut decoder, *at, want)?;
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0 / seeks.max(1) as f64)
}

fn pull(decoder: &mut VideoDecoder, at: i64, want: Want) -> Result<(), String> {
    match want {
        Want::Rgba => decoder.seek_and_decode(at).map(drop),
        // The mapped frame is dropped immediately, which returns the surface to
        // the pool — the same thing a consumer does once its texture is built,
        // so the pool pressure is representative.
        Want::Dmabuf => decoder.seek_and_map(at).map(drop),
    }
    .map_err(|e| e.to_string())
}
