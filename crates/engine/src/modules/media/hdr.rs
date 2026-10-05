//! HDR to SDR on the CPU: the twin of `render/shaders/yuv.wgsl`'s
//! `yuv_to_linear`, for the pictures that never reach the compositor.
//!
//! The compositor tone-maps HDR sources in its shader (decision 0034). But
//! thumbnails, proxies and the analysis passes take their frames from
//! [`super::VideoDecoder::seek_and_decode`] as RGBA, and swscale alone turns a
//! PQ or HLG signal into sRGB-tagged bytes that look grey and flat. So that
//! path asks swscale for 16-bit non-linear RGB instead and finishes here, with
//! the same steps and the same constants as the shader: EOTF (PQ, or HLG with
//! the BT.2100 OOTF for a 1000-nit display), HDR reference white (203 nits) as
//! SDR 1.0, BT.2020 to BT.709, the BT.2390 EETF on the brightest channel, sRGB.
//!
//! The per-sample curves are tables over the 16-bit input, built once, so a
//! frame costs a few lookups and a 3x3 matrix per pixel; only pixels above the
//! tone map's knee pay for the EETF's powers.

use std::sync::OnceLock;

/// The light a frame's R'G'B' is in, for the conversion below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    /// SMPTE ST 2084, BT.2020.
    Pq,
    /// ARIB STD-B67, BT.2020.
    Hlg,
    /// An SDR transfer in BT.2020 primaries: only the gamut changes.
    WideSdr,
}

impl Light {
    /// What a frame tagged with `transfer` and `primaries` needs, or `None`
    /// for ordinary SDR in BT.709, which swscale's RGBA is already right for.
    pub fn of(
        transfer: ffmpeg_next::color::TransferCharacteristic,
        primaries: ffmpeg_next::color::Primaries,
    ) -> Option<Light> {
        use ffmpeg_next::color::{Primaries, TransferCharacteristic};
        match transfer {
            TransferCharacteristic::SMPTE2084 => Some(Light::Pq),
            TransferCharacteristic::ARIB_STD_B67 => Some(Light::Hlg),
            _ if primaries == Primaries::BT2020 => Some(Light::WideSdr),
            _ => None,
        }
    }
}

const REFERENCE_WHITE_NITS: f32 = 203.0;
const SOURCE_PEAK_NITS: f32 = 1000.0;

const M1: f32 = 0.159_301_76;
const M2: f32 = 78.843_75;
const C1: f32 = 0.835_937_5;
const C2: f32 = 18.851_563;
const C3: f32 = 18.6875;

fn pq_eotf(e: f32) -> f32 {
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

fn pq_inverse(y: f32) -> f32 {
    let p = y.clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * p) / (1.0 + C3 * p)).powf(M2)
}

fn hlg_scene(e: f32) -> f32 {
    let (a, b, c) = (0.178_832_77, 0.284_668_92, 0.559_910_7);
    let e = e.clamp(0.0, 1.0);
    if e <= 0.5 {
        e * e / 3.0
    } else {
        (((e - c) / a).exp() + b) / 12.0
    }
}

fn srgb_eotf(e: f32) -> f32 {
    if e <= 0.040_45 {
        e / 12.92
    } else {
        ((e + 0.055) / 1.055).powf(2.4)
    }
}

/// The BT.2390 EETF for SDR-relative light, exactly as `yuv.wgsl`'s `eetf`.
fn eetf(light: f32) -> f32 {
    let source_peak = pq_inverse(SOURCE_PEAK_NITS / 10_000.0);
    let target_peak = pq_inverse(REFERENCE_WHITE_NITS / 10_000.0) / source_peak;
    let knee = 1.5 * target_peak - 0.5;
    let e = (pq_inverse(light * REFERENCE_WHITE_NITS / 10_000.0) / source_peak).min(1.0);
    if e <= knee {
        return light;
    }
    let t = (e - knee) / (1.0 - knee);
    let (t2, t3) = (t * t, t * t * t);
    let mapped = (2.0 * t3 - 3.0 * t2 + 1.0) * knee
        + (t3 - 2.0 * t2 + t) * (1.0 - knee)
        + (-2.0 * t3 + 3.0 * t2) * target_peak;
    pq_eotf(mapped * source_peak) * 10_000.0 / REFERENCE_WHITE_NITS
}

/// SDR-relative light below which [`eetf`] is the identity.
fn knee_light() -> f32 {
    static KNEE: OnceLock<f32> = OnceLock::new();
    *KNEE.get_or_init(|| {
        let source_peak = pq_inverse(SOURCE_PEAK_NITS / 10_000.0);
        let target_peak = pq_inverse(REFERENCE_WHITE_NITS / 10_000.0) / source_peak;
        let knee = 1.5 * target_peak - 0.5;
        pq_eotf(knee * source_peak) * 10_000.0 / REFERENCE_WHITE_NITS
    })
}

/// A curve over every 16-bit code value.
fn table(curve: impl Fn(f32) -> f32) -> Box<[f32]> {
    (0..=u16::MAX)
        .map(|code| curve(code as f32 / 65_535.0))
        .collect()
}

/// PQ signal to SDR-relative light.
fn pq_table() -> &'static [f32] {
    static TABLE: OnceLock<Box<[f32]>> = OnceLock::new();
    TABLE.get_or_init(|| table(|e| pq_eotf(e) * 10_000.0 / REFERENCE_WHITE_NITS))
}

/// HLG signal to scene light, before the OOTF.
fn hlg_table() -> &'static [f32] {
    static TABLE: OnceLock<Box<[f32]>> = OnceLock::new();
    TABLE.get_or_init(|| table(hlg_scene))
}

/// sRGB-encoded signal to linear light.
fn srgb_table() -> &'static [f32] {
    static TABLE: OnceLock<Box<[f32]>> = OnceLock::new();
    TABLE.get_or_init(|| table(srgb_eotf))
}

/// Linear light 0..1, in 65 536 steps, to an sRGB-encoded byte. Fine steps
/// because the encode is steep near black: 4096 would put three code values
/// in the first step.
fn encode_table() -> &'static [u8] {
    static TABLE: OnceLock<Box<[u8]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=u16::MAX)
            .map(|code| {
                let v = code as f32 / 65_535.0;
                let e = if v <= 0.003_130_8 {
                    v * 12.92
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                (e * 255.0).round().clamp(0.0, 255.0) as u8
            })
            .collect()
    })
}

fn bt2020_to_bt709([r, g, b]: [f32; 3]) -> [f32; 3] {
    [
        1.660_491 * r - 0.587_641 * g - 0.072_850 * b,
        -0.124_551 * r + 1.1329 * g - 0.008_349 * b,
        -0.018_151 * r - 0.100_579 * g + 1.118_73 * b,
    ]
}

/// One pixel of 16-bit non-linear BT.2020 R'G'B' to sRGB bytes.
fn pixel(light: Light, rgb: [u16; 3]) -> [u8; 3] {
    let linear = match light {
        Light::Pq => {
            let t = pq_table();
            rgb.map(|v| t[v as usize])
        }
        Light::Hlg => {
            let t = hlg_table();
            let scene = rgb.map(|v| t[v as usize]);
            let ys = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
            // The BT.2100 OOTF for a 1000-nit display, gamma 1.2, then nits
            // to SDR-relative light.
            let gain = SOURCE_PEAK_NITS * ys.max(1e-6).powf(0.2) / REFERENCE_WHITE_NITS;
            scene.map(|v| v * gain)
        }
        Light::WideSdr => {
            let t = srgb_table();
            rgb.map(|v| t[v as usize])
        }
    };
    let mut out = bt2020_to_bt709(linear).map(|v| v.max(0.0));
    if light != Light::WideSdr {
        let peak = out[0].max(out[1]).max(out[2]);
        if peak > knee_light() {
            let ratio = eetf(peak) / peak;
            out = out.map(|v| v * ratio);
        }
    }
    let encode = encode_table();
    out.map(|v| encode[(v.clamp(0.0, 1.0) * 65_535.0).round() as usize])
}

/// Convert `rows` of packed RGB48LE (`stride` bytes apart) into tightly packed
/// RGBA8, opaque.
pub fn tone_map_rgb48(
    light: Light,
    data: &[u8],
    stride: usize,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let line = &data[row * stride..row * stride + width as usize * 6];
        for texel in line.as_chunks::<6>().0 {
            let rgb = [
                u16::from_le_bytes([texel[0], texel[1]]),
                u16::from_le_bytes([texel[2], texel[3]]),
                u16::from_le_bytes([texel[4], texel[5]]),
            ];
            let [r, g, b] = pixel(light, rgb);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PQ code for SDR-relative light in BT.2020, as 16 bits.
    fn pq_code(light: f32) -> u16 {
        (pq_inverse(light * REFERENCE_WHITE_NITS / 10_000.0) * 65_535.0).round() as u16
    }

    #[test]
    fn midtone_greys_come_through_unchanged() {
        // Grey is grey in both gamuts, and below the knee the tone map is
        // the identity: a PQ grey at 18 % of SDR white is an sRGB 118.
        for (light, srgb) in [(0.18f32, 118u8), (0.0, 0), (0.05, 63), (0.4, 170)] {
            let code = pq_code(light);
            let [r, g, b] = pixel(Light::Pq, [code; 3]);
            assert!(r.abs_diff(srgb) <= 1, "{light}: {r} against {srgb}");
            assert_eq!((r, r), (g, b), "grey stays grey");
        }
    }

    #[test]
    fn highlights_fold_back_under_white_and_keep_their_order() {
        let white = pixel(Light::Pq, [pq_code(1.0); 3])[0];
        let peak = pixel(Light::Pq, [pq_code(1000.0 / 203.0); 3])[0];
        let beyond = pixel(Light::Pq, [65_535; 3])[0];
        // BT.2390 with SDR white as the target peak puts reference white at
        // about 0.79 linear (sRGB 229) and the 1000-nit peak at 1.0.
        assert!((227..=231).contains(&white), "reference white at {white}");
        assert_eq!(peak, 255);
        assert_eq!(beyond, 255);
        assert!(pixel(Light::Pq, [pq_code(0.6); 3])[0] < white);
    }

    #[test]
    fn hlg_reference_white_lands_where_pq_reference_white_does() {
        // 75 % HLG is reference white (BT.2408): 203 nits on a 1000-nit
        // display, the same light as PQ's 203 nits.
        let hlg = pixel(Light::Hlg, [(0.75 * 65_535.0) as u16; 3]);
        let pq = pixel(Light::Pq, [pq_code(1.0); 3]);
        for c in 0..3 {
            assert!(hlg[c].abs_diff(pq[c]) <= 2, "HLG {hlg:?} against PQ {pq:?}");
        }
    }

    #[test]
    fn a_bt2020_primary_is_more_saturated_than_bt709_can_show() {
        // Pure BT.2020 green has no BT.709 equivalent: red and blue go
        // negative and are clipped, green stays.
        let [r, g, b] = pixel(Light::WideSdr, [0, 40_000, 0]);
        assert_eq!((r, b), (0, 0));
        assert!(g > 150);
    }

    #[test]
    fn packed_rows_are_read_with_their_stride() {
        let mut data = vec![0u8; 2 * 16];
        // Two pixels a row, 12 bytes, padded to 16; the second row is white.
        for byte in &mut data[16..28] {
            *byte = 0xff;
        }
        let rgba = tone_map_rgb48(Light::WideSdr, &data, 16, 2, 2);
        assert_eq!(rgba.len(), 16);
        assert_eq!(&rgba[0..4], &[0, 0, 0, 255]);
        assert_eq!(&rgba[8..12], &[255, 255, 255, 255]);
    }
}
