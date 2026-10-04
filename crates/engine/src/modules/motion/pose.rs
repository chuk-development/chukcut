//! From parameters to a pose, frame by frame.
//!
//! Pure arithmetic on the document: no GPU, no IO, so every curve is testable
//! by number. The compositor asks [`clip_motion`] once per segment per frame
//! and gets back what to add to the clip's own transform.
//!
//! ## How an animation composes with the clip
//!
//! The clip's static transform and its keyframes come first
//! (`layout::animated_transform`); a [`Pose`] is then applied *relative* to
//! that: offsets add, scales multiply, angles add, opacity multiplies. So a
//! clip the user placed in the corner at 40 % slides in *to the corner* and
//! pops *to 40 %* — the preset never needs to know where the clip lives. The
//! punch-in zoom comes last, about a canvas pivot, like a camera move over
//! the finished layer.

use crate::modules::project::animation::{
    AnimationMaterial, AnimationPreset, ClipAnimation, Ease, PunchZoom,
};
use crate::modules::project::document::{Micros, Segment, Transform};
use crate::modules::project::MaterialPool;

/// What an animation does to a clip at one instant, relative to the clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Added to `Transform::position`, normalized canvas units, +y up.
    pub offset: [f32; 2],
    /// Multiplies `Transform::scale`.
    pub scale: [f32; 2],
    /// Added to `Transform::rotation`, degrees clockwise.
    pub rotation: f32,
    /// Multiplies `Transform::opacity`.
    pub opacity: f32,
    /// How far into its blur the clip is: 0 sharp and fully there, 1 fully
    /// blurred and gone. The blur pass fades with it — a blur is always an
    /// entrance or an exit.
    pub blur: f32,
    /// The radius at `blur` = 1, as a fraction of the frame width.
    pub blur_radius: f32,
    /// The visible part of the clip as `[x0, y0, x1, y1]` fractions of its
    /// own rectangle, y down. The whole clip is `[0, 0, 1, 1]`.
    pub reveal: [f32; 4],
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        offset: [0.0, 0.0],
        scale: [1.0, 1.0],
        rotation: 0.0,
        opacity: 1.0,
        blur: 0.0,
        blur_radius: 0.0,
        reveal: [0.0, 0.0, 1.0, 1.0],
    };

    /// `self` followed by `other`.
    pub fn then(self, other: Pose) -> Pose {
        Pose {
            offset: [
                self.offset[0] + other.offset[0],
                self.offset[1] + other.offset[1],
            ],
            scale: [
                self.scale[0] * other.scale[0],
                self.scale[1] * other.scale[1],
            ],
            rotation: self.rotation + other.rotation,
            opacity: self.opacity * other.opacity,
            blur: self.blur.max(other.blur),
            blur_radius: self.blur_radius.max(other.blur_radius),
            reveal: [
                self.reveal[0].max(other.reveal[0]),
                self.reveal[1].max(other.reveal[1]),
                self.reveal[2].min(other.reveal[2]),
                self.reveal[3].min(other.reveal[3]),
            ],
        }
    }

    pub fn is_identity(&self) -> bool {
        // A radius with no blur to scale is still at rest.
        Pose {
            blur_radius: 0.0,
            ..*self
        } == Pose::IDENTITY
            && self.blur <= 0.0
    }

    /// Whether `reveal` hides any of the clip.
    pub fn reveals_part(&self) -> bool {
        self.reveal != [0.0, 0.0, 1.0, 1.0]
    }

    /// `transform` with this pose applied on top.
    pub fn apply(&self, transform: Transform) -> Transform {
        let mut t = transform;
        t.position[0] += self.offset[0];
        t.position[1] += self.offset[1];
        t.scale[0] *= self.scale[0];
        t.scale[1] *= self.scale[1];
        t.rotation += self.rotation;
        t.opacity = (t.opacity * self.opacity).clamp(0.0, 1.0);
        t
    }
}

impl Default for Pose {
    fn default() -> Self {
        Pose::IDENTITY
    }
}

/// A pose for `preset` at eased progress `p`, where 1 is at rest.
///
/// `p` is the eased value, so it may leave `0..1` for the overshooting
/// curves — a pop at 1.08 is the overshoot. `t` is seconds since the clip's
/// start, for the presets that shake; `strength` scales the travel.
pub fn preset_pose(preset: AnimationPreset, p: f32, t: f32, strength: f32) -> Pose {
    use AnimationPreset as P;
    let d = 1.0 - p;
    let s = strength;
    let fade = (p * 2.0).clamp(0.0, 1.0);
    let mut pose = Pose::IDENTITY;
    match preset {
        P::Fade => pose.opacity = p.clamp(0.0, 1.0),
        // Named for the direction of travel: "slide left" moves leftwards,
        // so it starts to the right of where the clip rests.
        P::SlideLeft => {
            pose.offset[0] = 0.6 * s * d;
            pose.opacity = fade;
        }
        P::SlideRight => {
            pose.offset[0] = -0.6 * s * d;
            pose.opacity = fade;
        }
        P::SlideUp => {
            pose.offset[1] = -0.6 * s * d;
            pose.opacity = fade;
        }
        P::SlideDown => {
            pose.offset[1] = 0.6 * s * d;
            pose.opacity = fade;
        }
        P::ZoomIn => {
            let k = (1.0 - 0.6 * s * d).max(0.0);
            pose.scale = [k, k];
            pose.opacity = fade;
        }
        P::ZoomOut => {
            let k = 1.0 + 0.6 * s * d;
            pose.scale = [k, k];
            pose.opacity = fade;
        }
        P::Pop => {
            let k = (1.0 - s * d).max(0.0);
            pose.scale = [k, k];
            pose.opacity = (p * 4.0).clamp(0.0, 1.0);
        }
        P::Bounce => {
            pose.offset[1] = 0.8 * s * d;
            pose.opacity = (p * 4.0).clamp(0.0, 1.0);
        }
        P::Spin => {
            pose.rotation = -180.0 * s * d;
            let k = (1.0 - 0.7 * d).max(0.0);
            pose.scale = [k, k];
            pose.opacity = fade;
        }
        P::Blur => {
            pose.blur = d.clamp(0.0, 1.0);
            pose.blur_radius = 0.05 * s;
        }
        // Named for the edge's travel: "wipe right" reveals left to right.
        P::WipeRight => pose.reveal[2] = p.clamp(0.0, 1.0),
        P::WipeLeft => pose.reveal[0] = (1.0 - p).clamp(0.0, 1.0),
        P::WipeDown => pose.reveal[3] = p.clamp(0.0, 1.0),
        P::WipeUp => pose.reveal[1] = (1.0 - p).clamp(0.0, 1.0),
        P::Swing => {
            pose.rotation = 30.0 * s * d;
            pose.opacity = (p * 3.0).clamp(0.0, 1.0);
        }
        P::Shake => {
            let amplitude = 0.08 * s * d.max(0.0);
            let (x, y) = jitter(t, 30.0);
            pose.offset = [x * amplitude, y * amplitude];
            pose.rotation = jitter(t + 7.0, 30.0).0 * 6.0 * s * d.max(0.0);
            pose.opacity = fade;
        }
        P::Rise => {
            pose.offset[1] = -0.15 * s * d;
            let k = 1.0 - 0.1 * s * d;
            pose.scale = [k, k];
            pose.opacity = p.clamp(0.0, 1.0);
        }
        P::Flip => {
            pose.scale[0] = p.abs().min(4.0);
            pose.opacity = (p * 3.0).clamp(0.0, 1.0);
        }
        P::Whip => {
            pose.offset[0] = -1.6 * s * d;
            pose.blur = (0.6 * d).clamp(0.0, 1.0);
            pose.blur_radius = 0.04 * s;
        }
        // Combo presets have no In or Out form; at rest they are identity.
        P::Pulse
        | P::Heartbeat
        | P::Wobble
        | P::Rock
        | P::Float
        | P::Jitter
        | P::Rotate
        | P::Flicker => {}
    }
    pose
}

/// A looping pose for a Combo preset at `phase` in `0..1`.
pub fn combo_pose(preset: AnimationPreset, phase: f32, t: f32, strength: f32, ease: Ease) -> Pose {
    use std::f32::consts::TAU;
    use AnimationPreset as P;
    let s = strength;
    let phase = phase.rem_euclid(1.0);
    // A triangle wave 0→1→0 through the curve: the loop's easing shapes the
    // swell, and the loop is continuous whatever the curve.
    let swell = ease.apply(1.0 - (2.0 * phase - 1.0).abs());
    let wave = (TAU * phase).sin();
    let mut pose = Pose::IDENTITY;
    match preset {
        P::Pulse => {
            let k = 1.0 + 0.08 * s * swell;
            pose.scale = [k, k];
        }
        P::Heartbeat => {
            // Two beats then a rest: lub-dub.
            let beat = |c: f32| (1.0 - ((phase - c) / 0.08).abs()).max(0.0);
            let k = 1.0 + 0.1 * s * (beat(0.1) + 0.7 * beat(0.3));
            pose.scale = [k, k];
        }
        P::Wobble => {
            pose.scale = [1.0 + 0.06 * s * wave, 1.0 - 0.06 * s * wave];
        }
        P::Rock => pose.rotation = 8.0 * s * wave,
        P::Float => pose.offset[1] = 0.03 * s * wave,
        P::Jitter => {
            let (x, y) = jitter(t, 24.0);
            pose.offset = [0.02 * s * x, 0.02 * s * y];
        }
        P::Rotate => pose.rotation = 360.0 * phase * s,
        P::Flicker => {
            let (n, _) = jitter(t, 12.0);
            pose.opacity = if n > 0.4 { 1.0 - 0.6 * s.min(1.0) } else { 1.0 };
        }
        // An In/Out preset in the Combo slot plays as a loop of itself:
        // in over the first half, out over the second.
        other => return preset_pose(other, swell, t, strength),
    }
    pose
}

/// Two deterministic values in `-1..1` that change `rate` times a second.
///
/// A pure function of time, never a random generator: the preview and the
/// export render the same instant and must shake the same way.
pub fn jitter(t: f32, rate: f32) -> (f32, f32) {
    let step = (t * rate).floor() as i64;
    let h = hash(step as u64);
    let a = (h & 0xffff) as f32 / 65535.0 * 2.0 - 1.0;
    let b = ((h >> 16) & 0xffff) as f32 / 65535.0 * 2.0 - 1.0;
    (a, b)
}

/// SplitMix64: a well-mixed hash of an integer.
pub fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The In and Out windows of a clip `duration` long.
///
/// When the two do not fit — a clip trimmed shorter than its animations —
/// both shrink in proportion, so the clip still enters and leaves and the
/// two never overlap. This is what makes a preset survive a trim.
pub fn windows(material: &AnimationMaterial, duration: Micros) -> (Micros, Micros) {
    let wanted_in = material.intro.map_or(0, |a| a.duration.max(0));
    let wanted_out = material.outro.map_or(0, |a| a.duration.max(0));
    let total = wanted_in + wanted_out;
    let duration = duration.max(0);
    if total <= duration || total == 0 {
        return (wanted_in, wanted_out);
    }
    let factor = duration as f64 / total as f64;
    let fit_in = (wanted_in as f64 * factor).floor() as Micros;
    (fit_in, duration - fit_in)
}

/// The pose of a whole material at `rel` microseconds into a clip
/// `duration` long. Excludes the zoom, which is not a relative pose.
pub fn material_pose(material: &AnimationMaterial, rel: Micros, duration: Micros) -> Pose {
    let t = rel as f32 / 1_000_000.0;
    let (win_in, win_out) = windows(material, duration);
    let mut pose = Pose::IDENTITY;

    if let Some(a) = material.intro {
        if win_in > 0 && rel < win_in {
            let p = a.easing.apply(rel.max(0) as f32 / win_in as f32);
            pose = pose.then(preset_pose(a.preset, p, t, a.strength));
        }
    }
    if let Some(a) = material.outro {
        let start = duration - win_out;
        if win_out > 0 && rel >= start {
            // The In played backwards: progress runs from 1 at the start of
            // the window to 0 on the clip's last instant.
            let remaining = (duration - rel).max(0) as f32 / win_out as f32;
            let p = a.easing.apply(remaining);
            pose = pose.then(preset_pose(a.preset, p, t, a.strength));
        }
    }
    if let Some(a) = material.combo {
        pose = pose.then(combo(a, rel, t));
    }
    pose
}

fn combo(a: ClipAnimation, rel: Micros, t: f32) -> Pose {
    let period = a.duration.max(1);
    let phase = rel.rem_euclid(period) as f32 / period as f32;
    combo_pose(a.preset, phase, t, a.strength, a.easing)
}

/// The zoom factor a punch-in has reached at `rel`.
pub fn zoom_factor(zoom: &PunchZoom, rel: Micros) -> f32 {
    let progress = if zoom.duration <= 0 {
        1.0
    } else {
        zoom.easing.apply(rel.max(0) as f32 / zoom.duration as f32)
    };
    1.0 + (zoom.amount - 1.0) * progress
}

/// `transform` scaled by `factor` about the canvas point `pivot`.
///
/// Scaling everything about a point moves the clip's centre away from the
/// pivot by the same factor; rotation commutes with a uniform scale, so a
/// rotated clip needs no special case.
pub fn zoom_about(transform: Transform, pivot: [f32; 2], factor: f32) -> Transform {
    let mut t = transform;
    t.position[0] = pivot[0] + (t.position[0] - pivot[0]) * factor;
    t.position[1] = pivot[1] + (t.position[1] - pivot[1]) * factor;
    t.scale[0] *= factor;
    t.scale[1] *= factor;
    t
}

/// What the compositor needs from the motion module for one clip at `time`.
#[derive(Debug, Clone, Copy)]
pub struct ClipMotion {
    /// The clip's transform with every animation applied.
    pub transform: Transform,
    /// See [`Pose::blur`].
    pub blur: f32,
    pub blur_radius: f32,
    pub reveal: [f32; 4],
}

/// The motion of `segment` at timeline `time`, given the transform its
/// keyframes already produced. `None` when the clip has no animation, so the
/// compositor's path for an unanimated clip is exactly what it was.
pub fn clip_motion(
    materials: &MaterialPool,
    segment: &Segment,
    time: Micros,
    keyed: Transform,
) -> Option<ClipMotion> {
    let material = materials.animation_of(segment)?;
    let rel = time - segment.target_range.start;
    let duration = segment.target_range.duration;
    let pose = material_pose(material, rel, duration);
    let mut transform = pose.apply(keyed);
    if let Some(zoom) = &material.zoom {
        transform = zoom_about(transform, zoom.pivot, zoom_factor(zoom, rel));
    }
    Some(ClipMotion {
        transform,
        blur: pose.blur,
        blur_radius: pose.blur_radius,
        reveal: pose.reveal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::animation::AnimationPreset as P;

    fn anim(preset: P, duration: Micros) -> ClipAnimation {
        ClipAnimation {
            preset,
            duration,
            easing: Ease::Linear,
            strength: 1.0,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn every_in_preset_is_at_rest_when_it_finishes() {
        for preset in crate::modules::motion::catalog::clip_presets()
            .into_iter()
            .filter(|d| d.in_out)
            .map(|d| d.preset)
        {
            let pose = preset_pose(preset, 1.0, 0.37, 1.0);
            assert!(pose.is_identity(), "{preset:?} rests at {pose:?}");
        }
    }

    #[test]
    fn every_in_preset_moves_something_at_the_start() {
        for preset in crate::modules::motion::catalog::clip_presets()
            .into_iter()
            .filter(|d| d.in_out)
            .map(|d| d.preset)
        {
            let pose = preset_pose(preset, 0.0, 0.37, 1.0);
            assert!(!pose.is_identity(), "{preset:?} does nothing");
        }
    }

    #[test]
    fn an_in_runs_from_the_clip_start_and_an_out_into_the_clip_end() {
        let mut m = AnimationMaterial::new();
        m.intro = Some(anim(P::Fade, 500_000));
        m.outro = Some(anim(P::Fade, 1_000_000));
        let d = 4_000_000;
        assert!(close(material_pose(&m, 0, d).opacity, 0.0));
        assert!(close(material_pose(&m, 250_000, d).opacity, 0.5));
        assert!(close(material_pose(&m, 2_000_000, d).opacity, 1.0));
        assert!(close(material_pose(&m, 3_500_000, d).opacity, 0.5));
        assert!(close(material_pose(&m, 4_000_000, d).opacity, 0.0));
    }

    #[test]
    fn windows_that_do_not_fit_shrink_in_proportion() {
        let mut m = AnimationMaterial::new();
        m.intro = Some(anim(P::Fade, 1_000_000));
        m.outro = Some(anim(P::Fade, 3_000_000));
        assert_eq!(windows(&m, 10_000_000), (1_000_000, 3_000_000));
        assert_eq!(windows(&m, 2_000_000), (500_000, 1_500_000));
        assert_eq!(windows(&m, 0), (0, 0));
    }

    #[test]
    fn a_combo_loops_with_its_period() {
        let mut m = AnimationMaterial::new();
        m.combo = Some(anim(P::Rock, 1_000_000));
        let d = 10_000_000;
        let a = material_pose(&m, 250_000, d);
        let b = material_pose(&m, 3_250_000, d);
        assert!(close(a.rotation, b.rotation));
        assert!(close(a.rotation, 8.0));
        assert!(close(material_pose(&m, 2_000_000, d).rotation, 0.0));
    }

    #[test]
    fn a_pose_is_relative_to_where_the_clip_sits() {
        let keyed = Transform {
            position: [0.5, -0.5],
            scale: [0.4, 0.4],
            opacity: 0.8,
            ..Default::default()
        };
        let pose = preset_pose(P::SlideLeft, 0.5, 0.0, 1.0);
        let t = pose.apply(keyed);
        assert!(close(t.position[0], 0.8));
        assert!(close(t.position[1], -0.5));
        assert!(close(t.scale[0], 0.4));
        assert!(close(t.opacity, 0.8 * 1.0));
        let pop = preset_pose(P::Pop, 0.5, 0.0, 1.0).apply(keyed);
        assert!(close(pop.scale[0], 0.2));
    }

    #[test]
    fn a_zoom_keeps_its_pivot_still() {
        let t = Transform {
            position: [0.2, 0.1],
            ..Default::default()
        };
        let pivot = [0.5, 0.5];
        let z = zoom_about(t, pivot, 2.0);
        assert!(close(z.position[0], -0.1));
        assert!(close(z.position[1], -0.3));
        assert!(close(z.scale[0], 2.0));
        // A pivot at the clip's centre only scales.
        let c = zoom_about(t, t.position, 1.5);
        assert_eq!(c.position, t.position);
    }

    #[test]
    fn a_zoom_ramps_then_holds() {
        let zoom = PunchZoom {
            amount: 1.2,
            pivot: [0.0, 0.0],
            duration: 1_000_000,
            easing: Ease::Linear,
        };
        assert!(close(zoom_factor(&zoom, 0), 1.0));
        assert!(close(zoom_factor(&zoom, 500_000), 1.1));
        assert!(close(zoom_factor(&zoom, 9_000_000), 1.2));
        let punch = PunchZoom {
            duration: 0,
            ..zoom
        };
        assert!(close(zoom_factor(&punch, 0), 1.2));
    }

    #[test]
    fn shaking_is_a_function_of_time_alone() {
        let a = preset_pose(P::Shake, 0.3, 1.234, 1.0);
        let b = preset_pose(P::Shake, 0.3, 1.234, 1.0);
        assert_eq!(a, b);
        assert_ne!(a.offset, preset_pose(P::Shake, 0.3, 2.5, 1.0).offset);
    }

    #[test]
    fn a_wipe_reveals_from_its_edge() {
        let half = preset_pose(P::WipeRight, 0.5, 0.0, 1.0);
        assert_eq!(half.reveal, [0.0, 0.0, 0.5, 1.0]);
        let up = preset_pose(P::WipeUp, 0.25, 0.0, 1.0);
        assert_eq!(up.reveal, [0.0, 0.75, 1.0, 1.0]);
    }
}
