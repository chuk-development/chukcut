//! Turning document values into quad geometry.
//!
//! Everything here is pure arithmetic on the project model — no wgpu, no
//! device, no IO. That is deliberate: transform and crop maths is where a
//! compositor is most likely to be subtly wrong, and a bug you can only see by
//! looking at a rendered frame is a bug you will not find. These functions are
//! testable on a machine with no GPU at all.
//!
//! ## Coordinate conventions
//!
//! `Transform::position` is in normalized canvas units: `[0,0]` is the centre
//! and `1.0` is half the canvas dimension. That is the same space as clip
//! space, so a position maps straight onto an NDC offset — **+x is right, +y is
//! up**. Rotation is degrees clockwise as the viewer sees it, which is negative
//! in this y-up space.
//!
//! The rotation is applied in *pixel* space rather than in NDC. Rotating in NDC
//! on a non-square canvas shears the quad, which is the classic "why is my
//! rotated clip a parallelogram on 9:16" bug.

use glam::{Mat4, Vec3};

use crate::modules::project::document::{
    AnimatableProperty, Crop, Micros, Project, Segment, Track, TrackKind, Transform,
};

/// The geometry of one textured quad, ready to become a uniform block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuadPlacement {
    /// Model-view-projection, column-major, unit quad (`±0.5`) to clip space.
    pub mvp: [f32; 16],
    /// Source UV rectangle as `[u0, v0, u1, v1]`.
    pub crop: [f32; 4],
    /// Straight-alpha multiplier, `0..1`.
    pub opacity: f32,
}

/// Size a source is drawn at before the segment's own scale, in canvas pixels.
///
/// "Fit", not "fill": the whole source is visible and the canvas is padded.
/// Filling is what a user achieves by scaling up, and making it the default
/// would silently crop footage on an aspect-ratio change — the one thing this
/// document format goes out of its way to survive.
pub fn fit_size(canvas: (u32, u32), source: (u32, u32)) -> (f32, f32) {
    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);
    let (sw, sh) = (source.0.max(1) as f32, source.1.max(1) as f32);
    let scale = (cw / sw).min(ch / sh);
    (sw * scale, sh * scale)
}

/// Normalize a crop into a `[u0, v0, u1, v1]` UV rectangle.
///
/// `None` is the whole source. Values are clamped into `0..1` and an inverted
/// or empty rectangle is reported as such by returning `None`, because a
/// zero-area crop means the segment draws nothing rather than that it draws
/// everything.
pub fn crop_uv(crop: Option<Crop>) -> Option<[f32; 4]> {
    let Some(crop) = crop else {
        return Some([0.0, 0.0, 1.0, 1.0]);
    };
    let u0 = crop.left.clamp(0.0, 1.0);
    let v0 = crop.top.clamp(0.0, 1.0);
    let u1 = crop.right.clamp(0.0, 1.0);
    let v1 = crop.bottom.clamp(0.0, 1.0);
    if !(u1 > u0 && v1 > v0) {
        return None;
    }
    Some([u0, v0, u1, v1])
}

/// Fraction of the source the crop keeps, as `(width, height)`.
pub fn crop_extent(uv: [f32; 4]) -> (f32, f32) {
    (uv[2] - uv[0], uv[3] - uv[1])
}

/// The segment's transform with every keyframe track sampled and applied.
///
/// Keyframe times are relative to the segment start, so a clip carries its
/// animation when it is moved or trimmed. A keyframe track *overrides* the
/// static value rather than offsetting it: the static transform is what the
/// clip looks like when nothing is animating that property.
pub fn animated_transform(segment: &Segment, time: Micros) -> Transform {
    let mut transform = segment.transform;
    let relative = time - segment.target_range.start;

    for track in &segment.keyframes {
        let Some(value) = track.sample(relative) else {
            continue;
        };
        match track.property {
            AnimatableProperty::PositionX => transform.position[0] = value,
            AnimatableProperty::PositionY => transform.position[1] = value,
            AnimatableProperty::ScaleX => transform.scale[0] = value,
            AnimatableProperty::ScaleY => transform.scale[1] = value,
            AnimatableProperty::Rotation => transform.rotation = value,
            AnimatableProperty::Opacity => transform.opacity = value,
            // Audio. The compositor has no opinion about it; the mixer reads
            // the same track.
            AnimatableProperty::Volume => {}
            // Not part of the transform: `animated_crop` reads these.
            AnimatableProperty::CropLeft
            | AnimatableProperty::CropTop
            | AnimatableProperty::CropRight
            | AnimatableProperty::CropBottom => {}
        }
    }

    transform
}

/// The segment's crop at `time` (timeline time), with its crop keyframes
/// sampled. The static crop when it has none. See [`Segment::crop_at`].
pub fn animated_crop(segment: &Segment, time: Micros) -> Option<Crop> {
    segment.crop_at(time)
}

/// Build the quad for one segment, or `None` when it would draw nothing.
///
/// Order of operations: crop the source, fit the cropped result into the
/// canvas, apply the segment's scale, flip, rotate, then translate. Cropping
/// first is what makes a crop behave like a crop — the kept region grows to
/// fill the frame — rather than like a mask that leaves a hole.
pub fn place_quad(
    canvas: (u32, u32),
    source: (u32, u32),
    transform: &Transform,
    crop: Option<Crop>,
) -> Option<QuadPlacement> {
    let opacity = transform.opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return None;
    }

    let uv = crop_uv(crop)?;
    let (crop_w, crop_h) = crop_extent(uv);

    let cropped_source = (
        (source.0.max(1) as f32 * crop_w).max(1.0) as u32,
        (source.1.max(1) as f32 * crop_h).max(1.0) as u32,
    );
    let (fit_w, fit_h) = fit_size(canvas, cropped_source);

    let width = fit_w * transform.scale[0];
    let height = fit_h * transform.scale[1];
    if width == 0.0 || height == 0.0 || !width.is_finite() || !height.is_finite() {
        return None;
    }

    let (cw, ch) = (canvas.0.max(1) as f32, canvas.1.max(1) as f32);

    // Pixel space, origin at the canvas centre, +y up.
    let flip_x = if transform.flip_h { -1.0 } else { 1.0 };
    let flip_y = if transform.flip_v { -1.0 } else { 1.0 };
    let model = Mat4::from_scale(Vec3::new(width * flip_x, height * flip_y, 1.0));
    let rotate = Mat4::from_rotation_z(-transform.rotation.to_radians());
    let translate = Mat4::from_translation(Vec3::new(
        transform.position[0] * cw * 0.5,
        transform.position[1] * ch * 0.5,
        0.0,
    ));

    // Pixel space to clip space.
    let projection = Mat4::from_scale(Vec3::new(2.0 / cw, 2.0 / ch, 1.0));

    let mvp = projection * translate * rotate * model;

    Some(QuadPlacement {
        mvp: mvp.to_cols_array(),
        crop: uv,
        opacity,
    })
}

/// Narrow a placed quad to the part of it `reveal` keeps.
///
/// `reveal` is `[x0, y0, x1, y1]` as fractions of the clip's own rectangle,
/// y down — what a wipe animation produces. The kept part stays exactly where
/// it was on the canvas and shows exactly the texels it showed: the quad is
/// shrunk in its local space (before rotation and flips) and the UV rectangle
/// with it, so a wipe follows a rotated or mirrored clip. `None` when nothing
/// is left to draw.
pub fn reveal(placement: QuadPlacement, reveal: [f32; 4]) -> Option<QuadPlacement> {
    let x0 = reveal[0].clamp(0.0, 1.0);
    let y0 = reveal[1].clamp(0.0, 1.0);
    let x1 = reveal[2].clamp(0.0, 1.0);
    let y1 = reveal[3].clamp(0.0, 1.0);
    if !(x1 > x0 && y1 > y0) {
        return None;
    }
    // The unit quad is ±0.5 with +y up and v = 0 at the top edge.
    let centre = Vec3::new(-0.5 + (x0 + x1) * 0.5, 0.5 - (y0 + y1) * 0.5, 0.0);
    let local = Mat4::from_translation(centre) * Mat4::from_scale(Vec3::new(x1 - x0, y1 - y0, 1.0));
    let mvp = Mat4::from_cols_array(&placement.mvp) * local;
    let [u0, v0, u1, v1] = placement.crop;
    Some(QuadPlacement {
        mvp: mvp.to_cols_array(),
        crop: [
            u0 + (u1 - u0) * x0,
            v0 + (v1 - v0) * y0,
            u0 + (u1 - u0) * x1,
            v0 + (v1 - v0) * y1,
        ],
        opacity: placement.opacity,
    })
}

/// Whether a track contributes pixels.
///
/// `hidden` is the visibility switch. `muted` deliberately is *not*: it silences
/// a lane's audio, and hiding the picture along with it would surprise anyone
/// who muted a video track to hear the music underneath. Audio lanes are
/// skipped because there is nothing to draw, not because they might be muted.
pub fn track_is_visible(track: &Track) -> bool {
    !track.hidden && track.kind != TrackKind::Audio
}

/// Segments to composite at `time`, back-to-front.
///
/// `Project::segments_at` already sorts by `render_index`; this only drops the
/// lanes that do not paint.
pub fn visible_segments(project: &Project, time: Micros) -> Vec<(&Track, &Segment)> {
    project
        .segments_at(time)
        .into_iter()
        .filter(|(track, _)| track_is_visible(track))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{
        AnimatableProperty, Easing, Keyframe, KeyframeTrack, Segment, TimeRange, Transform,
    };
    use glam::{Vec3, Vec4};

    fn segment() -> Segment {
        Segment {
            id: "s".into(),
            material_id: "m".into(),
            target_range: TimeRange::new(1_000_000, 2_000_000),
            source_range: TimeRange::new(0, 2_000_000),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// Where a unit-quad corner lands in clip space.
    fn corner(mvp: &[f32; 16], x: f32, y: f32) -> Vec3 {
        let m = Mat4::from_cols_array(mvp);
        let v = m * Vec4::new(x, y, 0.0, 1.0);
        v.truncate() / v.w
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_reveal_keeps_the_kept_part_where_it_was() {
        let full = place_quad((1000, 1000), (1000, 1000), &Transform::default(), None).unwrap();
        let left = reveal(full, [0.0, 0.0, 0.5, 1.0]).unwrap();
        // The left half of the clip: its right edge is now the canvas centre
        // and it samples the left half of the texture.
        let right_edge = corner(&left.mvp, 0.5, 0.0);
        let left_edge = corner(&left.mvp, -0.5, 0.0);
        assert!(close(right_edge.x, 0.0) && close(left_edge.x, -1.0));
        assert_eq!(left.crop, [0.0, 0.0, 0.5, 1.0]);
        let top = reveal(full, [0.0, 0.0, 1.0, 0.25]).unwrap();
        assert!(close(corner(&top.mvp, 0.0, -0.5).y, 0.5));
        assert_eq!(top.crop, [0.0, 0.0, 1.0, 0.25]);
        assert!(reveal(full, [0.5, 0.0, 0.5, 1.0]).is_none());
        assert_eq!(reveal(full, [0.0, 0.0, 1.0, 1.0]), Some(full));
    }

    #[test]
    fn matching_aspect_fits_exactly() {
        assert_eq!(fit_size((1920, 1080), (1920, 1080)), (1920.0, 1080.0));
        assert_eq!(fit_size((1920, 1080), (960, 540)), (1920.0, 1080.0));
    }

    #[test]
    fn wide_source_on_a_tall_canvas_is_pillarboxed_by_width() {
        // 16:9 into 9:16 — width is the binding constraint.
        let (w, h) = fit_size((1080, 1920), (1920, 1080));
        assert!(close(w, 1080.0));
        assert!(close(h, 607.5));
    }

    #[test]
    fn tall_source_on_a_wide_canvas_is_letterboxed_by_height() {
        let (w, h) = fit_size((1920, 1080), (1080, 1920));
        assert!(close(h, 1080.0));
        assert!(close(w, 607.5));
    }

    #[test]
    fn default_transform_on_a_matching_source_is_identity() {
        let placement =
            place_quad((1920, 1080), (1920, 1080), &Transform::default(), None).unwrap();
        // The quad is ±0.5 in local space, so "identity" here means it maps
        // exactly onto the full clip volume: a plain doubling, no rotation, no
        // translation, no aspect correction left over.
        let m = Mat4::from_cols_array(&placement.mvp);
        assert!(m.abs_diff_eq(Mat4::from_scale(Vec3::new(2.0, 2.0, 1.0)), 1e-5));
        for (x, y) in [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            let c = corner(&placement.mvp, x, y);
            assert!(close(c.x, x * 2.0) && close(c.y, y * 2.0));
        }
        assert_eq!(placement.crop, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(placement.opacity, 1.0);
    }

    #[test]
    fn fit_is_applied_before_the_segment_scale() {
        // 16:9 source, 9:16 canvas: fitted height is 607.5 of 1920 canvas
        // pixels, so the quad covers 0.3164 of clip space vertically.
        let placement =
            place_quad((1080, 1920), (1920, 1080), &Transform::default(), None).unwrap();
        let top = corner(&placement.mvp, 0.0, 0.5);
        assert!(close(top.y, 607.5 / 1920.0));

        let doubled = Transform {
            scale: [2.0, 2.0],
            ..Default::default()
        };
        let placement = place_quad((1080, 1920), (1920, 1080), &doubled, None).unwrap();
        let top = corner(&placement.mvp, 0.0, 0.5);
        assert!(close(top.y, 2.0 * 607.5 / 1920.0));
    }

    #[test]
    fn position_is_in_half_canvas_units() {
        let t = Transform {
            position: [0.5, -0.25],
            ..Default::default()
        };
        let placement = place_quad((1920, 1080), (1920, 1080), &t, None).unwrap();
        let centre = corner(&placement.mvp, 0.0, 0.0);
        assert!(close(centre.x, 0.5));
        assert!(close(centre.y, -0.25));
    }

    #[test]
    fn rotation_is_clockwise_and_square_on_a_non_square_canvas() {
        let t = Transform {
            rotation: 90.0,
            ..Default::default()
        };
        // Square source on a 2:1 canvas so a naive NDC rotation would shear.
        let placement = place_quad((1920, 960), (960, 960), &t, None).unwrap();
        let right = corner(&placement.mvp, 0.5, 0.0);
        // The source fits to 960x960 pixels; its +x edge is 480px from centre.
        // Rotated 90 degrees clockwise that edge points down: -480px in y,
        // which on a 960-tall canvas is -1.0 in clip space.
        assert!(close(right.x, 0.0));
        assert!(close(right.y, -1.0));
    }

    #[test]
    fn flips_mirror_without_moving_the_quad() {
        let t = Transform {
            flip_h: true,
            ..Default::default()
        };
        let placement = place_quad((1000, 1000), (1000, 1000), &t, None).unwrap();
        let left = corner(&placement.mvp, -0.5, 0.0);
        assert!(close(left.x, 1.0));
    }

    #[test]
    fn crop_maps_to_a_uv_rectangle() {
        let crop = Crop {
            left: 0.25,
            top: 0.1,
            right: 0.75,
            bottom: 0.9,
        };
        assert_eq!(crop_uv(Some(crop)), Some([0.25, 0.1, 0.75, 0.9]));
        let (w, h) = crop_extent([0.25, 0.1, 0.75, 0.9]);
        assert!(close(w, 0.5) && close(h, 0.8));
        assert_eq!(crop_uv(None), Some([0.0, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn crop_is_clamped_and_degenerate_crops_draw_nothing() {
        let over = Crop {
            left: -1.0,
            top: 0.0,
            right: 4.0,
            bottom: 1.0,
        };
        assert_eq!(crop_uv(Some(over)), Some([0.0, 0.0, 1.0, 1.0]));

        let inverted = Crop {
            left: 0.8,
            top: 0.0,
            right: 0.2,
            bottom: 1.0,
        };
        assert_eq!(crop_uv(Some(inverted)), None);
        assert!(place_quad(
            (100, 100),
            (100, 100),
            &Transform::default(),
            Some(inverted)
        )
        .is_none());
    }

    #[test]
    fn cropping_changes_the_fitted_aspect() {
        // Take the middle half of a 16:9 source horizontally: 8:9, which on a
        // square canvas is now height-bound.
        let crop = Crop {
            left: 0.25,
            top: 0.0,
            right: 0.75,
            bottom: 1.0,
        };
        let placement = place_quad(
            (1000, 1000),
            (1920, 1080),
            &Transform::default(),
            Some(crop),
        )
        .unwrap();
        let top = corner(&placement.mvp, 0.0, 0.5);
        let right = corner(&placement.mvp, 0.5, 0.0);
        assert!(close(top.y, 1.0));
        // 960x1080 cropped source fitted to a 1000px square: 888.9 x 1000.
        assert!(close(right.x, (1080.0f32 / 1080.0) * (960.0 / 1080.0)));
    }

    #[test]
    fn fully_transparent_segments_are_skipped() {
        let t = Transform {
            opacity: 0.0,
            ..Default::default()
        };
        assert!(place_quad((100, 100), (100, 100), &t, None).is_none());
    }

    #[test]
    fn keyframes_override_the_static_transform() {
        let mut seg = segment();
        seg.transform.opacity = 1.0;
        seg.transform.position = [0.9, 0.9];
        seg.keyframes = vec![
            KeyframeTrack {
                property: AnimatableProperty::Opacity,
                keyframes: vec![
                    Keyframe {
                        time: 0,
                        value: 0.0,
                        easing: Easing::Linear,
                    },
                    Keyframe {
                        time: 1_000_000,
                        value: 1.0,
                        easing: Easing::Linear,
                    },
                ],
            },
            KeyframeTrack {
                property: AnimatableProperty::PositionX,
                keyframes: vec![Keyframe {
                    time: 0,
                    value: -0.5,
                    easing: Easing::Linear,
                }],
            },
        ];

        // Half a second into a segment that starts at t=1s.
        let t = animated_transform(&seg, 1_500_000);
        assert!(close(t.opacity, 0.5));
        // A single keyframe holds its value everywhere.
        assert!(close(t.position[0], -0.5));
        // Untouched properties keep the static value.
        assert!(close(t.position[1], 0.9));
    }

    #[test]
    fn keyframe_times_are_relative_to_the_segment_not_the_timeline() {
        let mut seg = segment();
        seg.keyframes = vec![KeyframeTrack {
            property: AnimatableProperty::Rotation,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.0,
                    easing: Easing::Linear,
                },
                Keyframe {
                    time: 2_000_000,
                    value: 180.0,
                    easing: Easing::Linear,
                },
            ],
        }];

        // At the segment's own start the animation is at its first keyframe,
        // even though the timeline is already at one second.
        assert!(close(animated_transform(&seg, 1_000_000).rotation, 0.0));
        assert!(close(animated_transform(&seg, 2_000_000).rotation, 90.0));
        assert!(close(animated_transform(&seg, 3_000_000).rotation, 180.0));
    }

    #[test]
    fn hidden_and_audio_lanes_do_not_paint() {
        let mut video = Track::new(TrackKind::Video, "V1");
        assert!(track_is_visible(&video));
        video.muted = true;
        assert!(
            track_is_visible(&video),
            "muting silences, it does not hide"
        );
        video.hidden = true;
        assert!(!track_is_visible(&video));

        let audio = Track::new(TrackKind::Audio, "A1");
        assert!(!track_is_visible(&audio));
    }
}
