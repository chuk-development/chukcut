//! Pixels on the CPU: samples of a picture, the grade simulated over them,
//! and the statistics the colour tools fit against.
//!
//! Everything here works on *encoded* (sRGB, `0..1`) colour, the space the
//! decoder hands over and the space most of the grade runs in. The grade is
//! simulated with `render::grade::EncodedStages`, the same CPU reference the
//! shader is tested against, plus the linear-light exposure in front of it —
//! so a fit that converges here converges in the picture the compositor
//! draws. The neighbourhood and lens stages (sharpen, clarity, vignette,
//! grain) are left out: they move local detail and the edges of the frame,
//! not the statistics a balance or a match is made of.

use crate::modules::inspector::edit::GradeEdit;
use crate::modules::render::grade::EncodedStages;
use crate::modules::render::lut::Cube;

/// One encoded RGB colour, `0..1` per channel.
pub type Rgb = [f32; 3];

/// sRGB decode, the shader's `srgb_to_linear`.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB encode, the shader's `linear_to_srgb` (which clamps first).
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Rec. 709 luminance of an encoded colour, in linear light.
pub fn luminance(c: Rgb) -> f32 {
    0.2126 * srgb_to_linear(c[0]) + 0.7152 * srgb_to_linear(c[1]) + 0.0722 * srgb_to_linear(c[2])
}

/// CIE L*a*b* (D65) of an encoded sRGB colour. L in `0..100`.
pub fn lab(c: Rgb) -> [f32; 3] {
    let [r, g, b] = c.map(srgb_to_linear);
    let x = (0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b) / 0.950_47;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b;
    let z = (0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b) / 1.088_83;
    let f = |t: f32| {
        if t > 0.008_856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Pixels sampled from a picture: a few thousand encoded colours, enough
/// for statistics and few enough to simulate a grade over many times.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Samples {
    pub pixels: Vec<Rgb>,
}

impl Samples {
    /// Take every `step`-th pixel of a tightly packed RGBA8 frame.
    pub fn add_rgba(&mut self, rgba: &[u8], step: usize) {
        for px in rgba.as_chunks::<4>().0.iter().step_by(step.max(1)) {
            self.pixels.push([
                px[0] as f32 / 255.0,
                px[1] as f32 / 255.0,
                px[2] as f32 / 255.0,
            ]);
        }
    }

    /// The samples as `edit` would draw them.
    pub fn graded(&self, edit: &GradeEdit, lut: Option<&Cube>) -> Samples {
        let sim = Simulation::new(edit, lut);
        Samples {
            pixels: self.pixels.iter().map(|&c| sim.apply(c)).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pixels.is_empty()
    }
}

/// A grade prepared for many pixels: exposure in light, then the encoded
/// stages and the LUT, exactly in the shader's order.
pub struct Simulation<'a> {
    exposure: Option<f32>,
    stages: EncodedStages<'a>,
    lut: Option<(&'a Cube, f32)>,
}

impl<'a> Simulation<'a> {
    pub fn new(edit: &'a GradeEdit, lut: Option<&'a Cube>) -> Self {
        let color = (edit.brightness != 0.0
            || edit.contrast != 1.0
            || edit.saturation != 1.0
            || edit.temperature != 0.0)
            .then_some([
                edit.brightness,
                edit.contrast,
                edit.saturation,
                edit.temperature,
            ]);
        let intensity = edit.lut.as_ref().map_or(0.0, |l| l.intensity);
        Self {
            exposure: (edit.grade.exposure != 0.0).then(|| edit.grade.exposure.exp2()),
            stages: EncodedStages::new(color, &edit.grade),
            lut: lut.filter(|_| edit.lut.is_some()).map(|c| (c, intensity)),
        }
    }

    pub fn apply(&self, c: Rgb) -> Rgb {
        let c = match self.exposure {
            Some(k) => c.map(|v| linear_to_srgb(srgb_to_linear(v) * k)),
            None => c,
        };
        let sample = |rgb: Rgb| self.lut.map_or(rgb, |(cube, _)| cube.sample(rgb));
        let look = self
            .lut
            .map(|(_, intensity)| (&sample as &dyn Fn(Rgb) -> Rgb, intensity));
        self.stages.apply(c, look)
    }
}

/// Mean and standard deviation of the three L*a*b* channels.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct LabStats {
    pub mean: [f32; 3],
    pub std: [f32; 3],
}

impl LabStats {
    pub fn of(samples: &Samples) -> Self {
        let n = samples.pixels.len().max(1) as f64;
        let labs: Vec<[f32; 3]> = samples.pixels.iter().map(|&c| lab(c)).collect();
        let mut mean = [0f64; 3];
        for l in &labs {
            for i in 0..3 {
                mean[i] += l[i] as f64;
            }
        }
        mean = mean.map(|m| m / n);
        let mut var = [0f64; 3];
        for l in &labs {
            for i in 0..3 {
                let d = l[i] as f64 - mean[i];
                var[i] += d * d;
            }
        }
        Self {
            mean: mean.map(|m| m as f32),
            std: var.map(|v| (v / n).sqrt() as f32),
        }
    }

    /// How far apart two pictures' colour statistics are, in L*a*b* units:
    /// the distance between the means plus that between the spreads. Zero
    /// for identical statistics; a few units is a match the eye accepts.
    pub fn distance(&self, other: &LabStats) -> f32 {
        let mut d = 0.0;
        for i in 0..3 {
            d += (self.mean[i] - other.mean[i]).powi(2);
            d += (self.std[i] - other.std[i]).powi(2);
        }
        d.sqrt()
    }
}

/// The value below which `fraction` of `values` lie (sorted in place).
pub fn quantile(values: &mut [f32], fraction: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable_by(f32::total_cmp);
    let i = ((values.len() - 1) as f32 * fraction.clamp(0.0, 1.0)).round() as usize;
    values[i]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_has_the_textbook_white_grey_and_red() {
        let white = lab([1.0, 1.0, 1.0]);
        assert!((white[0] - 100.0).abs() < 0.1 && white[1].abs() < 0.1 && white[2].abs() < 0.1);
        let grey = lab([0.5, 0.5, 0.5]);
        assert!((grey[0] - 53.4).abs() < 0.3, "{grey:?}");
        let red = lab([1.0, 0.0, 0.0]);
        assert!(
            (red[0] - 53.2).abs() < 0.5 && (red[1] - 80.1).abs() < 1.0,
            "{red:?}"
        );
    }

    #[test]
    fn the_simulation_is_the_identity_at_rest_and_exposure_works_in_light() {
        let samples = Samples {
            pixels: vec![[0.2, 0.5, 0.8], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
        };
        assert_eq!(samples.graded(&GradeEdit::identity(), None), samples);
        let mut edit = GradeEdit::identity();
        edit.grade.exposure = 1.0;
        let brighter = samples.graded(&edit, None);
        // One stop doubles the light of the mid channel.
        let before = srgb_to_linear(0.5);
        let after = srgb_to_linear(brighter.pixels[0][1]);
        assert!((after / before - 2.0).abs() < 0.01, "{before} -> {after}");
    }
}
