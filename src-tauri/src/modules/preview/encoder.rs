//! RGBA8 to JPEG, on the GPU where there is one.
//!
//! This is the step that makes the whole approach viable: a 960x540 frame is
//! 2 MB of RGBA and about 120 KB of JPEG, which turns 62 MB/s of preview into
//! 3.6 MB/s the webview can fetch like any other image. It is also, at native
//! resolution, the step that used to blow the frame budget on its own — the
//! reason playback ran at a reduced size for a while.
//!
//! Two backends, and the caller does not choose between them:
//!
//! - [`vaapi::VaapiJpegEncoder`] — the fixed-function JPEG encoder in the iGPU,
//!   reached through FFmpeg's `mjpeg_vaapi`. What the preview uses when the
//!   machine has one.
//! - libjpeg-turbo, through the `turbojpeg` crate — SIMD, and the fallback for
//!   every machine and every frame size the hardware refuses. It replaced the
//!   pure-Rust `jpeg-encoder`, which was 3-4x slower.
//!
//! [`encode_preview_jpeg`] tries hardware and silently uses software when it
//! cannot. The fallback is not a rare path to be discovered in production: an
//! odd frame size takes it on every machine, because NV12 cannot represent one.
//!
//! ## Colour
//!
//! No colour conversion happens on the software path. The compositor renders
//! into an `Rgba8UnormSrgb` target, so the bytes it hands back are already
//! sRGB-encoded — exactly what a JPEG file is defined to contain. Converting
//! again would double-apply the transfer function and wash the picture out.
//!
//! The hardware path *does* convert, because the encoder eats NV12, and it
//! converts to **full**-range BT.601 so the result matches. Getting that wrong
//! is the trap documented at the top of [`vaapi`].
//!
//! ## The measurement
//!
//! Encode time is logged at debug level because this runs on the critical path
//! once per frame, and "the preview is choppy" has two possible causes —
//! rendering and encoding — that are indistinguishable without the number.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use parking_lot::Mutex;
use turbojpeg::{Compressor, Image, PixelFormat, Subsamp};

use super::error::{PreviewError, Result};
use super::vaapi::{size_is_encodable, VaapiJpegEncoder};

/// Bytes per RGBA8 pixel.
const BYTES_PER_PIXEL: usize = 4;

/// Which encoder produced a frame. Reported so the logs say whether the
/// hardware path is actually being taken, which is not otherwise visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Vaapi,
    Software,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Vaapi => "vaapi",
            Backend::Software => "libjpeg-turbo",
        }
    }
}

/// Env var that pins the backend, for benchmarking and for a user whose driver
/// turns out to lie: `software`, `hardware`, or unset for "hardware if it
/// works". `hardware` makes a failure loud instead of silently slow, which is
/// the only way to tell the two apart from the outside.
pub const BACKEND_ENV: &str = "CHUKCUT_PREVIEW_JPEG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Preference {
    Auto,
    ForceSoftware,
    ForceHardware,
}

/// What an unset [`BACKEND_ENV`] means.
///
/// **Software, for now, and this is a deliberate step back from a measured
/// win.** The hardware encoder is genuinely faster — 6.2 ms against 10.2 ms at
/// 1080×1920 — but on an otherwise idle machine it makes the preview *stop*:
///
/// ```text
/// cargo test -j 4 --lib preview::server                          → 30 s, 2 failed
/// CHUKCUT_PREVIEW_JPEG=software cargo test -j 4 --lib preview::server → 0.15 s, 1 failed
/// ```
///
/// In the failing run the renderer produced five frames and then made no
/// further progress for thirty seconds with `playing=true` and the playhead at
/// frame 0 — a stall, not slowness. What distinguishes it from the benchmark
/// that measured 6.2 ms is concurrency: encoding was moved off the render
/// thread onto a worker pool, so several encodes now run at once against one
/// non-reentrant hardware encoder. The benchmark encodes one frame at a time
/// and cannot see it.
///
/// Four milliseconds of a 9.4 ms frame is not worth a preview that can hang, so
/// the default reverts until the interaction is understood. `CHUKCUT_PREVIEW_JPEG=hardware`
/// turns it back on for whoever picks this up; the thing to establish is whether
/// the encoder needs serialising, a pool of its own, or to go back on the render
/// thread now that it is fast enough to belong there.
const DEFAULT_PREFERENCE: Preference = Preference::ForceSoftware;

fn preference() -> Preference {
    static CACHED: std::sync::OnceLock<Preference> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| match std::env::var(BACKEND_ENV).ok().as_deref() {
        Some("software") | Some("cpu") => Preference::ForceSoftware,
        Some("hardware") | Some("vaapi") | Some("gpu") => Preference::ForceHardware,
        Some("auto") => Preference::Auto,
        _ => DEFAULT_PREFERENCE,
    })
}

// ---------------------------------------------------------------------------
// The hardware encoder, and giving up on it
// ---------------------------------------------------------------------------

/// How many consecutive hardware failures it takes to stop trying.
///
/// Not one: a single `EAGAIN` from a busy driver is worth retrying. Not
/// unbounded either — a machine where the encode always fails would otherwise
/// pay the cost of opening a device on every frame forever, on top of the
/// software encode it ends up doing anyway.
const FAILURES_BEFORE_GIVING_UP: u32 = 3;

/// The one hardware encoder in the process.
///
/// One, not one per thread: encodes run on rayon workers and up to
/// `MAX_PENDING_ENCODES` of them can be in flight, but a VAAPI JPEG encode is
/// a few milliseconds and the driver serialises submissions anyway. A mutex is
/// cheaper than N devices, N surface pools and N codec contexts — and
/// `export::hwframes` already opens a device of its own, so the count matters.
///
/// **The lock is held across a rayon parallel loop**, because
/// `VaapiJpegEncoder::encode` converts the frame with one. That is a shape
/// worth being deliberate about:
///
/// - It cannot deadlock. A worker blocked here is blocked on a plain mutex and
///   is not holding a piece of the inner loop's work — nothing under
///   `rgba_to_nv12` takes a lock — so the thread holding the lock always makes
///   progress, running the conversion itself if no worker can be stolen from.
///   `tests::concurrent_encodes_all_finish` exercises it.
/// - It can lose the conversion's parallelism under contention, which costs a
///   couple of milliseconds. That is not worth restructuring for: with one
///   hardware encoder, contention means queueing whatever the lock covers, so
///   throughput is the same either way.
struct Hardware {
    /// Rebuilt when the size or the quality changes, because `avcodec_open2`
    /// reads `global_quality` and cannot be repeated on the same context.
    encoder: Option<VaapiJpegEncoder>,
    failures: u32,
    /// Set once the hardware has been written off for this process run.
    disabled: bool,
}

static HARDWARE: std::sync::OnceLock<Mutex<Hardware>> = std::sync::OnceLock::new();

fn hardware() -> &'static Mutex<Hardware> {
    HARDWARE.get_or_init(|| {
        Mutex::new(Hardware {
            encoder: None,
            failures: 0,
            disabled: false,
        })
    })
}

/// How many times the encoder has had to be rebuilt. Rebuilding costs a device
/// open, so a number that climbs with the frame count means something upstream
/// is changing the size or the quality every frame.
static REBUILDS: AtomicU64 = AtomicU64::new(0);

pub fn hardware_rebuild_count() -> u64 {
    REBUILDS.load(Ordering::Relaxed)
}

/// Whether the hardware JPEG encoder can be opened on this machine.
///
/// Answered by opening one, in the style of `export::hwaccel`: a codec being
/// present in the FFmpeg build says nothing about whether the driver
/// implements the entrypoint. The answer is cached by the encoder slot itself,
/// so calling this is cheap after the first time.
pub fn hardware_available() -> bool {
    if preference() == Preference::ForceSoftware {
        return false;
    }
    let mut hw = hardware().lock();
    if hw.disabled {
        return false;
    }
    if hw.encoder.is_some() {
        return true;
    }
    match VaapiJpegEncoder::open(64, 64, 80) {
        Ok(encoder) => {
            hw.encoder = Some(encoder);
            true
        }
        Err(error) => {
            tracing::info!(%error, "no hardware JPEG encoder; previews encode on the CPU");
            hw.disabled = true;
            false
        }
    }
}

/// Throw away the hardware encoder, so the next frame reopens it. Tests use
/// this to keep from measuring each other's state.
pub fn reset_hardware() {
    let mut hw = hardware().lock();
    hw.encoder = None;
    hw.failures = 0;
    hw.disabled = false;
}

fn encode_hardware(rgba: &[u8], width: u32, height: u32, quality: u8) -> Option<Vec<u8>> {
    if preference() == Preference::ForceSoftware || !size_is_encodable(width, height) {
        return None;
    }

    let mut hw = hardware().lock();
    if hw.disabled {
        return None;
    }

    if !hw.encoder.as_ref().is_some_and(|e| e.matches(width, height, quality)) {
        if hw.encoder.is_some() {
            REBUILDS.fetch_add(1, Ordering::Relaxed);
        }
        // Dropped before the new one is opened rather than after: two live
        // frame pools at 1080x1920 is 25 MB for no reason, and the old device
        // is of no use to the new encoder.
        hw.encoder = None;
        match VaapiJpegEncoder::open(width, height, quality) {
            Ok(encoder) => hw.encoder = Some(encoder),
            Err(error) => {
                hw.failures += 1;
                if hw.failures >= FAILURES_BEFORE_GIVING_UP {
                    hw.disabled = true;
                    tracing::warn!(%error, "giving up on the hardware JPEG encoder");
                } else {
                    tracing::debug!(%error, width, height, "cannot open the hardware JPEG encoder");
                }
                return None;
            }
        }
    }

    let encoder = hw.encoder.as_mut()?;
    match encoder.encode(rgba) {
        Ok(bytes) => {
            hw.failures = 0;
            Some(bytes)
        }
        Err(error) => {
            hw.failures += 1;
            hw.encoder = None;
            if hw.failures >= FAILURES_BEFORE_GIVING_UP {
                hw.disabled = true;
                tracing::warn!(%error, "giving up on the hardware JPEG encoder");
            } else {
                tracing::debug!(%error, "hardware JPEG encode failed; using the CPU");
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// The public surface
// ---------------------------------------------------------------------------

/// Encode one frame, on whichever encoder this machine has.
///
/// `rgba` must be tightly packed — `width * height * 4` bytes with no row
/// padding, which is what `Compositor::render_frame` returns. The alpha channel
/// is discarded; JPEG has no alpha and the preview composites onto the
/// project's background anyway.
pub fn encode_preview_jpeg(
    rgba: &[u8],
    width: u32,
    height: u32,
    quality: u8,
) -> Result<(Vec<u8>, Backend)> {
    check(rgba, width, height)?;

    if let Some(bytes) = encode_hardware(rgba, width, height, quality) {
        return Ok((bytes, Backend::Vaapi));
    }
    if preference() == Preference::ForceHardware {
        return Err(PreviewError::Encode(format!(
            "{BACKEND_ENV}=hardware, but the hardware JPEG encoder is not usable \
             for a {width}x{height} frame"
        )));
    }
    Ok((encode_jpeg(rgba, width, height, quality)?, Backend::Software))
}

/// Encode one frame on the CPU, with libjpeg-turbo.
///
/// Kept separate from [`encode_preview_jpeg`] because it is the reference the
/// hardware output is compared against, and because the thumbnail and test
/// paths want a result that does not depend on what GPU is present.
pub fn encode_jpeg(rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>> {
    check(rgba, width, height)?;

    let started = Instant::now();
    let image = Image {
        pixels: rgba,
        width: width as usize,
        // Tightly packed, as promised above.
        pitch: width as usize * BYTES_PER_PIXEL,
        height: height as usize,
        format: PixelFormat::RGBA,
    };

    let mut compressor =
        Compressor::new().map_err(|e| PreviewError::Encode(format!("libjpeg-turbo: {e}")))?;
    compressor
        .set_quality(quality.clamp(1, 100) as i32)
        .map_err(|e| PreviewError::Encode(format!("libjpeg-turbo quality: {e}")))?;
    // 4:2:0, matching what the hardware encoder produces and what every
    // consumer of a preview frame expects. Left at libjpeg's default it would
    // be 4:4:4, which is a third more bytes for detail nobody is looking at in
    // a moving picture.
    compressor
        .set_subsamp(Subsamp::Sub2x2)
        .map_err(|e| PreviewError::Encode(format!("libjpeg-turbo subsampling: {e}")))?;

    let out = compressor
        .compress_to_vec(image)
        .map_err(|e| PreviewError::Encode(format!("libjpeg-turbo: {e}")))?;

    tracing::debug!(
        width,
        height,
        quality,
        bytes = out.len(),
        micros = started.elapsed().as_micros() as u64,
        "encoded preview frame on the CPU"
    );

    Ok(out)
}

fn check(rgba: &[u8], width: u32, height: u32) -> Result<()> {
    // JPEG dimensions are 16-bit in the file format, which no preview
    // resolution will ever approach, but the cast has to be checked somewhere.
    if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
        return Err(PreviewError::FrameSize { width, height });
    }
    let want = width as usize * height as usize * BYTES_PER_PIXEL;
    if rgba.len() < want {
        return Err(PreviewError::FrameData {
            got: rgba.len(),
            want,
        });
    }
    Ok(())
}

/// Whether a byte slice starts with the JPEG start-of-image marker.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
}

/// Decode a JPEG back to RGB, for tests and for the hardware/software
/// comparison. Not on any hot path.
pub fn decode_jpeg_rgb(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let image: turbojpeg::Image<Vec<u8>> = turbojpeg::decompress(bytes, PixelFormat::RGB)
        .map_err(|e| PreviewError::Encode(format!("cannot decode the JPEG back: {e}")))?;
    Ok((image.pixels, image.width as u32, image.height as u32))
}

/// Peak signal-to-noise ratio between two RGB buffers, in dB.
///
/// The same measure `export/` uses to check that the hardware video encoder is
/// producing the same picture as the software one. `f64::INFINITY` for
/// identical inputs.
pub fn psnr_rgb(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return f64::INFINITY;
    }
    let mut sum = 0.0f64;
    for i in 0..n {
        let d = a[i] as f64 - b[i] as f64;
        sum += d * d;
    }
    let mse = sum / n as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[
                    (x % 256) as u8,
                    (y % 256) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]);
            }
        }
        data
    }

    #[test]
    fn encodes_a_frame_to_something_a_browser_would_accept() {
        let out = encode_jpeg(&gradient(64, 48), 64, 48, 80).expect("encode");
        assert!(is_jpeg(&out), "no SOI marker");
        assert!(out.ends_with(&[0xFF, 0xD9]), "no EOI marker");
        assert!(out.len() > 100);
    }

    #[test]
    fn lower_quality_produces_fewer_bytes() {
        let frame = gradient(128, 128);
        let high = encode_jpeg(&frame, 128, 128, 90).unwrap();
        let low = encode_jpeg(&frame, 128, 128, 30).unwrap();
        assert!(low.len() < high.len(), "{} vs {}", low.len(), high.len());
    }

    #[test]
    fn odd_dimensions_encode() {
        // Proxy sizes are derived by rounding, so odd edges are routine and a
        // chroma-subsampled encoder is where they usually break. This is also
        // the case the hardware path cannot take at all, so it exercises the
        // fallback on a machine that has a GPU.
        let out = encode_preview_jpeg(&gradient(101, 37), 101, 37, 80).expect("encode");
        assert!(is_jpeg(&out.0));
        assert_eq!(out.1, Backend::Software, "NV12 has no odd sizes");
    }

    #[test]
    fn a_short_buffer_is_an_error_rather_than_a_panic() {
        let err = encode_jpeg(&[0; 16], 64, 48, 80).unwrap_err();
        assert!(matches!(err, PreviewError::FrameData { .. }), "{err}");
        let err = encode_preview_jpeg(&[0; 16], 64, 48, 80).unwrap_err();
        assert!(matches!(err, PreviewError::FrameData { .. }), "{err}");
    }

    #[test]
    fn zero_and_oversized_frames_are_refused() {
        assert!(matches!(
            encode_jpeg(&[], 0, 10, 80).unwrap_err(),
            PreviewError::FrameSize { .. }
        ));
        assert!(matches!(
            encode_jpeg(&[], 70_000, 10, 80).unwrap_err(),
            PreviewError::FrameSize { .. }
        ));
    }

    #[test]
    fn quality_outside_the_valid_range_is_clamped_rather_than_rejected() {
        assert!(encode_jpeg(&gradient(16, 16), 16, 16, 0).is_ok());
        assert!(encode_jpeg(&gradient(16, 16), 16, 16, 255).is_ok());
    }

    #[test]
    fn a_frame_survives_the_round_trip() {
        // The encoder is only useful if what comes out decodes to roughly what
        // went in — the check that catches a swapped colour plane or a wrong
        // stride, both of which still produce a valid JPEG.
        let rgba = gradient(64, 64);
        let jpeg = encode_jpeg(&rgba, 64, 64, 95).unwrap();
        let (rgb, w, h) = decode_jpeg_rgb(&jpeg).unwrap();
        assert_eq!((w, h), (64, 64));

        let mut source = Vec::with_capacity(rgb.len());
        for px in rgba.chunks_exact(4) {
            source.extend_from_slice(&px[..3]);
        }
        let db = psnr_rgb(&source, &rgb);
        assert!(db > 30.0, "round trip scored only {db:.1} dB");
    }

    #[test]
    fn psnr_is_infinite_for_identical_buffers_and_finite_otherwise() {
        assert!(psnr_rgb(&[1, 2, 3], &[1, 2, 3]).is_infinite());
        assert!(psnr_rgb(&[0, 0, 0], &[255, 255, 255]) < 1.0);
    }

    /// Serialises the tests that reach for the one hardware encoder.
    ///
    /// `cargo test` runs these on several threads, and the hardware slot is a
    /// process-wide static: without this, one test's `reset_hardware` lands in
    /// the middle of another's encode and both spend their time reopening a
    /// VAAPI device instead of measuring anything. It also keeps them from
    /// fighting the preview server's own render thread in `server::tests`.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Several threads encoding at once, which is what the server does.
    ///
    /// Worth a test rather than an argument, because the hardware path holds a
    /// process-wide mutex and runs a rayon parallel loop *inside* it. That is a
    /// shape which deadlocks if a worker blocked on the mutex is holding a
    /// piece of the inner loop's work — it cannot here, because the closure
    /// under `rgba_to_nv12` never takes a lock, but "cannot" is worth checking
    /// with the real scheduler.
    #[test]
    fn concurrent_encodes_all_finish() {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        reset_hardware();
        let rgba = std::sync::Arc::new(gradient(256, 192));
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let rgba = std::sync::Arc::clone(&rgba);
                std::thread::spawn(move || {
                    // Alternating quality, so the encoder is rebuilt underneath
                    // the contention as well.
                    let quality = if i % 2 == 0 { 88 } else { 94 };
                    for _ in 0..6 {
                        let (bytes, _) =
                            encode_preview_jpeg(&rgba, 256, 192, quality).expect("encode");
                        assert!(is_jpeg(&bytes));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("a thread panicked or deadlocked");
        }
    }

    /// The point of the whole exercise: the hardware encoder has to produce
    /// the *same picture*, not merely a valid file. Skips where there is no
    /// device.
    #[test]
    fn the_hardware_encoder_matches_the_software_one() {
        let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        reset_hardware();
        if !hardware_available() {
            return;
        }
        // Even dimensions, so the hardware path is actually taken.
        let (w, h) = (256u32, 192u32);
        let rgba = gradient(w, h);

        let (hw, backend) = encode_preview_jpeg(&rgba, w, h, 90).expect("hardware encode");
        if backend != Backend::Vaapi {
            return;
        }
        let sw = encode_jpeg(&rgba, w, h, 90).expect("software encode");

        let (hw_rgb, hw_w, hw_h) = decode_jpeg_rgb(&hw).expect("decode the hardware JPEG");
        let (sw_rgb, sw_w, sw_h) = decode_jpeg_rgb(&sw).expect("decode the software JPEG");
        assert_eq!((hw_w, hw_h), (w, h));
        assert_eq!((sw_w, sw_h), (w, h));

        // 30 dB is the floor for "the same picture, differently quantised".
        // A limited-range mismatch — the trap this file's header warns about —
        // lands around 20 dB, and a swapped chroma plane lower still.
        let db = psnr_rgb(&sw_rgb, &hw_rgb);
        assert!(
            db > 30.0,
            "the hardware JPEG differs from the software one at {db:.1} dB, \
             which is not the same picture"
        );
    }
}
