//! Motion blur: a clip that moves fast smears along its path, the way a film
//! camera's open shutter records it.
//!
//! In the catalog it is an ordinary effect (`catalog::MOTION_BLUR`: shutter
//! angle and sample count, keyframable), so the Effects tab, the inspector,
//! undo and the CLI handle it like any other. It is never a pass of the
//! effect chain (`FxInstance::at_rest` says so): a pass sees one picture,
//! and motion blur needs the clip's *movement*. The compositor instead
//! places the clip at [`sample_times`] spread over the shutter — the
//! transform keyframes, the In/Out/Combo animation, a followed track and a
//! stabilisation window all evaluated at each — draws it once per sample
//! into a float layer with weight `1/n`, and lays the average where the clip
//! would have been (`render::compositor`, `Draw::Accumulated`).
//!
//! A clip that does not move within the shutter is drawn once, as before;
//! a still clip with motion blur costs nothing.

use crate::modules::project::document::{MaterialPool, Micros, Segment};

use super::catalog::{descriptor, MOTION_BLUR};

/// A clip's motion blur at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionBlur {
    /// How much of a frame the shutter is open for: the shutter angle / 360.
    pub shutter: f32,
    /// How many placements the frame averages.
    pub samples: u32,
}

/// The motion blur on `segment` at `source_time`, if it has one that is on
/// and open.
pub fn motion_blur_of(
    materials: &MaterialPool,
    segment: &Segment,
    source_time: Micros,
) -> Option<MotionBlur> {
    if segment.extras.is_empty() {
        return None;
    }
    let desc = descriptor(MOTION_BLUR)?;
    let effect = segment
        .extras
        .iter()
        .filter_map(|id| materials.effect(id))
        .find(|e| e.enabled && e.kind == MOTION_BLUR)?;
    let number = |id: &str| {
        let spec = desc.param(id)?;
        Some(spec.clamp(effect.number_at(id, source_time, spec.default_number())))
    };
    let shutter = number("shutter")? / 360.0;
    let samples = number("samples")?.round() as u32;
    (shutter > 0.0 && samples >= 2).then_some(MotionBlur { shutter, samples })
}

/// The timeline instants a frame at `time` averages: `samples` of them,
/// evenly over a shutter centred on `time`, kept inside the clip.
///
/// Centred rather than trailing, so the blurred clip sits where the sharp one
/// would — a trailing shutter shifts every moving thing half a shutter back,
/// which reads as lag against the sound.
pub fn sample_times(time: Micros, fps: f64, blur: MotionBlur, segment: &Segment) -> Vec<Micros> {
    let fps = if fps.is_finite() && fps > 0.0 {
        fps
    } else {
        30.0
    };
    let span = blur.shutter as f64 * 1_000_000.0 / fps;
    let first = segment.target_range.start;
    let last = (segment.target_range.end() - 1).max(first);
    let n = blur.samples.max(2);
    (0..n)
        .map(|i| {
            let offset = (i as f64 / (n - 1) as f64 - 0.5) * span;
            (time + offset.round() as Micros).clamp(first, last)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{TimeRange, Transform};
    use crate::modules::project::EffectMaterial;

    fn segment(extras: Vec<String>) -> Segment {
        Segment {
            id: "s".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(1_000_000, 1_000_000),
            source_range: TimeRange::new(0, 1_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras,
            keyframes: Vec::new(),
        }
    }

    #[test]
    fn a_half_open_shutter_spans_half_a_frame_centred_on_the_instant() {
        let blur = MotionBlur {
            shutter: 0.5,
            samples: 3,
        };
        let times = sample_times(1_500_000, 25.0, blur, &segment(Vec::new()));
        assert_eq!(times, vec![1_490_000, 1_500_000, 1_510_000]);
    }

    #[test]
    fn samples_never_leave_the_clip() {
        let blur = MotionBlur {
            shutter: 1.0,
            samples: 5,
        };
        let times = sample_times(1_000_000, 30.0, blur, &segment(Vec::new()));
        assert!(times.iter().all(|&t| (1_000_000..2_000_000).contains(&t)));
        assert_eq!(times[0], 1_000_000);
    }

    #[test]
    fn only_an_enabled_open_motion_blur_counts() {
        let mut pool = MaterialPool::default();
        let mut effect = EffectMaterial::new(MOTION_BLUR);
        effect.id = "mb".into();
        pool.effects.push(effect.clone());
        let seg = segment(vec!["mb".into()]);
        let blur = motion_blur_of(&pool, &seg, 0).unwrap();
        assert_eq!(blur.samples, 8);
        assert!((blur.shutter - 0.5).abs() < 1e-6);

        pool.effects[0].enabled = false;
        assert_eq!(motion_blur_of(&pool, &seg, 0), None);

        pool.effects[0].enabled = true;
        pool.effects[0].params.insert(
            "shutter".into(),
            crate::modules::project::EffectValue::Number(0.0),
        );
        assert_eq!(motion_blur_of(&pool, &seg, 0), None);
    }
}
