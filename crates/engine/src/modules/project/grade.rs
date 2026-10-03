//! The extended colour grade: everything past the four original sliders.
//!
//! [`Grade`] is one field on [`super::ColorAdjustMaterial`]. It holds the
//! controls a grading panel has — tone, presence, effects, HSL, curves and
//! colour wheels — and the compositor applies them in the one order written
//! down in `render/grade.rs`.
//!
//! ## Schema safety
//!
//! Every field has a serde default that is the identity of its operation, and
//! the material skips the whole block when it is the identity. So:
//!
//! - a project saved before this type existed opens with every clip
//!   ungraded here, and
//! - a project that never touches these controls saves byte-identical to one
//!   written before they existed.
//!
//! Both are pinned in the tests below.
//!
//! ## Units
//!
//! The document stores *meaning*, not slider positions. Most controls are
//! `-1..1` with `0` at rest; exposure is in photographic stops; the vignette
//! midpoint and feather are fractions of the frame's half diagonal. The panel
//! maps its `-100..100` sliders onto these.

use serde::{Deserialize, Serialize};

/// Everything a clip's grade holds beyond brightness, contrast, saturation,
/// temperature and the LUT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Grade {
    /// Exposure in stops, applied in linear light: `+1` doubles the light.
    pub exposure: f32,
    /// Green/magenta balance: positive is magenta, negative is green. `-1..1`.
    pub tint: f32,
    /// Brightens (positive) or darkens the bright tones. `-1..1`.
    pub highlights: f32,
    /// Brightens (positive) or darkens the dark tones. `-1..1`.
    pub shadows: f32,
    /// Moves the white point: positive clips more to white. `-1..1`.
    pub whites: f32,
    /// Moves the black point: positive lifts the blacks. `-1..1`.
    pub blacks: f32,
    /// Saturation that spares colours that are already saturated. `-1..1`.
    pub vibrance: f32,
    /// Unsharp-mask strength. `0..1`.
    pub sharpen: f32,
    /// Local contrast in the midtones. `-1..1`.
    pub clarity: f32,
    pub vignette: Vignette,
    /// Film grain strength. `0..1`.
    pub grain: f32,
    /// Lifts the blacks and lowers the whites, a faded print. `0..1`.
    pub fade: f32,
    pub hsl: Hsl,
    pub curves: Curves,
    pub wheels: Wheels,
}

/// A darkening (or lightening) towards the frame's edges.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// Positive darkens the edges, negative lightens them. `-1..1`.
    pub amount: f32,
    /// Where the falloff is centred, as a fraction of the distance from the
    /// centre to a corner. `0..1`, `0.5` by default.
    pub midpoint: f32,
    /// How wide the falloff is, as a fraction of the same distance. `0..1`,
    /// `0.5` by default. `0` is a hard edge.
    pub feather: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 0.5,
            feather: 0.5,
        }
    }
}

/// The eight hue bands of the HSL panel, in [`HSL_BANDS`] order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Hsl {
    pub bands: [HslBand; 8],
}

/// One hue band's adjustment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HslBand {
    /// Hue shift: `±1` turns the band's colours by ±30 degrees.
    pub hue: f32,
    /// `-1` removes the band's colour, `+1` doubles its saturation.
    pub saturation: f32,
    /// `-1..1`: darkens or brightens the band's colours, never greys.
    pub luminance: f32,
}

/// The HSL bands' names and centre hues in degrees.
///
/// Not evenly spaced, because the colours people name are not: orange and
/// yellow are thirty degrees apart, green and aqua sixty. The spacing is the
/// one Lightroom and CapCut use, so a user's muscle memory carries over.
pub const HSL_BANDS: [(&str, f32); 8] = [
    ("Red", 0.0),
    ("Orange", 30.0),
    ("Yellow", 60.0),
    ("Green", 120.0),
    ("Aqua", 180.0),
    ("Blue", 240.0),
    ("Purple", 270.0),
    ("Magenta", 300.0),
];

/// Tone curves: a master curve applied to every channel, then one per
/// channel.
///
/// Each curve is a list of `[x, y]` control points in `0..1`, kept sorted by
/// `x`. An empty list is the identity, and so is the list the panel starts
/// with, `[[0, 0], [1, 1]]` — the panel stores the empty list when a curve is
/// reset so "untouched" has one spelling. Points are joined by a monotone
/// cubic (Fritsch–Carlson), which never overshoots between two points: a
/// curve the user drew rising never dips.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Curves {
    pub master: Vec<[f32; 2]>,
    pub red: Vec<[f32; 2]>,
    pub green: Vec<[f32; 2]>,
    pub blue: Vec<[f32; 2]>,
}

/// Which tone curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveChannel {
    Master,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [CurveChannel; 4] = [Self::Master, Self::Red, Self::Green, Self::Blue];
}

impl Curves {
    pub fn get(&self, channel: CurveChannel) -> &Vec<[f32; 2]> {
        match channel {
            CurveChannel::Master => &self.master,
            CurveChannel::Red => &self.red,
            CurveChannel::Green => &self.green,
            CurveChannel::Blue => &self.blue,
        }
    }

    pub fn get_mut(&mut self, channel: CurveChannel) -> &mut Vec<[f32; 2]> {
        match channel {
            CurveChannel::Master => &mut self.master,
            CurveChannel::Red => &mut self.red,
            CurveChannel::Green => &mut self.green,
            CurveChannel::Blue => &mut self.blue,
        }
    }

    pub fn is_identity(&self) -> bool {
        CurveChannel::ALL
            .iter()
            .all(|&c| curve_is_identity(self.get(c)))
    }
}

/// Whether a point list leaves every value where it is.
pub fn curve_is_identity(points: &[[f32; 2]]) -> bool {
    if points.is_empty() {
        return true;
    }
    // Every point on the diagonal, and the points span 0..1: a monotone
    // cubic through collinear points is that line exactly. A diagonal that
    // stops short is flat beyond its ends, which is not the identity.
    points.len() >= 2
        && points.iter().all(|p| p[0] == p[1])
        && points[0][0] == 0.0
        && points[points.len() - 1][0] == 1.0
}

/// The four colour wheels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Wheels {
    /// Shadows: pushes the dark end towards a colour, leaving white alone.
    pub lift: Wheel,
    /// Midtones: bends the middle of the range, leaving black and white alone.
    pub gamma: Wheel,
    /// Highlights: scales the bright end, leaving black alone.
    pub gain: Wheel,
    /// Everything: shifts the whole range.
    pub offset: Wheel,
}

/// Which colour wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WheelKind {
    Lift,
    Gamma,
    Gain,
    Offset,
}

impl WheelKind {
    pub const ALL: [WheelKind; 4] = [Self::Lift, Self::Gamma, Self::Gain, Self::Offset];
}

impl Wheels {
    pub fn get(&self, kind: WheelKind) -> &Wheel {
        match kind {
            WheelKind::Lift => &self.lift,
            WheelKind::Gamma => &self.gamma,
            WheelKind::Gain => &self.gain,
            WheelKind::Offset => &self.offset,
        }
    }

    pub fn get_mut(&mut self, kind: WheelKind) -> &mut Wheel {
        match kind {
            WheelKind::Lift => &mut self.lift,
            WheelKind::Gamma => &mut self.gamma,
            WheelKind::Gain => &mut self.gain,
            WheelKind::Offset => &mut self.offset,
        }
    }
}

/// One wheel: a colour push and a luminance slider.
///
/// The push is a point in the unit disc — `(x, y)`, the way the puck sits on
/// the wheel — rather than a hue angle and an amount, because an angle has a
/// seam at 360 degrees and a point does not; dragging the puck through red is
/// then not a special case anywhere. `x` points at red, and hue turns
/// counter-clockwise (red, yellow, green, cyan, blue, magenta), the layout
/// of every grading wheel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Wheel {
    pub x: f32,
    pub y: f32,
    /// `-1..1`: darkens or brightens the wheel's range on all channels.
    pub luma: f32,
}

impl Wheel {
    pub fn is_identity(&self) -> bool {
        self.x == 0.0 && self.y == 0.0 && self.luma == 0.0
    }
}

impl Grade {
    /// Whether applying this grade changes nothing at all.
    ///
    /// Exact comparisons, not tolerances: the compositor skips every stage
    /// whose controls are at rest, so the question is "did anyone move this",
    /// which a float either was or was not.
    pub fn is_identity(&self) -> bool {
        self.exposure == 0.0
            && self.tint == 0.0
            && self.highlights == 0.0
            && self.shadows == 0.0
            && self.whites == 0.0
            && self.blacks == 0.0
            && self.vibrance == 0.0
            && self.sharpen == 0.0
            && self.clarity == 0.0
            && self.vignette.amount == 0.0
            && self.grain == 0.0
            && self.fade == 0.0
            && self.hsl == Hsl::default()
            && self.curves.is_identity()
            && WheelKind::ALL
                .iter()
                .all(|&k| self.wheels.get(k).is_identity())
    }

    /// Every float in the grade with a name, for validation.
    pub fn named_values(&self) -> Vec<(String, f32)> {
        let mut out: Vec<(String, f32)> = vec![
            ("exposure".into(), self.exposure),
            ("tint".into(), self.tint),
            ("highlights".into(), self.highlights),
            ("shadows".into(), self.shadows),
            ("whites".into(), self.whites),
            ("blacks".into(), self.blacks),
            ("vibrance".into(), self.vibrance),
            ("sharpen".into(), self.sharpen),
            ("clarity".into(), self.clarity),
            ("vignette amount".into(), self.vignette.amount),
            ("vignette midpoint".into(), self.vignette.midpoint),
            ("vignette feather".into(), self.vignette.feather),
            ("grain".into(), self.grain),
            ("fade".into(), self.fade),
        ];
        for (band, (name, _)) in self.hsl.bands.iter().zip(HSL_BANDS) {
            out.push((format!("{name} hue"), band.hue));
            out.push((format!("{name} saturation"), band.saturation));
            out.push((format!("{name} luminance"), band.luminance));
        }
        for channel in CurveChannel::ALL {
            for p in self.curves.get(channel) {
                out.push((format!("{channel:?} curve point"), p[0]));
                out.push((format!("{channel:?} curve point"), p[1]));
            }
        }
        for kind in WheelKind::ALL {
            let w = self.wheels.get(kind);
            out.push((format!("{kind:?} wheel"), w.x));
            out.push((format!("{kind:?} wheel"), w.y));
            out.push((format!("{kind:?} wheel luminance"), w.luma));
        }
        out
    }

    /// The first value that is not a finite number, by name.
    pub fn non_finite_field(&self) -> Option<String> {
        self.named_values()
            .into_iter()
            .find(|(_, v)| !v.is_finite())
            .map(|(name, _)| name)
    }

    /// Bring every value into its documented range and every curve into
    /// canonical form: points clamped to `0..1`, sorted, one per `x`, and an
    /// identity curve stored empty. Called on every committed edit, so the
    /// document never holds a value the renderer would have to second-guess.
    pub fn normalized(mut self) -> Self {
        let unit = |v: f32| v.clamp(-1.0, 1.0);
        self.exposure = self.exposure.clamp(-5.0, 5.0);
        self.tint = unit(self.tint);
        self.highlights = unit(self.highlights);
        self.shadows = unit(self.shadows);
        self.whites = unit(self.whites);
        self.blacks = unit(self.blacks);
        self.vibrance = unit(self.vibrance);
        self.sharpen = self.sharpen.clamp(0.0, 1.0);
        self.clarity = unit(self.clarity);
        self.vignette.amount = unit(self.vignette.amount);
        self.vignette.midpoint = self.vignette.midpoint.clamp(0.0, 1.0);
        self.vignette.feather = self.vignette.feather.clamp(0.0, 1.0);
        self.grain = self.grain.clamp(0.0, 1.0);
        self.fade = self.fade.clamp(0.0, 1.0);
        for band in &mut self.hsl.bands {
            band.hue = unit(band.hue);
            band.saturation = unit(band.saturation);
            band.luminance = unit(band.luminance);
        }
        for channel in CurveChannel::ALL {
            let points = self.curves.get_mut(channel);
            *points = normalize_curve(std::mem::take(points));
        }
        for kind in WheelKind::ALL {
            let w = self.wheels.get_mut(kind);
            let r = (w.x * w.x + w.y * w.y).sqrt();
            if r > 1.0 {
                w.x /= r;
                w.y /= r;
            }
            w.luma = unit(w.luma);
        }
        self
    }
}

/// A curve's canonical form: see [`Grade::normalized`].
pub fn normalize_curve(points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    let mut points: Vec<[f32; 2]> = points
        .into_iter()
        .filter(|p| p[0].is_finite() && p[1].is_finite())
        .map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)])
        .collect();
    points.sort_by(|a, b| a[0].total_cmp(&b[0]));
    // Two points at one `x` would be a vertical step, which no function
    // through the points can draw; the later one wins, as the one the user
    // placed last on that spot.
    points.dedup_by(|later, earlier| {
        if later[0] == earlier[0] {
            *earlier = *later;
            true
        } else {
            false
        }
    });
    if curve_is_identity(&points) {
        Vec::new()
    } else {
        points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_grade_is_the_identity_and_reads_from_nothing() {
        let grade: Grade = serde_json::from_str("{}").expect("an empty block opens");
        assert!(grade.is_identity());
        assert_eq!(grade, Grade::default());
        assert_eq!(grade.vignette.midpoint, 0.5);

        // A partial block fills in the rest with identities.
        let grade: Grade =
            serde_json::from_str(r#"{ "exposure": 1.0, "hsl": { "bands": [{}, {"hue": 0.5}, {}, {}, {}, {}, {}, {}] } }"#)
                .expect("partial block opens");
        assert_eq!(grade.exposure, 1.0);
        assert_eq!(grade.hsl.bands[1].hue, 0.5);
        assert_eq!(grade.hsl.bands[1].saturation, 0.0);
        assert!(!grade.is_identity());
    }

    #[test]
    fn every_control_breaks_identity() {
        let moved: Vec<Box<dyn Fn(&mut Grade)>> = vec![
            Box::new(|g| g.exposure = 0.1),
            Box::new(|g| g.tint = 0.1),
            Box::new(|g| g.highlights = 0.1),
            Box::new(|g| g.shadows = 0.1),
            Box::new(|g| g.whites = 0.1),
            Box::new(|g| g.blacks = 0.1),
            Box::new(|g| g.vibrance = 0.1),
            Box::new(|g| g.sharpen = 0.1),
            Box::new(|g| g.clarity = 0.1),
            Box::new(|g| g.vignette.amount = 0.1),
            Box::new(|g| g.grain = 0.1),
            Box::new(|g| g.fade = 0.1),
            Box::new(|g| g.hsl.bands[7].luminance = 0.1),
            Box::new(|g| g.curves.blue = vec![[0.0, 0.1], [1.0, 1.0]]),
            Box::new(|g| g.wheels.offset.luma = 0.1),
            Box::new(|g| g.wheels.lift.x = 0.1),
        ];
        for (i, m) in moved.iter().enumerate() {
            let mut g = Grade::default();
            m(&mut g);
            assert!(!g.is_identity(), "control {i} did not count");
        }
        // Moving only the vignette's shape with no amount changes nothing.
        let mut g = Grade::default();
        g.vignette.midpoint = 0.2;
        assert!(g.is_identity());
    }

    #[test]
    fn curves_normalise_to_one_spelling() {
        assert!(normalize_curve(vec![[0.0, 0.0], [1.0, 1.0]]).is_empty());
        assert!(normalize_curve(vec![[1.0, 1.0], [0.5, 0.5], [0.0, 0.0]]).is_empty());
        let c = normalize_curve(vec![[0.7, 0.9], [0.0, 0.0], [1.4, 1.0], [0.7, 0.8]]);
        assert_eq!(c, vec![[0.0, 0.0], [0.7, 0.8], [1.0, 1.0]]);
        // A diagonal that does not span the whole range is still the
        // identity between its ends but flat outside: not the identity.
        assert!(!curve_is_identity(&[[0.2, 0.2], [0.8, 0.8]]));
    }

    #[test]
    fn normalising_clamps_and_keeps_the_wheel_puck_on_the_wheel() {
        let mut g = Grade::default();
        g.tint = 3.0;
        g.wheels.gain = Wheel {
            x: 3.0,
            y: 4.0,
            luma: -2.0,
        };
        let g = g.normalized();
        assert_eq!(g.tint, 1.0);
        assert!((g.wheels.gain.x - 0.6).abs() < 1e-6);
        assert!((g.wheels.gain.y - 0.8).abs() < 1e-6);
        assert_eq!(g.wheels.gain.luma, -1.0);
    }

    /// A material written before the extended grade existed reads back
    /// with it at rest and writes back byte-identical: old projects open
    /// unchanged and save unchanged.
    #[test]
    fn a_material_from_before_the_extended_grade_round_trips_byte_identical() {
        use crate::modules::project::document::ColorAdjustMaterial;
        let old = r#"{"id":"c1","brightness":0.1,"contrast":1.2,"saturation":1.0,"temperature":0.0,"lut":null}"#;
        let material: ColorAdjustMaterial =
            serde_json::from_str(old).expect("an old material opens");
        assert!(material.grade.is_identity());
        assert_eq!(serde_json::to_string(&material).unwrap(), old);

        let mut graded = material.clone();
        graded.grade.curves.red = vec![[0.0, 0.1], [1.0, 1.0]];
        graded.grade.wheels.gamma.x = 0.3;
        graded.grade.hsl.bands[4].luminance = -0.2;
        let json = serde_json::to_string(&graded).unwrap();
        assert!(json.contains("\"grade\""));
        let back: ColorAdjustMaterial = serde_json::from_str(&json).unwrap();
        assert_eq!(back, graded);
        assert!(!back.is_identity());
    }

    #[test]
    fn a_non_finite_value_is_named() {
        let mut g = Grade::default();
        g.hsl.bands[2].saturation = f32::NAN;
        assert_eq!(g.non_finite_field().as_deref(), Some("Yellow saturation"));
    }
}
