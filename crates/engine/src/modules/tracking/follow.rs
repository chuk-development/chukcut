//! Evaluating a follow link: where an overlay goes at a timeline instant.
//!
//! Pure arithmetic on the document, like `render/layout.rs`, and called from
//! the compositor for every segment it draws — so the preview and the export
//! place a follower identically, because there is only one function that
//! does it. See [`FollowMaterial`] for the model.

use std::borrow::Cow;

use glam::{Mat4, Vec4};

use super::model::{FollowMaterial, FollowMode, Pose, TrackingMaterial};
use crate::modules::project::document::{
    AnimatableProperty, MaterialPool, Micros, Project, Segment, Transform,
};
use crate::modules::render::layout::{animated_transform, place_quad};

impl MaterialPool {
    pub fn tracking(&self, id: &str) -> Option<&TrackingMaterial> {
        self.trackings.iter().find(|m| m.id == id)
    }

    pub fn follow(&self, id: &str) -> Option<&FollowMaterial> {
        self.follows.iter().find(|m| m.id == id)
    }

    /// The follow link `segment` carries, if any. The resolution step for the
    /// follow category, like `color_adjust_of` is for colour.
    pub fn follow_of(&self, segment: &Segment) -> Option<&FollowMaterial> {
        segment.extras.iter().find_map(|id| self.follow(id))
    }

    /// Every segment id whose extras name `follow_id`.
    pub fn followers<'a>(&self, project: &'a Project, follow_id: &str) -> Vec<&'a Segment> {
        project
            .tracks
            .iter()
            .flat_map(|t| t.segments.iter())
            .filter(|s| s.extras.iter().any(|id| id == follow_id))
            .collect()
    }
}

/// The tracked object as it appears on the canvas at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObjectOnCanvas {
    /// Box centre, canvas-normalised (`0,0` centre, `±1` edges, +y up) — the
    /// units of `Transform::position`.
    pub position: [f32; 2],
    /// The box's four corners in the same units, top-left first, clockwise.
    pub corners: [[f32; 2]; 4],
    /// The pose read off the track, for its confidence and for scale and
    /// rotation relative to a reference.
    pub pose: Pose,
    /// Source time the pose was read at.
    pub source_time: Micros,
}

/// The clip that maps `track` onto the canvas at `time`.
///
/// The named clip while it covers `time`. Otherwise the topmost visible clip
/// of the same file that does — a split, or a copy of the clip, keeps
/// following without anybody re-linking it. Failing both, the named clip
/// anyway, with `time` held at its nearer edge, so an overlay that runs past
/// the end of the video stays where the object was last seen.
pub fn target_segment<'a>(
    project: &'a Project,
    follow: &FollowMaterial,
    track: &TrackingMaterial,
    time: Micros,
) -> Option<&'a Segment> {
    let named = project
        .segment(&follow.target_segment_id)
        .map(|(_, s)| s)
        .filter(|s| s.material_id == track.media_id);
    if let Some(s) = named.filter(|s| s.target_range.contains(time)) {
        return Some(s);
    }
    let covering = project
        .segments_at(time)
        .into_iter()
        .filter(|(t, s)| {
            crate::modules::render::layout::track_is_visible(t) && s.material_id == track.media_id
        })
        .map(|(_, s)| s)
        .next_back();
    covering.or(named)
}

/// Source time of `segment` at `time`, held at its edges.
fn clamped_source_time(segment: &Segment, time: Micros) -> Micros {
    let start = segment.target_range.start;
    let end = segment.target_range.end() - 1;
    segment
        .source_time_at(time.clamp(start, end.max(start)))
        .unwrap_or(segment.source_range.start)
}

/// The quad matrix of `segment` at `time`, ignoring opacity: a clip faded to
/// nothing still has a place on the canvas, and its followers keep theirs.
fn segment_matrix(project: &Project, segment: &Segment, time: Micros) -> Option<Mat4> {
    // A stabilised clip is drawn through a moving crop window; the object is
    // where the stabilised picture shows it (`modules::analysis::stabilise`).
    let stabilised =
        crate::modules::analysis::stabilise::resolve(project, Cow::Borrowed(segment), time);
    let segment: &Segment = &stabilised;
    let video = project.materials.video(&segment.material_id)?;
    let mut transform = animated_transform(segment, time);
    transform.opacity = 1.0;
    let canvas = (project.canvas.width, project.canvas.height);
    let placement = place_quad(
        canvas,
        (video.width, video.height),
        &transform,
        segment.crop,
    )?;
    let crop = placement.crop;
    // The unit quad is ±0.5 with v = 0 at y = +0.5; fold the crop rectangle
    // in so the matrix takes source fractions directly.
    let from_source = Mat4::from_cols_array(&[
        1.0 / (crop[2] - crop[0]),
        0.0,
        0.0,
        0.0,
        0.0,
        -1.0 / (crop[3] - crop[1]),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        -crop[0] / (crop[2] - crop[0]) - 0.5,
        crop[1] / (crop[3] - crop[1]) + 0.5,
        0.0,
        1.0,
    ]);
    Some(Mat4::from_cols_array(&placement.mvp) * from_source)
}

fn project_point(matrix: &Mat4, u: f32, v: f32) -> [f32; 2] {
    let p = *matrix * Vec4::new(u, v, 0.0, 1.0);
    [p.x / p.w, p.y / p.w]
}

/// Where a point of `segment`'s source frame (fractions, top-left origin)
/// lands on the canvas at `time`.
pub fn source_to_canvas(
    project: &Project,
    segment: &Segment,
    time: Micros,
    point: [f32; 2],
) -> Option<[f32; 2]> {
    let m = segment_matrix(project, segment, time)?;
    Some(project_point(&m, point[0], point[1]))
}

/// The inverse: which source point of `segment` is under canvas point
/// `point` at `time`. What turns a box the user drew on the player into a
/// box on the video's own pixels.
pub fn canvas_to_source(
    project: &Project,
    segment: &Segment,
    time: Micros,
    point: [f32; 2],
) -> Option<[f32; 2]> {
    let m = segment_matrix(project, segment, time)?;
    // The matrix maps the z = 0 plane; drop z to invert the 2-D part.
    let a = m.to_cols_array();
    let (m00, m10, m01, m11, m03, m13) = (a[0], a[1], a[4], a[5], a[12], a[13]);
    let det = m00 * m11 - m01 * m10;
    if det.abs() < 1e-12 {
        return None;
    }
    let (x, y) = (point[0] - m03, point[1] - m13);
    Some([(m11 * x - m01 * y) / det, (-m10 * x + m00 * y) / det])
}

/// The object of `follow` on the canvas at timeline `time`.
pub fn object_at(
    project: &Project,
    follow: &FollowMaterial,
    time: Micros,
) -> Option<ObjectOnCanvas> {
    let track = project.materials.tracking(&follow.track_id)?;
    let target = target_segment(project, follow, track, time)?;
    object_through(project, track, target, time)
}

/// The object of `track` seen through `target` at `time`.
pub fn object_through(
    project: &Project,
    track: &TrackingMaterial,
    target: &Segment,
    time: Micros,
) -> Option<ObjectOnCanvas> {
    let source_time = clamped_source_time(target, time);
    let pose = track.pose_at(source_time)?;
    let m = segment_matrix(project, target, time)?;
    let (sin, cos) = pose.angle.to_radians().sin_cos();
    let (hw, hh) = (pose.w * 0.5, pose.h * 0.5);
    let video = project.materials.video(&target.material_id)?;
    // Rotate in source *pixels*, then back to fractions, or a turned box on a
    // wide frame shears.
    let (pw, ph) = (video.width.max(1) as f32, video.height.max(1) as f32);
    let corner = |dx: f32, dy: f32| {
        let (x, y) = (dx * pw, dy * ph);
        let (rx, ry) = (cos * x - sin * y, sin * x + cos * y);
        project_point(&m, pose.x + rx / pw, pose.y + ry / ph)
    };
    Some(ObjectOnCanvas {
        position: project_point(&m, pose.x, pose.y),
        corners: [
            corner(-hw, -hh),
            corner(hw, -hh),
            corner(hw, hh),
            corner(-hw, hh),
        ],
        pose,
        source_time,
    })
}

/// The transform `segment` is drawn with at `time` when it follows a track;
/// `None` when it does not follow one, or the link cannot be resolved (track
/// deleted, target gone) — the overlay then keeps its own transform.
pub fn followed_transform(project: &Project, segment: &Segment, time: Micros) -> Option<Transform> {
    let follow = project.materials.follow_of(segment)?;
    let object = object_at(project, follow, time)?;
    let track = project.materials.tracking(&follow.track_id)?;
    let reference = track.pose_at(follow.reference).unwrap_or(object.pose);
    let base = animated_transform(segment, time);
    Some(compose(project, follow, &base, &object, &reference))
}

/// The follower's transform from its own transform, the object now, and the
/// object at the reference frame.
pub fn compose(
    project: &Project,
    follow: &FollowMaterial,
    base: &Transform,
    object: &ObjectOnCanvas,
    reference: &Pose,
) -> Transform {
    let mut out = *base;
    let vector = [
        base.position[0] + follow.offset[0],
        base.position[1] + follow.offset[1],
    ];
    let (scale, angle) = relative(follow.mode, &object.pose, reference);
    // The offset turns and stretches with the object in canvas *pixels*; doing
    // it in normalised units would shear it on a non-square canvas.
    let (hw, hh) = (
        project.canvas.width.max(1) as f32 * 0.5,
        project.canvas.height.max(1) as f32 * 0.5,
    );
    let (x, y) = (vector[0] * hw, vector[1] * hh);
    // Clockwise on screen is negative in this y-up space.
    let (sin, cos) = (-angle).to_radians().sin_cos();
    let (rx, ry) = (scale * (cos * x - sin * y), scale * (sin * x + cos * y));
    out.position = [object.position[0] + rx / hw, object.position[1] + ry / hh];
    out.scale = [base.scale[0] * scale, base.scale[1] * scale];
    out.rotation = base.rotation + angle;
    out
}

/// Scale factor and added rotation since the reference pose, per the mode.
fn relative(mode: FollowMode, now: &Pose, reference: &Pose) -> (f32, f32) {
    let scale = if mode.scales() {
        (now.size() / reference.size()).clamp(0.01, 100.0)
    } else {
        1.0
    };
    let angle = if mode.rotates() {
        now.angle - reference.angle
    } else {
        0.0
    };
    (scale, angle)
}

/// `segment` as the compositor should draw it at `time`: itself, or a copy
/// whose transform is the followed one and whose transform keyframes are
/// gone (they are already inside it). Borrowed in the common case, so a
/// project without tracking pays one lookup per clip and no allocation.
pub fn resolve<'a>(project: &Project, segment: &'a Segment, time: Micros) -> Cow<'a, Segment> {
    if project.materials.follows.is_empty() {
        return Cow::Borrowed(segment);
    }
    match followed_transform(project, segment, time) {
        Some(transform) => {
            let mut copy = segment.clone();
            copy.transform = transform;
            copy.keyframes
                .retain(|t| t.property == AnimatableProperty::Volume);
            Cow::Owned(copy)
        }
        None => Cow::Borrowed(segment),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::model::{TrackSample, TrackSettings};
    use super::*;
    use crate::modules::project::document::{
        CanvasConfig, Crop, TextMaterial, TimeRange, Track, TrackKind, VideoMaterial,
    };

    fn segment(
        id: &str,
        material: &str,
        start: Micros,
        duration: Micros,
        source: Micros,
    ) -> Segment {
        Segment {
            id: id.into(),
            material_id: material.into(),
            target_range: TimeRange::new(start, duration),
            source_range: TimeRange::new(source, duration),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// A 1920×1080 canvas, a 1920×1080 clip, and a track that moves the
    /// object from the left edge (x = 0) at source 0 to the right edge
    /// (x = 1) at source 10 s, at y = 0.5.
    pub(crate) fn project() -> Project {
        let mut p = Project::new(
            "t",
            CanvasConfig {
                width: 1920,
                height: 1080,
                background: [0.0; 4],
            },
            30.0,
        );
        p.materials.videos.push(VideoMaterial {
            id: "video".into(),
            path: "/nonexistent.mp4".into(),
            width: 1920,
            height: 1080,
            duration: 10_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        p.materials.texts.push(TextMaterial {
            id: "text".into(),
            content: "hi".into(),
            font_family: "Noto Sans".into(),
            font_size: 64.0,
            color: [1.0; 4],
            bold: false,
            italic: false,
            align: Default::default(),
            stroke_width: 0.0,
            stroke_color: [0.0; 4],
            shadow: None,
            background: None,
            caption: None,
        });
        let samples = (0..=100)
            .map(|i| TrackSample {
                t: i * 100_000,
                x: i as f32 / 100.0,
                y: 0.5,
                w: 0.1,
                h: 0.1,
                a: 0.0,
                c: 1.0,
                f: 0,
            })
            .collect();
        let mut track = TrackingMaterial::new("video".into(), TrackSettings::default(), samples);
        track.id = "track".into();
        p.materials.trackings.push(track);
        p.materials.follows.push(FollowMaterial {
            id: "follow".into(),
            track_id: "track".into(),
            target_segment_id: "v".into(),
            mode: FollowMode::Position,
            offset: [0.0, 0.0],
            reference: 0,
        });
        let mut video = Track::new(TrackKind::Video, "V");
        video.segments.push(segment("v", "video", 0, 10_000_000, 0));
        let mut text = Track::new(TrackKind::Text, "T");
        let mut overlay = segment("o", "text", 0, 10_000_000, 0);
        // The order `reindex_render_order` gives two lanes, so an edit that
        // reinserts the overlay leaves it where it was.
        overlay.render_index = 1;
        overlay.extras.push("follow".into());
        text.segments.push(overlay);
        p.tracks.push(video);
        p.tracks.push(text);
        p
    }

    fn overlay(p: &Project) -> &Segment {
        p.segment("o").unwrap().1
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn the_object_lands_where_the_video_shows_it() {
        let p = project();
        // Source x 0.25 on a full-canvas clip is canvas -0.5.
        let t = followed_transform(&p, overlay(&p), 2_500_000).unwrap();
        assert!(close(t.position, [-0.5, 0.0]), "{:?}", t.position);
    }

    #[test]
    fn canvas_and_source_mapping_are_inverse() {
        let mut p = project();
        let v = &mut p.tracks[0].segments[0];
        v.transform.position = [0.2, -0.1];
        v.transform.scale = [0.5, 0.5];
        v.transform.rotation = 30.0;
        v.crop = Some(Crop {
            left: 0.1,
            top: 0.2,
            right: 0.9,
            bottom: 0.8,
        });
        let v = p.tracks[0].segments[0].clone();
        for point in [[0.3f32, 0.4f32], [0.75, 0.25], [0.5, 0.5]] {
            let canvas = source_to_canvas(&p, &v, 0, point).unwrap();
            let back = canvas_to_source(&p, &v, 0, canvas).unwrap();
            assert!(close(point, back), "{point:?} -> {canvas:?} -> {back:?}");
        }
    }

    #[test]
    fn trimming_the_video_keeps_the_overlay_on_the_object() {
        let mut p = project();
        let before = followed_transform(&p, overlay(&p), 5_000_000).unwrap();
        // Trim two seconds off the head: the clip now starts at source 2 s,
        // still at timeline 0. Timeline 3 s now shows source 5 s.
        let v = &mut p.tracks[0].segments[0];
        v.source_range = TimeRange::new(2_000_000, 8_000_000);
        v.target_range = TimeRange::new(0, 8_000_000);
        let after = followed_transform(&p, overlay(&p), 3_000_000).unwrap();
        assert!(close(before.position, after.position));
    }

    #[test]
    fn slipping_and_moving_the_video_keep_the_overlay_on_the_object() {
        let mut p = project();
        let truth = followed_transform(&p, overlay(&p), 4_000_000).unwrap();
        // Move the clip one second later on the timeline and slip it one
        // second in: timeline 4 s now shows source 4 s again.
        {
            let v = &mut p.tracks[0].segments[0];
            v.target_range = TimeRange::new(1_000_000, 9_000_000);
            v.source_range = TimeRange::new(1_000_000, 9_000_000);
        }
        let moved = followed_transform(&p, overlay(&p), 4_000_000).unwrap();
        assert!(close(truth.position, moved.position));
    }

    #[test]
    fn a_speed_change_maps_through_source_time() {
        let mut p = project();
        {
            let v = &mut p.tracks[0].segments[0];
            v.speed = 2.0;
            v.target_range = TimeRange::new(0, 5_000_000);
        }
        // Timeline 2 s at 2x is source 4 s: x = 0.4 → canvas -0.2.
        let t = followed_transform(&p, overlay(&p), 2_000_000).unwrap();
        assert!(close(t.position, [-0.2, 0.0]), "{:?}", t.position);
    }

    #[test]
    fn scaling_the_video_moves_the_overlay_with_it() {
        let mut p = project();
        p.tracks[0].segments[0].transform.scale = [0.5, 0.5];
        p.tracks[0].segments[0].transform.position = [0.5, 0.5];
        // Source x 0.25 → local -0.25 → half size → -0.25·... = 0.5 - 0.25.
        let t = followed_transform(&p, overlay(&p), 2_500_000).unwrap();
        assert!(close(t.position, [0.25, 0.5]), "{:?}", t.position);
    }

    #[test]
    fn a_split_video_still_carries_its_followers() {
        let mut p = project();
        let whole = followed_transform(&p, overlay(&p), 7_000_000).unwrap();
        let v = &mut p.tracks[0].segments[0];
        v.target_range.duration = 5_000_000;
        v.source_range.duration = 5_000_000;
        p.tracks[0]
            .segments
            .push(segment("v2", "video", 5_000_000, 5_000_000, 5_000_000));
        let split = followed_transform(&p, overlay(&p), 7_000_000).unwrap();
        assert!(close(whole.position, split.position));
    }

    #[test]
    fn the_offset_and_the_overlay_position_are_relative_to_the_object() {
        let mut p = project();
        p.materials.follows[0].offset = [0.0, 0.1];
        p.tracks[1].segments[0].transform.position = [0.05, 0.0];
        let t = followed_transform(&p, overlay(&p), 5_000_000).unwrap();
        assert!(close(t.position, [0.05, 0.1]), "{:?}", t.position);
    }

    #[test]
    fn scale_and_rotation_follow_in_their_modes_only() {
        let mut p = project();
        for (i, s) in p.materials.trackings[0].samples.iter_mut().enumerate() {
            s.w = 0.1 * (1.0 + i as f32 / 100.0);
            s.h = s.w;
            s.a = i as f32;
        }
        let at = 5_000_000;
        let plain = followed_transform(&p, overlay(&p), at).unwrap();
        assert_eq!(plain.scale, [1.0, 1.0]);
        assert_eq!(plain.rotation, 0.0);
        p.materials.follows[0].mode = FollowMode::PositionScaleRotation;
        let full = followed_transform(&p, overlay(&p), at).unwrap();
        assert!((full.scale[0] - 1.5).abs() < 1e-3);
        assert!((full.rotation - 50.0).abs() < 1e-3);
    }

    #[test]
    fn a_broken_link_leaves_the_overlay_alone() {
        let mut p = project();
        p.materials.trackings.clear();
        assert!(followed_transform(&p, overlay(&p), 0).is_none());
        assert!(matches!(resolve(&p, overlay(&p), 0), Cow::Borrowed(_)));
    }
}
