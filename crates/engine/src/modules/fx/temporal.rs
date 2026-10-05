//! Temporal noise reduction: the "Reduce noise" effect in its Temporal mode.
//!
//! Sensor noise changes from frame to frame; the picture mostly does not.
//! Averaging a pixel with the same pixel in the frames either side of it
//! removes noise that a spatial filter can only blur. Where something moves,
//! the pixels differ by far more than noise does, and the neighbour's weight
//! falls to nothing — the filter is *motion-adaptive*, not motion-compensated:
//! a moving area keeps the spatial pass only, and nothing ghosts.
//!
//! ## Which frames
//!
//! The source frames one period before and one after the frame shown
//! (`source_time ± 1/fps` of the file), not the previous two. The media
//! provider keeps two frames per video (`CachedTexture::previous`), and the
//! decode-ahead brings the next frame in before each render: a window of
//! "this one and the two before" would find the oldest evicted every frame
//! and seek the decoder backwards (25–200 ms). Asked in the order before,
//! now, after, a centred window hits the cache twice and decodes forwards
//! once, in playback and export alike. The decode-ahead asks for "before"
//! and "now" for such a clip (`media::provider::want_video`).
//!
//! ## Where it runs
//!
//! The compositor draws the two neighbours with the clip's own placement into
//! layers beside the clip's (`render::compositor`, `Draw::Effected`), and
//! [`super::FxFrame::apply_with_neighbours`] runs the temporal pass on the
//! clip as decoded, before any effect of the stack, so an effect above the
//! denoise never sees two different pictures mixed. The spatial pass then
//! runs where the effect sits in the stack, gentler, because less noise is
//! left. Preview and export take the same path; only the sizes differ, and
//! every length is in pixels of the frame being drawn.
//!
//! A clip that blends frames, has motion blur or runs a blur animation draws
//! through another path and gets the spatial pass only.

use crate::modules::project::document::{MaterialPool, Micros, Segment};

use super::catalog::{descriptor, DENOISE};

/// The "Mode" choice of the denoise effect, in index order.
pub const MODES: &[&str] = &["Spatial", "Temporal"];

/// Whether `segment` has a Reduce noise effect in Temporal mode that does
/// something at `source_time`.
pub fn is_on(materials: &MaterialPool, segment: &Segment, source_time: Micros) -> bool {
    let Some(desc) = descriptor(DENOISE) else {
        return false;
    };
    materials
        .effects_of(segment)
        .into_iter()
        .filter(|e| e.enabled && e.kind == DENOISE)
        .any(|effect| {
            let number = |id: &str| {
                desc.param(id).map(|spec| {
                    spec.clamp(effect.number_at(id, source_time, spec.default_number()))
                })
            };
            number("mode").is_some_and(|m| m >= 0.5) && number("strength").is_some_and(|s| s > 0.0)
        })
}

/// One frame of the file at `fps`, in microseconds; 30 fps when the rate is
/// unknown.
pub fn period(fps: f64) -> Micros {
    if fps.is_finite() && fps > 1.0 {
        ((1_000_000.0 / fps).round() as Micros).max(1)
    } else {
        33_333
    }
}

/// The source instants of the frames before and after `source_time`. The
/// one before is `None` at the start of the file.
pub fn neighbours(source_time: Micros, period: Micros) -> (Option<Micros>, Micros) {
    let before = source_time - period;
    ((before >= 0).then_some(before), source_time + period)
}

/// The temporal pass's parameters: `(range sigma, neighbour weight)`.
///
/// The range sigma is how far apart (in √-linear units, a 3×3 mean on both
/// sides so the noise itself barely counts) a neighbour may be and still be
/// averaged in. Strength widens it and raises the neighbours' weight to an
/// even three-frame mean; "keep detail" narrows it, which keeps fine texture
/// that changes between frames (leaves, water) out of the average.
pub fn params(strength: f32, detail: f32) -> (f32, f32) {
    let strength = (strength / 100.0).clamp(0.0, 1.0);
    let detail = (detail / 100.0).clamp(0.0, 1.0);
    let sigma = (0.03 + 0.09 * strength) * (1.25 - 0.75 * detail);
    let weight = 0.35 + 0.65 * strength;
    (sigma, weight)
}

/// How much of the spatial pass's range is left once the temporal pass ran:
/// averaging three frames leaves about `1/√3` of the noise.
pub const SPATIAL_AFTER_TEMPORAL: f32 = 0.8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_centred_and_stops_at_the_start_of_the_file() {
        assert_eq!(period(30.0), 33_333);
        assert_eq!(period(0.0), 33_333);
        assert_eq!(neighbours(100_000, 33_333), (Some(66_667), 133_333));
        assert_eq!(neighbours(20_000, 33_333), (None, 53_333));
    }

    #[test]
    fn strength_widens_and_detail_narrows_the_range() {
        let (weak, w0) = params(10.0, 50.0);
        let (strong, w1) = params(90.0, 50.0);
        assert!(strong > weak && w1 > w0);
        let (fine, _) = params(50.0, 100.0);
        let (coarse, _) = params(50.0, 0.0);
        assert!(fine < coarse);
        assert!(w1 <= 1.0);
    }
}
