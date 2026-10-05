//! The colour an export writes, and the tags that say so.
//!
//! ## What was wrong
//!
//! Until this module existed an export converted RGB to YUV with BT.601
//! coefficients — swscale's default, and what `yuv.wgsl`'s forward direction
//! hard-coded — and wrote no colour tags at all. Every player reads an
//! untagged HD stream as BT.709, so saturated colours came out with a
//! consistent hue shift: reds and greens up to ten code values off, the kind
//! of error that looks like taste rather than a fault and survives every review
//! until a delivery check rejects it.
//!
//! ## What an export writes now
//!
//! - **The matrix** is BT.709 for anything larger than standard definition
//!   and BT.601 for SD, which is the same rule FFmpeg's `scale` filter and
//!   every player apply to an untagged file — so even a player that ignores
//!   the tags shows the right colours. The user can force either.
//! - **The range** is limited (16..235) unless the user asks for full.
//! - **The primaries and transfer** are always BT.709. The compositor works in
//!   BT.709 primaries with an sRGB-encoded target, and an SD export does not
//!   gamut-convert, so tagging SD primaries as SMPTE 170M would make a careful
//!   player (mpv) convert a gamut nobody changed. The matrix is the only thing
//!   that differs between an SD and an HD export.
//! - **The tags** go on the codec context before it opens. libx264, libx265,
//!   NVENC, VAAPI and QSV all copy them into the bitstream's VUI from there, and
//!   `avcodec_parameters_from_context` copies them onto the stream, from where
//!   the MP4 and MOV muxers write a `colr` atom and Matroska its `Colour`
//!   element.
//!
//! HDR output (PQ or HLG, BT.2020) is not offered: the compositor's target is
//! 8-bit sRGB, so there is no HDR picture to write. See decision 0034.

use serde::{Deserialize, Serialize};

use ffmpeg_next as ffmpeg;

use crate::modules::render::{YuvEncoding, YuvMatrix, YuvRange};

/// Which matrix the user wants, before it is resolved against the frame size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMatrix {
    /// BT.709 above standard definition, BT.601 at or below it.
    #[default]
    Auto,
    Bt709,
    Bt601,
}

/// Which range the user wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorRange {
    /// 16..235 luma: what every delivery platform and player expects.
    #[default]
    Limited,
    /// 0..255. Legal and tagged, but some players and most web pipelines
    /// still read it as limited, so it is a choice and never a default.
    Full,
}

/// The largest frame that is still standard definition, as (long side, short
/// side): 576 lines (PAL), and 1024 columns for an anamorphic widescreen frame
/// stored square. Anything larger is HD and gets BT.709 — the rule FFmpeg's
/// own conversion falls back on, which is what makes "auto" agree with a
/// player that never reads the tags. Measured on the sides rather than on
/// width and height, so a vertical 480p export is SD like a landscape one.
const SD_MAX: (u32, u32) = (1024, 576);

/// What an export writes, resolved: the YUV encoding the frames are converted
/// into, and whether the samples are 10-bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputColour {
    pub encoding: YuvEncoding,
    /// Ten bits a sample (`yuv420p10le` / `P010`). Only H.265 and AV1 take it;
    /// `job::resolve_settings` refuses it for the others in prose.
    pub ten_bit: bool,
}

impl Default for OutputColour {
    /// BT.709, limited, 8-bit: what an HD export writes.
    fn default() -> Self {
        Self {
            encoding: YuvEncoding::BT709_LIMITED,
            ten_bit: false,
        }
    }
}

impl OutputColour {
    /// Resolve the user's choice against the output size.
    pub fn resolve(
        matrix: ColorMatrix,
        range: ColorRange,
        ten_bit: bool,
        (width, height): (u32, u32),
    ) -> Self {
        let matrix = match matrix {
            ColorMatrix::Bt709 => YuvMatrix::Bt709,
            ColorMatrix::Bt601 => YuvMatrix::Bt601,
            ColorMatrix::Auto if is_standard_definition(width, height) => YuvMatrix::Bt601,
            ColorMatrix::Auto => YuvMatrix::Bt709,
        };
        let range = match range {
            ColorRange::Limited => YuvRange::Limited,
            ColorRange::Full => YuvRange::Full,
        };
        Self {
            encoding: YuvEncoding { matrix, range },
            ten_bit,
        }
    }

    /// The automatic choice for a frame size, 8-bit and limited range.
    pub fn for_size(width: u32, height: u32) -> Self {
        Self::resolve(
            ColorMatrix::Auto,
            ColorRange::Limited,
            false,
            (width, height),
        )
    }

    /// The `colorspace` tag: which matrix the samples are in.
    pub fn space(&self) -> ffmpeg::color::Space {
        match self.encoding.matrix {
            // SMPTE 170M rather than BT.470BG: the two are the same matrix,
            // and 170M is what x264 and every analysis tool print for "601".
            YuvMatrix::Bt601 => ffmpeg::color::Space::SMPTE170M,
            YuvMatrix::Bt709 => ffmpeg::color::Space::BT709,
            YuvMatrix::Bt2020 => ffmpeg::color::Space::BT2020NCL,
        }
    }

    /// The `color_primaries` tag. Always BT.709; see the module docs.
    pub fn primaries(&self) -> ffmpeg::color::Primaries {
        ffmpeg::color::Primaries::BT709
    }

    /// The `color_trc` tag. Always BT.709; see the module docs.
    pub fn transfer(&self) -> ffmpeg::color::TransferCharacteristic {
        ffmpeg::color::TransferCharacteristic::BT709
    }

    /// The `color_range` tag.
    pub fn range(&self) -> ffmpeg::color::Range {
        match self.encoding.range {
            YuvRange::Limited => ffmpeg::color::Range::MPEG,
            YuvRange::Full => ffmpeg::color::Range::JPEG,
        }
    }

    /// The `SWS_CS_*` constant `sws_getCoefficients` takes for this matrix.
    ///
    /// Those constants are numerically the `AVColorSpace` values they name
    /// (`SWS_CS_ITU709` is 1 and so is `AVCOL_SPC_BT709`; `SWS_CS_SMPTE170M`
    /// is 5 and so is `AVCOL_SPC_BT470BG`), which `media::decoder` relies on
    /// from the other side. Spelled out here because 170M's own number, 6,
    /// is *not* an `SWS_CS_*` and would silently fall back to the default.
    pub fn sws_coefficients(&self) -> i32 {
        match self.encoding.matrix {
            YuvMatrix::Bt601 => 5,
            YuvMatrix::Bt709 => 1,
            YuvMatrix::Bt2020 => 9,
        }
    }

    /// A short label for the export log: `bt709 limited 8-bit`.
    pub fn label(&self) -> String {
        let matrix = match self.encoding.matrix {
            YuvMatrix::Bt601 => "bt601",
            YuvMatrix::Bt709 => "bt709",
            YuvMatrix::Bt2020 => "bt2020",
        };
        let range = match self.encoding.range {
            YuvRange::Limited => "limited",
            YuvRange::Full => "full",
        };
        let depth = if self.ten_bit { 10 } else { 8 };
        format!("{matrix} {range} {depth}-bit")
    }
}

fn is_standard_definition(width: u32, height: u32) -> bool {
    width.max(height) <= SD_MAX.0 && width.min(height) <= SD_MAX.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_is_bt709_for_hd_and_bt601_for_sd() {
        for size in [
            (1920, 1080),
            (1280, 720),
            (1080, 1920),
            (3840, 2160),
            (720, 1280),
        ] {
            assert_eq!(
                OutputColour::for_size(size.0, size.1).encoding.matrix,
                YuvMatrix::Bt709,
                "{size:?}"
            );
        }
        for size in [
            (720, 576),
            (720, 480),
            (640, 480),
            (320, 240),
            (1024, 576),
            (480, 854),
        ] {
            assert_eq!(
                OutputColour::for_size(size.0, size.1).encoding.matrix,
                YuvMatrix::Bt601,
                "{size:?}"
            );
        }
    }

    #[test]
    fn a_forced_matrix_wins_over_the_size() {
        let sd = OutputColour::resolve(ColorMatrix::Bt709, ColorRange::Limited, false, (640, 480));
        assert_eq!(sd.encoding.matrix, YuvMatrix::Bt709);
        let hd = OutputColour::resolve(ColorMatrix::Bt601, ColorRange::Full, true, (1920, 1080));
        assert_eq!(hd.encoding, YuvEncoding::bt601(YuvRange::Full));
        assert!(hd.ten_bit);
        assert_eq!(hd.range(), ffmpeg::color::Range::JPEG);
    }

    #[test]
    fn the_tags_name_what_the_samples_are() {
        let hd = OutputColour::for_size(1920, 1080);
        assert_eq!(hd.space(), ffmpeg::color::Space::BT709);
        assert_eq!(hd.primaries(), ffmpeg::color::Primaries::BT709);
        assert_eq!(hd.transfer(), ffmpeg::color::TransferCharacteristic::BT709);
        assert_eq!(hd.range(), ffmpeg::color::Range::MPEG);
        assert_eq!(hd.label(), "bt709 limited 8-bit");

        // SD changes the matrix and nothing else: the pixels are still in
        // BT.709 primaries, and saying otherwise makes mpv convert a gamut.
        let sd = OutputColour::for_size(720, 576);
        assert_eq!(sd.space(), ffmpeg::color::Space::SMPTE170M);
        assert_eq!(sd.primaries(), ffmpeg::color::Primaries::BT709);
    }

    /// `sws_getCoefficients` clamps an unknown id to BT.601 without a word, so
    /// a wrong number here would quietly undo the whole module. Check the
    /// table it returns against the matrix it is supposed to be.
    #[test]
    fn the_swscale_ids_select_the_right_tables() {
        for (colour, kr) in [
            (OutputColour::for_size(1920, 1080), 0.2126),
            (OutputColour::for_size(720, 576), 0.299),
        ] {
            // SAFETY: `sws_getCoefficients` returns a pointer into a static
            // table of four ints, valid for the life of the process.
            let table = unsafe {
                std::slice::from_raw_parts(
                    ffmpeg::ffi::sws_getCoefficients(colour.sws_coefficients()),
                    4,
                )
            };
            // The first entry is the Cr→R coefficient, 2(1 - Kr), stretched
            // from limited chroma's 224 steps to 255, in 16.16.
            let expected = (2.0 * (1.0 - kr) * 255.0 / 224.0 * 65536.0f64).round() as i32;
            assert!(
                (table[0] - expected).abs() <= 2,
                "{}: table {table:?}, expected {expected} first",
                colour.label()
            );
        }
    }
}
