//! How much a removed static logo shimmers on a panning shot, with and
//! without following the camera (`enhance::removal`).
//!
//! Generates its own footage (ffmpeg): a still picture panned left to right
//! at `--speed` pixels a frame under a white box that stays where it is (the
//! "logo"), and the same pan without the box (the truth). Each frame goes
//! through `Remover` with the box as the mask, twice: following the camera
//! and not. Per run it prints, inside the hole:
//!
//! - **flicker**: the mean change from one output frame to the next after
//!   the known pan is taken out (what the eye sees as shimmer), next to the
//!   same number for the truth (what the footage itself changes by);
//! - **error**: the mean difference from the truth;
//! - how many frames LaMa ran on, and how many were followed.
//!
//! Needs the ML worker and the LaMa model:
//!
//! ```text
//! CHUKCUT_ML_WORKER=target/release/chukcut-ml-worker \
//!   cargo run --release -p chukcut-engine --example removal_flicker -- --speed 3 --frames 120
//! ```
//!
//! `--moves` prints the camera move `removal::camera_move` finds between
//! each pair of frames instead (no model needed). ffmpeg's crop moves by
//! even pixels on 4:2:0 footage only to the pixel, so `--speed 2.5` pans by
//! 2 and 3 in turn, with the chroma a little off on the odd steps.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use chukcut_engine::modules::enhance::removal::Remover;
use chukcut_engine::modules::media::VideoDecoder;

const W: usize = 1280;
const H: usize = 720;
/// The logo: x, y, width, height.
const LOGO: (usize, usize, usize, usize) = (980, 60, 220, 90);

fn arg(name: &str, default: f64) -> f64 {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error"])
        .args(args)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg {args:?}");
}

/// The panned footage, with and without the logo.
fn footage(speed: f64, frames: usize) -> (PathBuf, PathBuf) {
    let dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-media/removal-flicker");
    std::fs::create_dir_all(&dir).unwrap();
    let still = dir.join("still.png");
    if !still.exists() {
        // A picture with detail everywhere: a frame of the Mandelbrot zoom
        // over a colour test pattern.
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "mandelbrot=size=2560x720:start_scale=0.6:end_pts=1",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=2560x720",
            "-filter_complex",
            "[0][1]blend=all_mode=average",
            "-frames:v",
            "1",
            still.to_str().unwrap(),
        ]);
    }
    let name = |logo: bool| dir.join(format!("pan-{speed}-{frames}-{}.mp4", u8::from(logo)));
    for logo in [false, true] {
        let out = name(logo);
        if out.exists() {
            continue;
        }
        let mut filter = format!("crop={W}:{H}:x='n*{speed}':y=0");
        if logo {
            filter.push_str(&format!(
                ",drawbox=x={}:y={}:w={}:h={}:color=white:t=fill",
                LOGO.0, LOGO.1, LOGO.2, LOGO.3
            ));
        }
        ffmpeg(&[
            "-loop",
            "1",
            "-framerate",
            "30",
            "-i",
            still.to_str().unwrap(),
            "-vf",
            &filter,
            "-frames:v",
            &frames.to_string(),
            "-c:v",
            "libx264",
            "-crf",
            "12",
            "-pix_fmt",
            "yuv420p",
            out.to_str().unwrap(),
        ]);
    }
    (name(true), name(false))
}

fn frames(path: &Path, n: usize) -> Vec<Vec<u8>> {
    let mut decoder = VideoDecoder::open(path).expect("open");
    (0..n)
        .map(|k| {
            let at = ((k as f64 + 0.5) * 1_000_000.0 / 30.0) as i64;
            let frame = decoder.seek_and_decode(at).expect("decode");
            assert_eq!((frame.width as usize, frame.height as usize), (W, H));
            frame.data
        })
        .collect()
}

fn in_hole(x: usize, y: usize) -> bool {
    (LOGO.0..LOGO.0 + LOGO.2).contains(&x) && (LOGO.1..LOGO.1 + LOGO.3).contains(&y)
}

/// Mean |a(x, y) − b(x + shift, y)| over the hole (b sampled linearly).
fn moved_difference(a: &[u8], b: &[u8], shift: f64) -> f64 {
    let (mut sum, mut n) = (0.0, 0u64);
    for y in LOGO.1..LOGO.1 + LOGO.3 {
        for x in LOGO.0..LOGO.0 + LOGO.2 {
            let sx = x as f64 + shift;
            let (x0, f) = (sx.floor() as usize, sx - sx.floor());
            if x0 + 1 >= W {
                continue;
            }
            for c in 0..3 {
                let v = b[(y * W + x0) * 4 + c] as f64 * (1.0 - f)
                    + b[(y * W + x0 + 1) * 4 + c] as f64 * f;
                sum += (a[(y * W + x) * 4 + c] as f64 - v).abs();
                n += 1;
            }
        }
    }
    sum / n as f64
}

fn main() {
    let speed = arg("--speed", 3.0);
    let count = arg("--frames", 60.0) as usize;
    let (logo, clean) = footage(speed, count);
    let input = frames(&logo, count);
    let truth = frames(&clean, count);
    let mask: Vec<u8> = (0..W * H)
        .map(|i| u8::from(in_hole(i % W, i / W)) * 255)
        .collect();
    if std::env::args().any(|a| a == "--moves") {
        for (k, pair) in input.windows(2).enumerate() {
            let m = chukcut_engine::modules::enhance::removal::camera_move(
                W, H, &pair[0], &mask, &pair[1], &mask,
            );
            println!("{k}: {m:?}");
        }
        return;
    }
    let cancel = AtomicBool::new(false);
    let provider = chukcut_engine::modules::ml::inpaint::prepare(&|_, _| {}, &cancel)
        .expect("the LaMa model and the ML worker");
    let flicker = |outs: &[Vec<u8>]| {
        outs.windows(2)
            .map(|p| moved_difference(&p[1], &p[0], speed))
            .sum::<f64>()
            / (outs.len() - 1) as f64
    };
    println!(
        "{count} frames of {W}x{H} panning {speed} px a frame, a {}x{} logo, LaMa on {provider}",
        LOGO.2, LOGO.3
    );
    println!("truth: flicker {:.2}", flicker(&truth));
    for follow in [false, true] {
        let mut remover = Remover::new(W, H, chukcut_engine::modules::enhance::DEFAULT_GROW);
        remover.follow_camera = follow;
        let started = std::time::Instant::now();
        let outs: Vec<Vec<u8>> = input
            .iter()
            .map(|f| remover.next(f, &mask, None, Some(&cancel)).expect("remove"))
            .collect();
        let error = outs
            .iter()
            .zip(&truth)
            .map(|(o, t)| moved_difference(o, t, 0.0))
            .sum::<f64>()
            / count as f64;
        println!(
            "{}: flicker {:.2}, error {:.2}, LaMa on {} of {count} frames, followed {}, {:.1} ms a frame",
            if follow { "following the camera" } else { "per frame (before)" },
            flicker(&outs),
            error,
            remover.model_runs,
            remover.followed,
            started.elapsed().as_secs_f64() * 1000.0 / count as f64
        );
    }
    chukcut_engine::modules::ml::worker::shutdown();
}
