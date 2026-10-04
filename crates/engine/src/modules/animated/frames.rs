//! Animated GIF and WebP, decoded whole with the `image` crate.
//!
//! Both formats store frames that are composited onto the previous ones
//! (disposal, blending); the decoder hands back each frame already composed
//! as full RGBA, so what is kept is exactly what a viewer would show.

use std::io::BufReader;
use std::path::Path;

use image::{AnimationDecoder, RgbaImage};

use super::Format;
use crate::modules::project::document::Micros;

/// How much decoded picture one file may hold. Above it the file is refused
/// and drawn as its first frame; a sticker is small, a screen recording saved
/// as a GIF is not a sticker.
pub const MAX_BYTES: usize = 384 * 1024 * 1024;

/// What browsers do with a GIF frame of 0–10 ms: show it for 100 ms. Files in
/// the wild rely on it.
const SHORTEST_FRAME: Micros = 20_000;
const DEFAULT_FRAME: Micros = 100_000;

pub struct FrameAnimation {
    pub format: Format,
    pub frames: Vec<RgbaImage>,
    /// When each frame starts, from 0; ascending.
    pub starts: Vec<Micros>,
    /// One pass of all frames.
    pub duration: Micros,
}

impl FrameAnimation {
    pub fn open(path: &Path, format: Format) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let reader = BufReader::new(file);
        let decoded = match format {
            Format::Gif => image::codecs::gif::GifDecoder::new(reader)
                .map_err(|e| e.to_string())?
                .into_frames(),
            Format::WebP => {
                let decoder =
                    image::codecs::webp::WebPDecoder::new(reader).map_err(|e| e.to_string())?;
                if !decoder.has_animation() {
                    return Err("a single frame: a still picture".into());
                }
                decoder.into_frames()
            }
            Format::Lottie => return Err("not a GIF or WebP".into()),
        };
        let mut frames = Vec::new();
        let mut starts = Vec::new();
        let mut at: Micros = 0;
        let mut bytes = 0usize;
        for frame in decoded {
            let frame = frame.map_err(|e| e.to_string())?;
            let (numer, denom) = frame.delay().numer_denom_ms();
            let lasts = (numer as i64 * 1000) / (denom.max(1) as i64);
            let lasts = if lasts < SHORTEST_FRAME {
                DEFAULT_FRAME
            } else {
                lasts
            };
            let buffer = frame.into_buffer();
            bytes += buffer.as_raw().len();
            if bytes > MAX_BYTES {
                return Err(format!(
                    "{} decodes to more than {} MB of frames; too large for a sticker",
                    path.display(),
                    MAX_BYTES / (1024 * 1024)
                ));
            }
            starts.push(at);
            frames.push(buffer);
            at += lasts;
        }
        if frames.is_empty() {
            return Err("no frames".into());
        }
        Ok(Self {
            format,
            frames,
            starts,
            duration: at,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.frames[0].dimensions()
    }

    /// The frame on screen at `time` (`0..duration`; clamped).
    pub fn index_at(&self, time: Micros) -> usize {
        match self.starts.binary_search(&time.max(0)) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
        .min(self.frames.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba};

    /// A three-frame GIF: red 50 ms, green 100 ms, blue with a 0 ms delay.
    pub(crate) fn write_gif(path: &Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = GifEncoder::new(file);
        for (colour, ms) in [
            ([255, 0, 0, 255], 50),
            ([0, 255, 0, 255], 100),
            ([0, 0, 255, 255], 0),
        ] {
            let image = RgbaImage::from_pixel(8, 6, Rgba(colour));
            encoder
                .encode_frame(Frame::from_parts(
                    image,
                    0,
                    0,
                    Delay::from_numer_denom_ms(ms, 1),
                ))
                .unwrap();
        }
    }

    #[test]
    fn frames_keep_their_own_durations_and_zero_delays_become_a_tenth() {
        let dir = super::super::test_dir("frames");
        let path = dir.join("three.gif");
        write_gif(&path);
        let animation = FrameAnimation::open(&path, Format::Gif).unwrap();
        assert_eq!(animation.frames.len(), 3);
        assert_eq!(animation.starts, vec![0, 50_000, 150_000]);
        assert_eq!(animation.duration, 250_000);
        assert_eq!(animation.size(), (8, 6));
        assert_eq!(animation.index_at(0), 0);
        assert_eq!(animation.index_at(49_999), 0);
        assert_eq!(animation.index_at(50_000), 1);
        assert_eq!(animation.index_at(249_999), 2);
        assert_eq!(animation.frames[1].get_pixel(3, 3).0, [0, 255, 0, 255]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
