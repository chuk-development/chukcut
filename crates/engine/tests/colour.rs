//! Colour in and out: what an export writes, and what an HDR source becomes.
//!
//! Every fixture here is built from numbers this file chose, so the checks
//! are against known values rather than against another run of the same code.
//!
//! - **Export.** Colour bars encoded as BT.709 go through a whole export on
//!   every encoder this machine has and come back through two decoders: ours,
//!   and `ffmpeg`'s, which honours the tags the export wrote. Both must give
//!   back the source's colours within two code values, and `ffprobe` must
//!   see `bt709` (or `smpte170m` for SD) tags. Before the fix the export wrote
//!   BT.601 samples with no tags, and every player read them as BT.709.
//! - **HDR input.** Flat patches of known SDR colours are encoded as PQ and as
//!   HLG in BT.2020, 10-bit, by a generator written here from the standards'
//!   formulas — not by the shader's inverse. The compositor must turn them
//!   back into the SDR colours wherever they sit below the tone map's knee,
//!   and into a reference BT.2390 tone map above it, on the software decoder
//!   and on NVDEC alike, in the preview and in the export.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ColorMatrix, ColorRange, ExportJob, ExportOverrides,
    ExportRequest, FnSink, Quality,
};
use chukcut_engine::modules::media::{Acceleration, MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{CanvasConfig, Project, Track, TrackKind};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};

use support::{material_for, segment};

const W: u32 = 1280;
const H: u32 = 720;

/// Seventy-five per cent bars over full-saturation primaries and greys: the
/// colours a matrix error moves most (saturated red and green) and the ones it
/// cannot move at all (greys), so a failure says which it is.
const BARS: [[u8; 3]; 16] = [
    [191, 191, 191],
    [191, 191, 0],
    [0, 191, 191],
    [0, 191, 0],
    [191, 0, 191],
    [191, 0, 0],
    [0, 0, 191],
    [16, 16, 16],
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [0, 255, 255],
    [255, 0, 255],
    [255, 255, 0],
    [235, 235, 235],
    [128, 128, 128],
];

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("colour");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir.join(name)
}

fn has_tool(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run ffmpeg with raw input on stdin.
fn ffmpeg_with_input(args: &[&str], input: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .map_err(|e| format!("feeding ffmpeg: {e}"))?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "ffmpeg {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// The patch grid: eight columns, two rows.
fn patch_rect(index: usize, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let (cols, rows) = (8u32, 2u32);
    let (pw, ph) = (width / cols, height / rows);
    let (col, row) = (index as u32 % cols, index as u32 / cols);
    (col * pw, row * ph, pw, ph)
}

/// The mean colour of the middle of patch `index` in a packed RGBA frame,
/// away from the edges where chroma subsampling blends neighbours.
fn patch_mean(rgba: &[u8], width: u32, height: u32, index: usize) -> [f64; 3] {
    let (x0, y0, pw, ph) = patch_rect(index, width, height);
    let (cx, cy) = (x0 + pw / 2, y0 + ph / 2);
    let mut sum = [0.0f64; 3];
    let mut n = 0.0;
    for y in cy - 8..cy + 8 {
        for x in cx - 8..cx + 8 {
            let i = ((y * width + x) * 4) as usize;
            for c in 0..3 {
                sum[c] += rgba[i + c] as f64;
            }
            n += 1.0;
        }
    }
    sum.map(|v| v / n)
}

fn max_error(a: &[f64; 3], b: &[f64; 3]) -> f64 {
    (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f64::max)
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// The bars as RGB24, `width` x `height`.
fn bars_rgb(width: u32, height: u32) -> Vec<u8> {
    let mut rgb = vec![0u8; (width * height * 3) as usize];
    for (index, colour) in BARS.iter().enumerate() {
        let (x0, y0, pw, ph) = patch_rect(index, width, height);
        for y in y0..y0 + ph {
            for x in x0..x0 + pw {
                let i = ((y * width + x) * 3) as usize;
                rgb[i..i + 3].copy_from_slice(colour);
            }
        }
    }
    rgb
}

/// The bars as a BT.709, limited range, tagged H.264 file, near-lossless.
fn bars_clip(width: u32, height: u32) -> Option<PathBuf> {
    if !has_tool("ffmpeg") {
        return None;
    }
    let path = scratch(&format!("bars_{width}x{height}.mp4"));
    if path.exists() {
        return Some(path);
    }
    let frame = bars_rgb(width, height);
    let mut input = Vec::new();
    for _ in 0..10 {
        input.extend_from_slice(&frame);
    }
    let size = format!("{width}x{height}");
    ffmpeg_with_input(
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            &size,
            "-r",
            "30",
            "-i",
            "-",
            "-vf",
            "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
            path.to_str().expect("utf-8 path"),
        ],
        &input,
    )
    .expect("encode the bars");
    Some(path)
}

fn project_of(clip: &Path, width: u32, height: u32) -> Project {
    let mut project = Project::new(
        "colour",
        CanvasConfig {
            width,
            height,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project
        .materials
        .videos
        .push(material_for("clip", clip).expect("probe the clip"));
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment("clip", 0, 300_000));
    project.tracks.push(track);
    project
}

/// Export `project` with `overrides` on the encoder `hardware` (software when
/// `None`). Returns the file.
fn export(
    project: &Project,
    name: &str,
    hardware: Option<&str>,
    overrides: ExportOverrides,
) -> PathBuf {
    let ctx = support::gpu().expect("a GPU");
    let path = scratch(name);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: Some(overrides),
        hardware: hardware.map(str::to_string),
        include_audio: false,
        range: None,
    };
    let settings = resolve_settings(project, &request).expect("resolve the settings");
    let path = settings.output_path.clone();
    let _ = std::fs::remove_file(&path);
    let job = ExportJob {
        job_id: "colour".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(project)) as Arc<dyn SourceProvider>,
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    run_export(&job, &FnSink(|_| {})).expect("the export should finish");
    path
}

/// `color_space,color_transfer,color_primaries,color_range` as ffprobe sees
/// the first video stream.
fn probe_tags(file: &Path) -> String {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=color_space,color_transfer,color_primaries,color_range",
            "-of",
            "csv=p=0",
        ])
        .arg(file)
        .output()
        .expect("run ffprobe");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The first frame decoded by our decoder, as packed RGBA.
fn decode_ours(file: &Path) -> (Vec<u8>, u32, u32) {
    let mut decoder = VideoDecoder::open(file).expect("open the export");
    let frame = decoder.seek_and_decode(0).expect("decode the first frame");
    (frame.data, frame.width, frame.height)
}

/// The first frame decoded by `ffmpeg` to RGB, as packed RGBA. ffmpeg's
/// conversion reads the stream's matrix and range tags, which is what makes it
/// an independent check of them.
fn decode_ffmpeg(file: &Path, width: u32, height: u32) -> Vec<u8> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len(), (width * height * 4) as usize);
    output.stdout
}

/// The first frame's Y'CbCr samples as ffmpeg decodes them, in 8-bit code
/// values (10-bit samples divided by four), one `(Y, Cb, Cr)` mean per patch.
/// No conversion of any kind happens on the way, so this is what the encoder
/// was given, less the codec's loss.
fn patch_yuv(file: &Path, width: u32, height: u32) -> Vec<[f64; 3]> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(file)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p16le",
            "-",
        ])
        .output()
        .expect("run ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let samples: Vec<f64> = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b) as f64 / 256.0)
        .collect();
    let (w, h) = (width as usize, height as usize);
    let (luma, rest) = samples.split_at(w * h);
    let (cb, cr) = rest.split_at(w * h / 4);
    (0..BARS.len())
        .map(|index| {
            let (x0, y0, pw, ph) = patch_rect(index, width, height);
            let (cx, cy) = ((x0 + pw / 2) as usize, (y0 + ph / 2) as usize);
            let mut sum = [0.0; 3];
            let mut n = 0.0;
            for y in cy - 8..cy + 8 {
                for x in cx - 8..cx + 8 {
                    sum[0] += luma[y * w + x];
                    sum[1] += cb[(y / 2) * (w / 2) + x / 2];
                    sum[2] += cr[(y / 2) * (w / 2) + x / 2];
                    n += 1.0;
                }
            }
            sum.map(|v| v / n)
        })
        .collect()
}

/// The worst patch error of `file` against `reference`, through both
/// decoders.
fn round_trip_error(file: &Path, reference: &[[f64; 3]], label: &str) -> f64 {
    let (ours, width, height) = decode_ours(file);
    let theirs = decode_ffmpeg(file, width, height);
    let mut worst: f64 = 0.0;
    for (index, expected) in reference.iter().enumerate() {
        for (decoder, rgba) in [("chukcut", &ours), ("ffmpeg", &theirs)] {
            let got = patch_mean(rgba, width, height, index);
            let error = max_error(&got, expected);
            eprintln!("{label} {decoder} patch {index}: expected {expected:?}, got {got:?}");
            worst = worst.max(error);
        }
    }
    eprintln!("{label}: worst patch error {worst:.2} code values");
    worst
}

/// The software encoders and every hardware encoder this machine can drive,
/// for one codec.
fn encoders(codec: &str) -> Vec<Option<String>> {
    let mut all = vec![None];
    all.extend(
        chukcut_engine::modules::export::hwaccel::detect()
            .into_iter()
            .filter(|e| e.usable && e.id.ends_with(codec))
            .map(|e| Some(e.id)),
    );
    all
}

#[test]
fn an_hd_export_is_bt709_tagged_and_its_colours_survive_the_round_trip() {
    let _ = require_gpu!();
    let Some(clip) = bars_clip(W, H) else {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    };
    // The reference is the source as decoded, not the numbers it was made
    // from: 8-bit limited-range Y'CbCr cannot hold saturated RGB to better
    // than two or three code values, so the source itself is that far off
    // the bars before anything of ours touches it.
    let (source, sw, sh) = decode_ours(&clip);
    let reference: Vec<[f64; 3]> = (0..BARS.len())
        .map(|index| patch_mean(&source, sw, sh, index))
        .collect();
    for (index, (got, bar)) in reference.iter().zip(BARS).enumerate() {
        assert!(
            max_error(got, &bar.map(f64::from)) <= 3.0,
            "source patch {index}: {got:?}"
        );
    }
    let source_yuv = patch_yuv(&clip, W, H);

    let project = project_of(&clip, W, H);
    for (codec, ten_bit) in [("h264", false), ("h265", false), ("h265", true)] {
        for hardware in encoders(codec) {
            let label = format!(
                "{} {codec}{}",
                hardware.as_deref().unwrap_or("software"),
                if ten_bit { " 10-bit" } else { "" }
            );
            let name = format!(
                "{}_{codec}_{ten_bit}.mp4",
                hardware.as_deref().unwrap_or("sw")
            );
            let overrides = ExportOverrides {
                video_codec: Some(if codec == "h264" {
                    chukcut_engine::modules::export::VideoCodec::H264
                } else {
                    chukcut_engine::modules::export::VideoCodec::H265
                }),
                quality: Some(Quality::Crf(12)),
                ten_bit,
                ..Default::default()
            };
            let file = export(&project, &name, hardware.as_deref(), overrides);
            assert_eq!(probe_tags(&file), "tv,bt709,bt709,bt709", "{label}");
            // The samples the encoder was given, against the source's: the
            // measure a matrix error shows up in first, by ten code values
            // and more in saturated colours.
            let yuv = patch_yuv(&file, W, H);
            let yuv_error = yuv
                .iter()
                .zip(&source_yuv)
                .map(|(a, b)| max_error(a, b))
                .fold(0.0, f64::max);
            eprintln!("{label}: worst Y'CbCr patch error {yuv_error:.2} code values");
            assert!(yuv_error <= 2.0, "{label}: Y'CbCr off by {yuv_error:.2}");
            // And the picture as two players would show it.
            let error = round_trip_error(&file, &reference, &label);
            assert!(error <= 3.0, "{label}: worst patch error {error:.2}");
        }
    }
}

#[test]
fn an_sd_export_is_bt601_and_full_range_is_tagged_full() {
    let _ = require_gpu!();
    let Some(clip) = bars_clip(640, 480) else {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    };
    let (source, sw, sh) = decode_ours(&clip);
    let reference: Vec<[f64; 3]> = (0..BARS.len())
        .map(|index| patch_mean(&source, sw, sh, index))
        .collect();
    let project = project_of(&clip, 640, 480);

    let sd = export(
        &project,
        "sd.mp4",
        None,
        ExportOverrides {
            quality: Some(Quality::Crf(12)),
            ..Default::default()
        },
    );
    // SMPTE 170M matrix, BT.709 primaries and transfer: only the matrix
    // changes with the size. See `export::colour`.
    assert_eq!(probe_tags(&sd), "tv,smpte170m,bt709,bt709");
    assert!(round_trip_error(&sd, &reference, "sd auto") <= 3.0);

    let full = export(
        &project,
        "full.mp4",
        None,
        ExportOverrides {
            quality: Some(Quality::Crf(12)),
            color_matrix: Some(ColorMatrix::Bt709),
            color_range: Some(ColorRange::Full),
            ..Default::default()
        },
    );
    assert_eq!(probe_tags(&full), "pc,bt709,bt709,bt709");
    assert!(round_trip_error(&full, &reference, "sd forced bt709 full") <= 3.0);
}

// ---------------------------------------------------------------------------
// HDR input
// ---------------------------------------------------------------------------

/// SDR colours that span the tone map: blacks and midtones below the knee,
/// where the round trip must be exact, and white and bright saturated colours
/// above it, where it must match the reference curve.
const HDR_PATCHES: [[u8; 3]; 16] = [
    [0, 0, 0],
    [32, 32, 32],
    [64, 64, 64],
    [96, 96, 96],
    [128, 128, 128],
    [160, 160, 160],
    [200, 200, 200],
    [255, 255, 255],
    [140, 40, 40],
    [40, 140, 60],
    [50, 70, 160],
    [150, 120, 60],
    [255, 0, 0],
    [0, 255, 0],
    [255, 220, 120],
    [120, 220, 255],
];

const REFERENCE_WHITE: f64 = 203.0;
const PEAK: f64 = 1000.0;

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f64) -> f64 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// BT.709 to BT.2020 primaries, linear (ITU-R BT.2087).
fn bt709_to_bt2020(c: [f64; 3]) -> [f64; 3] {
    [
        0.627_404 * c[0] + 0.329_283 * c[1] + 0.043_313 * c[2],
        0.069_097 * c[0] + 0.919_540 * c[1] + 0.011_362 * c[2],
        0.016_391 * c[0] + 0.088_013 * c[1] + 0.895_595 * c[2],
    ]
}

const M1: f64 = 0.159_301_757_812_5;
const M2: f64 = 78.843_75;
const C1: f64 = 0.835_937_5;
const C2: f64 = 18.851_562_5;
const C3: f64 = 18.6875;

/// PQ inverse EOTF: light as a fraction of 10 000 nits to signal.
fn pq_encode(y: f64) -> f64 {
    let p = y.clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * p) / (1.0 + C3 * p)).powf(M2)
}

fn pq_decode(e: f64) -> f64 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// HLG OETF (ARIB STD-B67): scene light 0..1 to signal.
fn hlg_oetf(e: f64) -> f64 {
    let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_73);
    if e <= 1.0 / 12.0 {
        (3.0 * e).sqrt()
    } else {
        a * (12.0 * e - b).ln() + c
    }
}

/// Display light in nits to the HLG signal for a 1000-nit display: the
/// inverse of the BT.2100 OOTF (gamma 1.2), then the OETF.
fn hlg_encode(nits: [f64; 3]) -> [f64; 3] {
    let gamma = 1.2;
    let yd = 0.2627 * nits[0] + 0.6780 * nits[1] + 0.0593 * nits[2];
    if yd <= 0.0 {
        return [0.0; 3];
    }
    let scale = (yd / PEAK).powf((1.0 - gamma) / gamma) / PEAK;
    nits.map(|v| hlg_oetf(v * scale))
}

/// The BT.2390 EETF, written again from the recommendation: SDR-relative
/// light (1.0 = 203 nits) in, out mapped onto 0..1.
fn reference_eetf(light: f64) -> f64 {
    let source = pq_encode(PEAK / 10_000.0);
    let target = pq_encode(REFERENCE_WHITE / 10_000.0) / source;
    let knee = 1.5 * target - 0.5;
    let e = (pq_encode(light * REFERENCE_WHITE / 10_000.0) / source).min(1.0);
    if e <= knee {
        return light;
    }
    let t = (e - knee) / (1.0 - knee);
    let mapped = (2.0 * t.powi(3) - 3.0 * t.powi(2) + 1.0) * knee
        + (t.powi(3) - 2.0 * t.powi(2) + t) * (1.0 - knee)
        + (-2.0 * t.powi(3) + 3.0 * t.powi(2)) * target;
    pq_decode(mapped * source) * 10_000.0 / REFERENCE_WHITE
}

/// What the compositor should show for an SDR patch that went through HDR:
/// the tone map on the brightest channel, the others scaled with it.
fn expected_sdr(colour: [u8; 3]) -> [f64; 3] {
    let light = colour.map(|v| srgb_to_linear(v as f64 / 255.0));
    let peak = light.iter().cloned().fold(0.0, f64::max);
    let ratio = if peak > 0.0 {
        reference_eetf(peak) / peak
    } else {
        1.0
    };
    light.map(|v| linear_to_srgb(v * ratio) * 255.0)
}

#[derive(Clone, Copy, Debug)]
enum Hdr {
    Pq,
    Hlg,
}

/// The 10-bit limited-range BT.2020 Y'CbCr codes for one SDR colour.
fn hdr_codes(kind: Hdr, colour: [u8; 3]) -> [u16; 3] {
    let linear = colour.map(|v| srgb_to_linear(v as f64 / 255.0));
    let nits = bt709_to_bt2020(linear).map(|v| v.max(0.0) * REFERENCE_WHITE);
    let signal = match kind {
        Hdr::Pq => nits.map(|v| pq_encode(v / 10_000.0)),
        Hdr::Hlg => hlg_encode(nits),
    };
    // BT.2020 non-constant-luminance Y'CbCr, 10-bit limited range.
    let luma = 0.2627 * signal[0] + 0.6780 * signal[1] + 0.0593 * signal[2];
    let cb = (signal[2] - luma) / 1.8814;
    let cr = (signal[0] - luma) / 1.4746;
    [
        (64.0 + 876.0 * luma).round() as u16,
        (512.0 + 896.0 * cb).round() as u16,
        (512.0 + 896.0 * cr).round() as u16,
    ]
}

/// HLG signal to display nits on a 1000-nit display: inverse OETF, OOTF.
fn hlg_decode(signal: [f64; 3]) -> [f64; 3] {
    let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_73);
    let scene = signal.map(|e| {
        let e = e.clamp(0.0, 1.0);
        if e <= 0.5 {
            e * e / 3.0
        } else {
            (((e - c) / a).exp() + b) / 12.0
        }
    });
    let ys = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
    scene.map(|v| PEAK * ys.max(1e-6).powf(0.2) * v)
}

/// BT.2020 to BT.709 primaries, linear: the inverse of [`bt709_to_bt2020`].
fn bt2020_to_bt709(c: [f64; 3]) -> [f64; 3] {
    [
        1.660_491 * c[0] - 0.587_641 * c[1] - 0.072_850 * c[2],
        -0.124_551 * c[0] + 1.132_900 * c[1] - 0.008_349 * c[2],
        -0.018_151 * c[0] - 0.100_579 * c[1] + 1.118_730 * c[2],
    ]
}

/// What the compositor should show for the codes the fixture holds, decoded
/// on the CPU from the standards: the reference tone map. It starts from the
/// quantised codes rather than from the SDR colour, because 10-bit PQ cannot
/// hold a saturated BT.709 primary exactly once it is in BT.2020 — pure green
/// comes back with a red of 0.3 % linear, which the sRGB curve lifts to eleven
/// code values — and that is the fixture's rounding, not the shader's.
fn expected_from_codes(kind: Hdr, codes: [u16; 3]) -> [f64; 3] {
    let y = (codes[0] as f64 - 64.0) / 876.0;
    let cb = (codes[1] as f64 - 512.0) / 896.0;
    let cr = (codes[2] as f64 - 512.0) / 896.0;
    let r = y + 1.4746 * cr;
    let b = y + 1.8814 * cb;
    let g = (y - 0.2627 * r - 0.0593 * b) / 0.6780;
    let signal = [r, g, b].map(|v| v.clamp(0.0, 1.0));
    let nits = match kind {
        Hdr::Pq => signal.map(|e| pq_decode(e) * 10_000.0),
        Hdr::Hlg => hlg_decode(signal),
    };
    let light = bt2020_to_bt709(nits.map(|v| v / REFERENCE_WHITE)).map(|v| v.max(0.0));
    let peak = light.iter().cloned().fold(0.0, f64::max);
    let ratio = if peak > 0.0 {
        reference_eetf(peak) / peak
    } else {
        1.0
    };
    light.map(|v| linear_to_srgb(v * ratio) * 255.0)
}

/// One 10-bit BT.2020 frame of the HDR patches, as yuv420p10le.
fn hdr_frame(kind: Hdr) -> Vec<u8> {
    let (width, height) = (W as usize, H as usize);
    let mut y_plane = vec![0u16; width * height];
    let mut u_plane = vec![0u16; width * height / 4];
    let mut v_plane = vec![0u16; width * height / 4];
    for (index, colour) in HDR_PATCHES.iter().enumerate() {
        let [y10, u10, v10] = hdr_codes(kind, *colour);
        let (x0, y0, pw, ph) = patch_rect(index, W, H);
        for y in y0..y0 + ph {
            for x in x0..x0 + pw {
                y_plane[y as usize * width + x as usize] = y10;
                if x % 2 == 0 && y % 2 == 0 {
                    let i = (y as usize / 2) * (width / 2) + x as usize / 2;
                    u_plane[i] = u10;
                    v_plane[i] = v10;
                }
            }
        }
    }
    let mut bytes = Vec::with_capacity(width * height * 3);
    for plane in [&y_plane, &u_plane, &v_plane] {
        for sample in plane.iter() {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    bytes
}

/// The HDR patches as lossless 10-bit HEVC, tagged.
fn hdr_clip(kind: Hdr) -> Option<PathBuf> {
    if !has_tool("ffmpeg") {
        return None;
    }
    let path = scratch(&format!("{kind:?}.mp4").to_lowercase());
    if path.exists() {
        return Some(path);
    }
    let frame = hdr_frame(kind);
    let mut input = Vec::new();
    for _ in 0..10 {
        input.extend_from_slice(&frame);
    }
    let trc = match kind {
        Hdr::Pq => "smpte2084",
        Hdr::Hlg => "arib-std-b67",
    };
    let size = format!("{W}x{H}");
    let params =
        format!("log-level=error:lossless=1:colorprim=bt2020:transfer={trc}:colormatrix=bt2020nc");
    ffmpeg_with_input(
        &[
            "-f",
            "rawvideo",
            "-pix_fmt",
            "yuv420p10le",
            "-s",
            &size,
            "-r",
            "30",
            "-i",
            "-",
            "-c:v",
            "libx265",
            "-x265-params",
            &params,
            "-color_primaries",
            "bt2020",
            "-color_trc",
            trc,
            "-colorspace",
            "bt2020nc",
            "-color_range",
            "tv",
            path.to_str().expect("utf-8 path"),
        ],
        &input,
    )
    .expect("encode the HDR patches");
    Some(path)
}

/// Check one rendered frame of the HDR patches against the reference tone
/// map of the fixture's codes, and against the SDR colours they were made
/// from. Returns the worst error against each.
fn check_hdr(kind: Hdr, rgba: &[u8], label: &str) -> (f64, f64) {
    let mut worst: f64 = 0.0;
    let mut worst_sdr: f64 = 0.0;
    for (index, colour) in HDR_PATCHES.iter().enumerate() {
        let got = patch_mean(rgba, W, H, index);
        let expected = expected_from_codes(kind, hdr_codes(kind, *colour));
        let error = max_error(&got, &expected);
        eprintln!(
            "{label} patch {index} {colour:?}: reference {expected:.1?}, ideal {:.1?}, got {got:.1?}",
            expected_sdr(*colour)
        );
        worst = worst.max(error);
        // Greys below the knee: the tone map must leave them exactly as the
        // SDR scene had them, which is what lets an HDR clip cut against an
        // SDR one.
        if colour[0] == colour[1] && colour[1] == colour[2] && colour[0] <= 160 {
            worst_sdr = worst_sdr.max(max_error(&got, &colour.map(f64::from)));
        }
    }
    eprintln!(
        "{label}: worst error {worst:.2} against the reference tone map, \
         {worst_sdr:.2} against the SDR greys"
    );
    (worst, worst_sdr)
}

/// The decode paths this machine has: software always, NVDEC and VAAPI when
/// they decode 10-bit HEVC here.
fn decode_paths() -> Vec<Acceleration> {
    let mut paths = vec![Acceleration::Software];
    for hardware in [Acceleration::Cuda, Acceleration::Vaapi] {
        let clip = hdr_clip(Hdr::Pq).expect("fixture");
        if let Ok(mut decoder) = VideoDecoder::open_with(&clip, hardware) {
            if decoder.seek_and_decode(0).is_ok() && decoder.is_hardware() {
                paths.push(hardware);
            }
        }
    }
    paths
}

#[test]
fn hdr_sources_are_tone_mapped_to_the_reference_on_every_decode_path() {
    let ctx = require_gpu!();
    if hdr_clip(Hdr::Pq).is_none() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let compositor = Compositor::new(ctx);
    for kind in [Hdr::Pq, Hdr::Hlg] {
        let clip = hdr_clip(kind).expect("fixture");
        let project = project_of(&clip, W, H);
        for path in decode_paths() {
            let sources = MediaSourceProvider::from_project_with(&project, Some(path));
            let frame = compositor
                .render(&project, 100_000, (W, H), &sources)
                .expect("render the HDR frame");
            let label = format!("{kind:?} via {path:?}");
            let (worst, worst_sdr) = check_hdr(kind, &frame.data, &label);
            assert!(
                worst <= 2.0,
                "{label}: {worst:.2} off the reference tone map"
            );
            assert!(
                worst_sdr <= 1.0,
                "{label}: {worst_sdr:.2} off the SDR greys"
            );
        }
    }
}

/// Thumbnails, proxies and the analysis passes take RGBA from the decoder
/// rather than from the compositor; an HDR clip must look the same there.
#[test]
fn the_rgba_decode_path_tone_maps_hdr_like_the_shader() {
    let ctx = require_gpu!();
    if hdr_clip(Hdr::Pq).is_none() {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    }
    let compositor = Compositor::new(ctx);
    for kind in [Hdr::Pq, Hdr::Hlg] {
        let clip = hdr_clip(kind).expect("fixture");
        let project = project_of(&clip, W, H);
        let shader = compositor
            .render(
                &project,
                0,
                (W, H),
                &MediaSourceProvider::from_project(&project),
            )
            .expect("render the HDR frame");
        let (cpu, width, height) = decode_ours(&clip);
        assert_eq!((width, height), (W, H));
        let mut worst: f64 = 0.0;
        for index in 0..HDR_PATCHES.len() {
            let a = patch_mean(&shader.data, W, H, index);
            let b = patch_mean(&cpu, W, H, index);
            eprintln!("{kind:?} patch {index}: shader {a:.1?}, decoder {b:.1?}");
            // Channels both under 16 are left out: there the sRGB curve turns
            // a tenth of a per cent of light into eight code values (the red
            // of pure green, clipped out of BT.2020), which says nothing about
            // the conversion.
            for c in 0..3 {
                if a[c] >= 16.0 || b[c] >= 16.0 {
                    worst = worst.max((a[c] - b[c]).abs());
                }
            }
        }
        eprintln!("{kind:?}: decoder against shader, worst patch error {worst:.2}");
        // Two: swscale's YUV to 16-bit RGB runs about one code value dark
        // across the board. A thumbnail can carry it.
        assert!(
            worst <= 2.0,
            "{kind:?}: the decoder's RGBA is {worst:.2} off the shader"
        );
    }
}

#[test]
fn an_hdr_source_exports_as_it_previews() {
    let ctx = require_gpu!();
    let Some(clip) = hdr_clip(Hdr::Hlg) else {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    };
    let project = project_of(&clip, W, H);
    let sources = MediaSourceProvider::from_project(&project);
    let preview = Compositor::new(ctx)
        .render(&project, 0, (W, H), &sources)
        .expect("render the preview");

    let file = export(
        &project,
        "hlg_export.mp4",
        None,
        ExportOverrides {
            quality: Some(Quality::Crf(12)),
            ..Default::default()
        },
    );
    // The export is SDR: an HDR source is tone-mapped on the way in, and
    // the file says BT.709 like any other.
    assert_eq!(probe_tags(&file), "tv,bt709,bt709,bt709");
    let (exported, width, height) = decode_ours(&file);
    assert_eq!((width, height), (W, H));
    let mut worst: f64 = 0.0;
    for index in 0..HDR_PATCHES.len() {
        let a = patch_mean(&preview.data, W, H, index);
        let b = patch_mean(&exported, W, H, index);
        worst = worst.max(max_error(&a, &b));
    }
    eprintln!("hlg preview against export: worst patch error {worst:.2}");
    assert!(worst <= 2.0, "preview and export differ by {worst:.2}");
}

#[test]
fn ten_bit_is_refused_for_h264_in_prose() {
    let Some(clip) = bars_clip(640, 480) else {
        eprintln!("skipping: ffmpeg is not on PATH");
        return;
    };
    let project = project_of(&clip, 640, 480);
    let request = ExportRequest {
        output_path: scratch("refused.mp4").to_string_lossy().into_owned(),
        preset_id: None,
        overrides: Some(ExportOverrides {
            ten_bit: true,
            ..Default::default()
        }),
        hardware: None,
        include_audio: false,
        range: None,
    };
    let error = resolve_settings(&project, &request)
        .unwrap_err()
        .to_string();
    assert!(error.contains("10-bit"), "{error}");
    assert!(error.contains("H.265"), "{error}");
}

// ---------------------------------------------------------------------------
// 16-bit planes
// ---------------------------------------------------------------------------

/// A provider that hands out one prepared planar frame.
struct OneFrame(chukcut_engine::modules::render::SourceFrame);

impl SourceProvider for OneFrame {
    fn frame(
        &self,
        _ctx: &chukcut_engine::modules::render::RenderContext,
        _request: &chukcut_engine::modules::render::SourceRequest<'_>,
    ) -> anyhow::Result<Option<chukcut_engine::modules::render::SourceFrame>> {
        Ok(Some(self.0.clone()))
    }
}

/// Two flat planes of one Y'CbCr colour, 8-bit or P010-style 16-bit.
fn flat_planes(
    ctx: &chukcut_engine::modules::render::RenderContext,
    yuv: [u8; 3],
    deep: bool,
) -> chukcut_engine::modules::render::SourceFrame {
    use chukcut_engine::modules::render::{SourceFrame, YuvMatrix, YuvRange};
    let (w, h) = (64u32, 64u32);
    let plane = |format: wgpu::TextureFormat, width: u32, height: u32, texel: &[u8]| {
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("flat plane"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let data: Vec<u8> = texel
            .iter()
            .copied()
            .cycle()
            .take(texel.len() * (width * height) as usize)
            .collect();
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(texel.len() as u32 * width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Arc::new(texture)
    };
    // A 10-bit code is the 8-bit one times four, and P010 keeps it in the top
    // ten bits: the 8-bit code times 256.
    let deep_sample = |v: u8| ((v as u16) << 8).to_le_bytes();
    let (luma, chroma) = if deep {
        let [y0, y1] = deep_sample(yuv[0]);
        let [b0, b1] = deep_sample(yuv[1]);
        let [r0, r1] = deep_sample(yuv[2]);
        (
            plane(wgpu::TextureFormat::R16Unorm, w, h, &[y0, y1]),
            plane(
                wgpu::TextureFormat::Rg16Unorm,
                w / 2,
                h / 2,
                &[b0, b1, r0, r1],
            ),
        )
    } else {
        (
            plane(wgpu::TextureFormat::R8Unorm, w, h, &[yuv[0]]),
            plane(
                wgpu::TextureFormat::Rg8Unorm,
                w / 2,
                h / 2,
                &[yuv[1], yuv[2]],
            ),
        )
    };
    SourceFrame::from_planes(luma, chroma, YuvMatrix::Bt709, YuvRange::Limited, 0, None)
}

#[test]
fn sixteen_bit_planes_render_like_the_eight_bit_ones_they_widen() {
    let ctx = require_gpu!();
    eprintln!(
        "adapter: {} ({:?}); 16-bit planes: {}",
        ctx.adapter_info().name,
        ctx.adapter_info().device_type,
        ctx.supports_deep_planes()
    );
    if !ctx.supports_deep_planes() {
        eprintln!("skipping: this device has no 16-bit textures");
        return;
    }
    use chukcut_engine::modules::project::document::VideoMaterial;
    let mut project = Project::new(
        "deep",
        CanvasConfig {
            width: 64,
            height: 64,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project.materials.videos.push(VideoMaterial {
        id: "clip".into(),
        path: "planes".into(),
        width: 64,
        height: 64,
        duration: 1_000_000,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment("clip", 0, 1_000_000));
    project.tracks.push(track);

    let compositor = Compositor::new(Arc::clone(&ctx));
    for yuv in [
        [16u8, 128, 128],
        [120, 110, 150],
        [180, 90, 170],
        [235, 128, 128],
        [81, 90, 240],
    ] {
        let narrow = compositor
            .render(
                &project,
                0,
                (64, 64),
                &OneFrame(flat_planes(&ctx, yuv, false)),
            )
            .expect("render the 8-bit planes");
        let deep = compositor
            .render(
                &project,
                0,
                (64, 64),
                &OneFrame(flat_planes(&ctx, yuv, true)),
            )
            .expect("render the 16-bit planes");
        let (a, b) = (narrow.pixel(32, 32), deep.pixel(32, 32));
        eprintln!("{yuv:?}: 8-bit {a:?}, 16-bit {b:?}");
        for c in 0..3 {
            assert!(
                (a[c] as i32 - b[c] as i32).abs() <= 1,
                "{yuv:?}: 8-bit planes give {a:?}, 16-bit planes {b:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Deep sources of awkward shapes
// ---------------------------------------------------------------------------

/// A 10-bit source made by ffmpeg from `testsrc` (RGB, so odd sizes stay
/// odd), tagged BT.709.
fn deep_clip(name: &str, size: &str, codec_args: &[&str]) -> Option<PathBuf> {
    if !has_tool("ffmpeg") {
        return None;
    }
    let path = scratch(name);
    if path.exists() {
        return Some(path);
    }
    let source = format!("testsrc=size={size}:rate=30:duration=0.4");
    let mut args = vec!["-f", "lavfi", "-i", source.as_str()];
    args.extend_from_slice(codec_args);
    args.extend_from_slice(&[
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
        path.to_str().expect("utf-8 path"),
    ]);
    ffmpeg_with_input(&args, &[]).expect("make the deep clip");
    Some(path)
}

/// The same file with a 90° display rotation in its container: what a phone
/// held upright writes. Remuxed, because an encode does not take the flag.
fn rotated(clip: Option<PathBuf>) -> Option<PathBuf> {
    let clip = clip?;
    let path = clip.with_file_name(
        clip.file_name()
            .expect("a file name")
            .to_string_lossy()
            .replace("_plain", ""),
    );
    if !path.exists() {
        ffmpeg_with_input(
            &[
                "-display_rotation",
                "90",
                "-i",
                clip.to_str().expect("utf-8 path"),
                "-c",
                "copy",
                path.to_str().expect("utf-8 path"),
            ],
            &[],
        )
        .expect("remux with a rotation");
    }
    Some(path)
}

/// 10-bit 4:2:0 sources now reach the compositor as P010 planes; 4:2:2 and
/// 4:4:4 ones stay on the RGBA path so their chroma is not halved. Odd sizes,
/// every chroma layout and a rotation flag must each render as ffmpeg decodes
/// the file.
#[test]
fn deep_sources_of_awkward_shapes_render_as_ffmpeg_decodes_them() {
    let ctx = require_gpu!();
    let cases: Vec<(&str, Option<PathBuf>, u32, u32)> = vec![
        (
            "odd 1001x777 4:4:4 FFV1",
            deep_clip(
                "odd444.mkv",
                "1001x777",
                &["-c:v", "ffv1", "-pix_fmt", "yuv444p10le"],
            ),
            1001,
            777,
        ),
        (
            "4:2:2 ProRes",
            deep_clip(
                "prores.mov",
                "640x360",
                &[
                    "-c:v",
                    "prores_ks",
                    "-profile:v",
                    "3",
                    "-pix_fmt",
                    "yuv422p10le",
                ],
            ),
            640,
            360,
        ),
        (
            "4:2:0 FFV1",
            deep_clip(
                "420.mkv",
                "640x360",
                &["-c:v", "ffv1", "-pix_fmt", "yuv420p10le"],
            ),
            640,
            360,
        ),
        (
            "8-bit 4:2:0 FFV1, the RGBA path, for comparison",
            deep_clip(
                "420_8bit.mkv",
                "640x360",
                &["-c:v", "ffv1", "-pix_fmt", "yuv420p"],
            ),
            640,
            360,
        ),
        (
            "rotated H.264 8-bit, the RGBA path",
            rotated(deep_clip(
                "rotated8_plain.mp4",
                "640x360",
                &["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv420p"],
            )),
            360,
            640,
        ),
        (
            "rotated H.264 High 10",
            rotated(deep_clip(
                "rotated_plain.mp4",
                "640x360",
                &["-c:v", "libx264", "-qp", "0", "-pix_fmt", "yuv420p10le"],
            )),
            360,
            640,
        ),
    ];
    let compositor = Compositor::new(ctx);
    for (label, clip, width, height) in cases {
        let Some(clip) = clip else {
            eprintln!("skipping: ffmpeg is not on PATH");
            return;
        };
        let project = project_of(&clip, width, height);
        let sources =
            MediaSourceProvider::from_project_with(&project, Some(Acceleration::Software));
        let frame = compositor
            .render(&project, 0, (width, height), &sources)
            .expect("render the deep clip");
        // ffmpeg applies the rotation flag itself (autorotate).
        let reference = decode_ffmpeg(&clip, width, height);
        let mut sum = 0.0f64;
        let mut squares = 0.0f64;
        let mut n = 0.0f64;
        for (a, b) in frame
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .zip(reference.as_chunks::<4>().0)
        {
            for c in 0..3 {
                let d = a[c] as f64 - b[c] as f64;
                sum += d.abs();
                squares += d * d;
                n += 1.0;
            }
        }
        let mean = sum / n;
        let psnr = 10.0 * (255.0f64 * 255.0 / (squares / n).max(1e-9)).log10();
        eprintln!("{label}: mean difference {mean:.2}, {psnr:.1} dB");
        // The 4:2:0 deep sources are drawn from planes, and the shader's
        // bilinear chroma differs from swscale's at hard colour edges —
        // testsrc is nothing but hard colour edges. That is the same
        // difference NVDEC and VAAPI frames have always had (about 1.3 mean
        // on real footage, `STATUS.md` "Hardware decode through the
        // compositor"). Everything else is the RGBA path, which is swscale's
        // own conversion and agrees exactly.
        let planar = label.contains("High 10") || label == "4:2:0 FFV1";
        if planar {
            assert!(
                mean < 1.5 && psnr > 30.0,
                "{label}: {mean:.2} mean, {psnr:.1} dB"
            );
        } else {
            assert!(psnr > 60.0, "{label}: {mean:.2} mean, {psnr:.1} dB");
        }
    }
}
