//! Blend modes, the CPU reference for `fs_blend` in `fx/shaders/fx.wgsl`.
//!
//! A clip with a blend mode other than normal is drawn into a layer of its
//! own (masked, keyed, graded, faded, its effects run) and then laid onto the
//! frame composited beneath it by one fullscreen pass that reads both. The
//! modes are the W3C compositing formulas on gamma-encoded colour — what
//! every editor's blend menu means — and the result goes on with the same
//! straight-alpha source-over, in light, that the pipeline's blend state
//! gives a normal clip. The pixel tests compare the GPU against
//! [`composite`].

use crate::modules::project::compositing::BlendMode;

fn soft_light_d(b: f32) -> f32 {
    if b <= 0.25 {
        ((16.0 * b - 12.0) * b + 4.0) * b
    } else {
        b.sqrt()
    }
}

/// One channel of `mode` with backdrop `b` and source `s`, both encoded.
pub fn channel(mode: &BlendMode, b: f32, s: f32) -> f32 {
    let screen = |b: f32, s: f32| b + s - b * s;
    match mode {
        BlendMode::Multiply => b * s,
        BlendMode::Screen => screen(b, s),
        BlendMode::Overlay => {
            if b <= 0.5 {
                s * 2.0 * b
            } else {
                screen(s, 2.0 * b - 1.0)
            }
        }
        BlendMode::SoftLight => {
            if s <= 0.5 {
                b - (1.0 - 2.0 * s) * b * (1.0 - b)
            } else {
                b + (2.0 * s - 1.0) * (soft_light_d(b) - b)
            }
        }
        BlendMode::HardLight => {
            if s <= 0.5 {
                b * 2.0 * s
            } else {
                screen(b, 2.0 * s - 1.0)
            }
        }
        BlendMode::Darken => b.min(s),
        BlendMode::Lighten => b.max(s),
        BlendMode::ColorDodge => {
            if b <= 0.0 {
                0.0
            } else if s >= 1.0 {
                1.0
            } else {
                (b / (1.0 - s)).min(1.0)
            }
        }
        BlendMode::ColorBurn => {
            if b >= 1.0 {
                1.0
            } else if s <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - b) / s).min(1.0)
            }
        }
        BlendMode::Difference => (b - s).abs(),
        BlendMode::Exclusion => b + s - 2.0 * b * s,
        BlendMode::Add => (b + s).min(1.0),
        BlendMode::Subtract => (b - s).max(0.0),
        BlendMode::Normal | BlendMode::Other(_) => s,
    }
}

pub fn encode(linear: f32) -> f32 {
    let l = linear.clamp(0.0, 1.0);
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

pub fn decode(encoded: f32) -> f32 {
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// `top` (the clip, straight alpha) laid onto `base` (the frame so far) with
/// `mode`. Colours are what the shader reads: linear light when `srgb` (an
/// sRGB target), the stored values otherwise. Returns what it writes.
pub fn composite(mode: &BlendMode, base: [f32; 4], top: [f32; 4], srgb: bool) -> [f32; 4] {
    let enc = |v: f32| if srgb { encode(v) } else { v };
    let dec = |v: f32| if srgb { decode(v) } else { v };
    let a = top[3];
    let mut out = [0.0; 4];
    for c in 0..3 {
        let cb = enc(base[c]);
        let cs = enc(top[c].clamp(0.0, 1.0));
        let mixed = channel(mode, cb, cs);
        let shown = dec((cs * (1.0 - base[3]) + mixed * base[3]).clamp(0.0, 1.0));
        out[c] = shown * a + base[c] * (1.0 - a);
    }
    out[3] = a + base[3] * (1.0 - a);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_numbers_its_modes_as_the_menu_does() {
        // `fs_blend` switches on `BlendMode::code`; the cases must be the
        // menu order. Pin the ones whose formulas are easy to recognise.
        let shader = include_str!("../fx/shaders/fx.wgsl");
        let at = |mode: BlendMode| format!("case {}u:", mode.code().unwrap());
        let body = &shader[shader.find("fn blend_channel").unwrap()..];
        let line_after = |mode: BlendMode| {
            let i = body.find(&at(mode)).unwrap();
            body[i..i + 60].to_string()
        };
        assert!(line_after(BlendMode::Multiply).contains("b * s"));
        assert!(line_after(BlendMode::Darken).contains("min(b, s)"));
        assert!(line_after(BlendMode::Lighten).contains("max(b, s)"));
        assert!(line_after(BlendMode::Difference).contains("abs(b - s)"));
        assert!(line_after(BlendMode::Subtract).contains("max(0.0, b - s)"));
    }

    #[test]
    fn opaque_modes_follow_their_formulas() {
        let b = 0.6;
        let s = 0.3;
        assert!((channel(&BlendMode::Multiply, b, s) - 0.18).abs() < 1e-6);
        assert!((channel(&BlendMode::Screen, b, s) - 0.72).abs() < 1e-6);
        assert!((channel(&BlendMode::Difference, b, s) - 0.3).abs() < 1e-6);
        assert!((channel(&BlendMode::Exclusion, b, s) - 0.54).abs() < 1e-6);
        assert_eq!(channel(&BlendMode::Add, b, 0.7), 1.0);
        assert!((channel(&BlendMode::Subtract, b, s) - 0.3).abs() < 1e-6);
        // Overlay on a dark backdrop multiplies, hard light on a dark source
        // multiplies: the swap is the whole difference between them.
        assert!((channel(&BlendMode::Overlay, 0.25, 0.8) - 0.4).abs() < 1e-6);
        assert!((channel(&BlendMode::HardLight, 0.8, 0.25) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn normal_through_the_formula_is_plain_source_over() {
        let base = [0.2, 0.4, 0.6, 1.0];
        let top = [0.9, 0.1, 0.5, 0.5];
        let out = composite(&BlendMode::Normal, base, top, true);
        for c in 0..3 {
            let want = top[c] * 0.5 + base[c] * 0.5;
            assert!((out[c] - want).abs() < 1e-5);
        }
        assert_eq!(out[3], 1.0);
    }
}
