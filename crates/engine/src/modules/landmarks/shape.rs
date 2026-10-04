//! What the features read off a face: a few named points of the mesh, and
//! the face's pose for a follow link.
//!
//! The retouch passes need only the face's outline, the eyes, the mouth and
//! the jaw, so a face travels to the renderer as [`KEY_POINTS`] points, not
//! 478. Indices are MediaPipe's canonical face mesh ("subject's right" is
//! the left of the picture).

use serde::{Deserialize, Serialize};

use super::track::Face;

/// The mesh indices of the key points, in [`KeyPoint`] order.
pub const KEY_INDICES: [usize; 23] = [
    10,  // forehead, top of the face oval
    152, // chin
    234, // cheek edge, subject's right
    454, // cheek edge, subject's left
    33,  // right eye, outer corner
    133, // right eye, inner corner
    159, // right eye, upper lid
    145, // right eye, lower lid
    263, // left eye, outer corner
    362, // left eye, inner corner
    386, // left eye, upper lid
    374, // left eye, lower lid
    61,  // mouth corner, right
    291, // mouth corner, left
    0,   // upper lip, outer edge
    17,  // lower lip, outer edge
    13,  // upper lip, inner edge
    14,  // lower lip, inner edge
    58,  // jaw below the right cheek
    288, // jaw below the left cheek
    1,   // nose tip
    105, // right brow
    334, // left brow
];

/// Points of a face the renderer reads, by name.
pub mod key {
    pub const FOREHEAD: usize = 0;
    pub const CHIN: usize = 1;
    pub const CHEEK_R: usize = 2;
    pub const CHEEK_L: usize = 3;
    pub const R_EYE_OUT: usize = 4;
    pub const R_EYE_IN: usize = 5;
    pub const R_EYE_UP: usize = 6;
    pub const R_EYE_DOWN: usize = 7;
    pub const L_EYE_OUT: usize = 8;
    pub const L_EYE_IN: usize = 9;
    pub const L_EYE_UP: usize = 10;
    pub const L_EYE_DOWN: usize = 11;
    pub const MOUTH_R: usize = 12;
    pub const MOUTH_L: usize = 13;
    pub const LIP_UP: usize = 14;
    pub const LIP_DOWN: usize = 15;
    pub const INNER_UP: usize = 16;
    pub const INNER_DOWN: usize = 17;
    pub const JAW_R: usize = 18;
    pub const JAW_L: usize = 19;
    pub const NOSE: usize = 20;
    pub const BROW_R: usize = 21;
    pub const BROW_L: usize = 22;
}

/// How many key points a face carries.
pub const KEY_POINTS: usize = KEY_INDICES.len();

/// A face's key points, in whatever space the caller mapped them to.
pub type KeyPoints = [[f32; 2]; KEY_POINTS];

/// The key points of `face`, fractions of the displayed frame.
pub fn key_points(face: &Face) -> KeyPoints {
    std::array::from_fn(|i| {
        face.points
            .get(KEY_INDICES[i])
            .copied()
            .unwrap_or([0.5, 0.5])
    })
}

/// Which point of the face a follower is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    /// The middle of the face.
    #[default]
    Face,
    /// Between the eyes: glasses, an eye mask.
    Eyes,
    /// The top of the forehead: a hat, a crown, a name above the head.
    Forehead,
    Nose,
    Mouth,
    /// Below the chin: a caption that rides with the speaker.
    Chin,
}

impl Anchor {
    pub const ALL: [Anchor; 6] = [
        Anchor::Face,
        Anchor::Eyes,
        Anchor::Forehead,
        Anchor::Nose,
        Anchor::Mouth,
        Anchor::Chin,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Anchor::Face => "Face",
            Anchor::Eyes => "Eyes",
            Anchor::Forehead => "Forehead",
            Anchor::Nose => "Nose",
            Anchor::Mouth => "Mouth",
            Anchor::Chin => "Chin",
        }
    }
}

fn mid(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]
}

/// The pose of a face for a follow link: the anchor point, the face's size
/// and the eye line's angle. `aspect` is the frame's width over its height,
/// so lengths and the angle are measured in square pixels, not in fractions
/// that stretch with the frame's shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FacePose {
    /// Fractions of the displayed frame.
    pub x: f32,
    pub y: f32,
    /// The face's width (cheek to cheek) and height (forehead to chin), as
    /// fractions of the frame's width and height.
    pub w: f32,
    pub h: f32,
    /// Degrees, clockwise as the viewer sees it, of the line from the right
    /// eye to the left.
    pub angle: f32,
}

pub fn pose(face: &Face, anchor: Anchor, aspect: f32) -> FacePose {
    use key::*;
    let k = key_points(face);
    let eyes = mid(
        mid(k[R_EYE_OUT], k[R_EYE_IN]),
        mid(k[L_EYE_OUT], k[L_EYE_IN]),
    );
    let centre = mid(mid(k[FOREHEAD], k[CHIN]), mid(k[CHEEK_R], k[CHEEK_L]));
    let point = match anchor {
        Anchor::Face => centre,
        Anchor::Eyes => eyes,
        Anchor::Forehead => k[FOREHEAD],
        Anchor::Nose => k[NOSE],
        Anchor::Mouth => mid(k[INNER_UP], k[INNER_DOWN]),
        Anchor::Chin => k[CHIN],
    };
    let dist = |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0]) * aspect).hypot(a[1] - b[1]);
    let (r, l) = (
        mid(k[R_EYE_OUT], k[R_EYE_IN]),
        mid(k[L_EYE_OUT], k[L_EYE_IN]),
    );
    let angle = ((l[1] - r[1]).atan2((l[0] - r[0]) * aspect)).to_degrees();
    let width = dist(k[CHEEK_R], k[CHEEK_L]);
    let height = dist(k[FOREHEAD], k[CHIN]);
    FacePose {
        x: point[0],
        y: point[1],
        w: width / aspect,
        h: height,
        angle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::landmarks::track::POINTS;

    /// A face whose key points sit where a level face's would, centred at
    /// (0.5, 0.5), turned by `degrees`.
    pub(crate) fn synthetic(degrees: f32) -> Face {
        let mut points = vec![[0.5f32, 0.5]; POINTS];
        let place = |dx: f32, dy: f32| {
            let (s, c) = degrees.to_radians().sin_cos();
            [0.5 + dx * c - dy * s, 0.5 + dx * s + dy * c]
        };
        let layout: [(usize, f32, f32); 12] = [
            (10, 0.0, -0.2),
            (152, 0.0, 0.2),
            (234, -0.15, 0.0),
            (454, 0.15, 0.0),
            (33, -0.1, -0.05),
            (133, -0.04, -0.05),
            (263, 0.1, -0.05),
            (362, 0.04, -0.05),
            (13, 0.0, 0.1),
            (14, 0.0, 0.12),
            (1, 0.0, 0.03),
            (152, 0.0, 0.2),
        ];
        for (i, dx, dy) in layout {
            points[i] = place(dx, dy);
        }
        Face { score: 1.0, points }
    }

    #[test]
    fn the_pose_reads_position_size_and_tilt() {
        let level = pose(&synthetic(0.0), Anchor::Face, 1.0);
        assert!((level.x - 0.5).abs() < 1e-5 && (level.y - 0.5).abs() < 1e-5);
        assert!(level.angle.abs() < 1e-3);
        assert!((level.w - 0.3).abs() < 1e-4 && (level.h - 0.4).abs() < 1e-4);
        let tilted = pose(&synthetic(20.0), Anchor::Face, 1.0);
        assert!((tilted.angle - 20.0).abs() < 1e-2, "{tilted:?}");
        let chin = pose(&synthetic(0.0), Anchor::Chin, 1.0);
        assert!((chin.y - 0.7).abs() < 1e-5);
        let eyes = pose(&synthetic(0.0), Anchor::Eyes, 1.0);
        assert!((eyes.y - 0.45).abs() < 1e-5);
    }
}
