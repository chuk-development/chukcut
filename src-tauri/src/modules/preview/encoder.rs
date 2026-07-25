//! RGBA8 to JPEG.
//!
//! This is the step that makes the whole approach viable: a 960x540 frame is
//! 2 MB of RGBA and about 120 KB of JPEG, which turns 62 MB/s of preview into
//! 3.6 MB/s the webview can fetch like any other image.
//!
//! No colour conversion happens here. The compositor renders into an
//! `Rgba8UnormSrgb` target, so the bytes it hands back are already sRGB-encoded
//! — exactly what a JPEG file is defined to contain. Converting again would
//! double-apply the transfer function and wash the picture out.
//!
//! Encode time is logged at debug level because this runs on the critical path
//! once per frame, and "the preview is choppy" has two possible causes —
//! rendering and encoding — that are indistinguishable without the number.

use std::time::Instant;

use jpeg_encoder::{ColorType, Encoder};

use super::error::{PreviewError, Result};

/// Bytes per RGBA8 pixel.
const BYTES_PER_PIXEL: usize = 4;

/// Encode one frame.
///
/// `rgba` must be tightly packed — `width * height * 4` bytes with no row
/// padding, which is what `Compositor::render_frame` returns. The alpha channel
/// is discarded; JPEG has no alpha and the preview composites onto the
/// project's background anyway.
pub fn encode_jpeg(rgba: &[u8], width: u32, height: u32, quality: u8) -> Result<Vec<u8>> {
    // JPEG dimensions are 16-bit in the file format, which no proxy resolution
    // will ever approach, but the cast has to be checked somewhere.
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

    let started = Instant::now();
    // Sized from the pixel count rather than grown from empty: a preview frame
    // lands within a factor of two of this, so the encoder writes into one
    // allocation instead of a dozen.
    let mut out = Vec::with_capacity(want / 8);
    let encoder = Encoder::new(&mut out, quality.clamp(1, 100));
    encoder
        .encode(rgba, width as u16, height as u16, ColorType::Rgba)
        .map_err(|e| PreviewError::Encode(e.to_string()))?;

    tracing::debug!(
        width,
        height,
        quality,
        bytes = out.len(),
        micros = started.elapsed().as_micros() as u64,
        "encoded preview frame"
    );

    Ok(out)
}

/// Whether a byte slice starts with the JPEG start-of-image marker.
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8])
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
        // chroma-subsampled encoder is where they usually break.
        let out = encode_jpeg(&gradient(101, 37), 101, 37, 80).expect("encode");
        assert!(is_jpeg(&out));
    }

    #[test]
    fn a_short_buffer_is_an_error_rather_than_a_panic() {
        let err = encode_jpeg(&[0; 16], 64, 48, 80).unwrap_err();
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
}
