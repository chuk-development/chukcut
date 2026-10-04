//! What a follower reads off a body: a body part's position, size and
//! angle, from COCO's 17 keypoints.
//!
//! "Left" and "right" are the **person's own**, as in COCO (a person facing
//! the camera has their left hand on the picture's right). The angle of a
//! limb part is the limb's direction turned so that a limb hanging straight
//! down is 0°: a sticker in a hand turns with the forearm. Torso parts and
//! the head turn with the shoulder, hip or eye line, measured from the
//! person's right to their left, as the face's eye line is
//! (`landmarks::shape`).

use serde::{Deserialize, Serialize};

use super::track::Person;
use chukcut_ml_worker::rtmpose::SEEN;

/// COCO's keypoints, by name.
pub mod kp {
    pub const NOSE: usize = 0;
    pub const L_EYE: usize = 1;
    pub const R_EYE: usize = 2;
    pub const L_EAR: usize = 3;
    pub const R_EAR: usize = 4;
    pub const L_SHOULDER: usize = 5;
    pub const R_SHOULDER: usize = 6;
    pub const L_ELBOW: usize = 7;
    pub const R_ELBOW: usize = 8;
    pub const L_WRIST: usize = 9;
    pub const R_WRIST: usize = 10;
    pub const L_HIP: usize = 11;
    pub const R_HIP: usize = 12;
    pub const L_KNEE: usize = 13;
    pub const R_KNEE: usize = 14;
    pub const L_ANKLE: usize = 15;
    pub const R_ANKLE: usize = 16;
}

/// Which part of a body a follower is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyPart {
    /// The middle of the head (eyes and ears): a hat, a halo, a name tag.
    Head,
    /// Between the shoulders.
    Shoulders,
    /// The chest: a third of the way from the shoulders to the hips.
    #[default]
    Chest,
    /// Between the hips.
    Hips,
    /// The middle of the whole body.
    Body,
    LeftHand,
    RightHand,
    LeftElbow,
    RightElbow,
    LeftKnee,
    RightKnee,
    LeftFoot,
    RightFoot,
}

impl BodyPart {
    pub const ALL: [BodyPart; 13] = [
        BodyPart::Head,
        BodyPart::Shoulders,
        BodyPart::Chest,
        BodyPart::Hips,
        BodyPart::Body,
        BodyPart::LeftHand,
        BodyPart::RightHand,
        BodyPart::LeftElbow,
        BodyPart::RightElbow,
        BodyPart::LeftKnee,
        BodyPart::RightKnee,
        BodyPart::LeftFoot,
        BodyPart::RightFoot,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BodyPart::Head => "Head",
            BodyPart::Shoulders => "Shoulders",
            BodyPart::Chest => "Chest",
            BodyPart::Hips => "Hips",
            BodyPart::Body => "Whole body",
            BodyPart::LeftHand => "Left hand (theirs)",
            BodyPart::RightHand => "Right hand (theirs)",
            BodyPart::LeftElbow => "Left elbow (theirs)",
            BodyPart::RightElbow => "Right elbow (theirs)",
            BodyPart::LeftKnee => "Left knee (theirs)",
            BodyPart::RightKnee => "Right knee (theirs)",
            BodyPart::LeftFoot => "Left foot (theirs)",
            BodyPart::RightFoot => "Right foot (theirs)",
        }
    }

    /// The part named `name` (`left_hand`, `left-hand`, `Left hand`).
    pub fn parse(name: &str) -> Option<BodyPart> {
        let key: String = name
            .trim()
            .chars()
            .map(|c| {
                if c == '-' || c == ' ' {
                    '_'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        serde_json::from_value(serde_json::Value::String(key)).ok()
    }
}

/// A body part's pose for a follow link.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartPose {
    /// Fractions of the displayed frame.
    pub x: f32,
    pub y: f32,
    /// The part's size, as fractions of the frame's width and height.
    pub w: f32,
    pub h: f32,
    /// Degrees, clockwise as the viewer sees it.
    pub angle: f32,
    /// The lowest confidence of the keypoints it was read from.
    pub confidence: f32,
}

/// The pose of `part` of `person`, or `None` when a keypoint it needs was
/// not seen in this frame. `aspect` is the frame's width over its height,
/// so lengths and angles are measured in square pixels.
pub fn pose(person: &Person, part: BodyPart, aspect: f32) -> Option<PartPose> {
    use kp::*;
    let p = &person.points;
    let seen = |i: usize| (p[i][2] >= SEEN).then_some(i);
    let at = |i: usize| [p[i][0], p[i][1]];
    let mid = |a: [f32; 2], b: [f32; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let dist = |a: [f32; 2], b: [f32; 2]| ((a[0] - b[0]) * aspect).hypot(a[1] - b[1]);
    // Of the line from `a` to `b`, in degrees.
    let line =
        |a: [f32; 2], b: [f32; 2]| ((b[1] - a[1]).atan2((b[0] - a[0]) * aspect)).to_degrees();
    // A limb from `a` to `b`, hanging straight down at 0°.
    let limb = |a: [f32; 2], b: [f32; 2]| line(a, b) - 90.0;
    let conf = |ids: &[usize]| ids.iter().map(|&i| p[i][2]).fold(1.0f32, f32::min);
    let pair = |l: usize, r: usize| Some((seen(l)?, seen(r)?));

    // The body's own scale: shoulder width, else hip width, else the
    // keypoints' height. Parts without a length of their own use it.
    let scale = pair(L_SHOULDER, R_SHOULDER)
        .map(|(l, r)| dist(at(l), at(r)))
        .or_else(|| pair(L_HIP, R_HIP).map(|(l, r)| dist(at(l), at(r)) * 1.4))
        .or_else(|| person.bbox().map(|b| b[3] * 0.25))
        .filter(|s| *s > 0.0)?;
    let square = |x: f32, y: f32, size: f32, angle: f32, confidence: f32| PartPose {
        x,
        y,
        w: size / aspect,
        h: size,
        angle,
        confidence,
    };
    let shoulders = pair(L_SHOULDER, R_SHOULDER);
    let hips = pair(L_HIP, R_HIP);
    let shoulder_angle = shoulders.map(|(l, r)| line(at(r), at(l)));

    match part {
        BodyPart::Head => {
            let face: Vec<usize> = [L_EYE, R_EYE, L_EAR, R_EAR, NOSE]
                .into_iter()
                .filter_map(seen)
                .collect();
            if face.is_empty() {
                return None;
            }
            let n = face.len() as f32;
            let x = face.iter().map(|&i| p[i][0]).sum::<f32>() / n;
            let y = face.iter().map(|&i| p[i][1]).sum::<f32>() / n;
            let size = pair(L_EAR, R_EAR)
                .map(|(l, r)| dist(at(l), at(r)) * 1.2)
                .or_else(|| pair(L_EYE, R_EYE).map(|(l, r)| dist(at(l), at(r)) * 2.6))
                .unwrap_or(scale * 0.6);
            let angle = pair(L_EYE, R_EYE)
                .map(|(l, r)| line(at(r), at(l)))
                .or_else(|| pair(L_EAR, R_EAR).map(|(l, r)| line(at(r), at(l))))
                .or(shoulder_angle)
                .unwrap_or(0.0);
            Some(square(x, y, size, angle, conf(&face)))
        }
        BodyPart::Shoulders => {
            let (l, r) = shoulders?;
            let c = mid(at(l), at(r));
            Some(square(c[0], c[1], scale, line(at(r), at(l)), conf(&[l, r])))
        }
        BodyPart::Chest => {
            let (l, r) = shoulders?;
            let (hl, hr) = hips?;
            let s = mid(at(l), at(r));
            let h = mid(at(hl), at(hr));
            let x = s[0] + (h[0] - s[0]) / 3.0;
            let y = s[1] + (h[1] - s[1]) / 3.0;
            Some(square(
                x,
                y,
                scale,
                line(at(r), at(l)),
                conf(&[l, r, hl, hr]),
            ))
        }
        BodyPart::Hips => {
            let (l, r) = hips?;
            let c = mid(at(l), at(r));
            let size = dist(at(l), at(r)).max(scale * 0.5);
            Some(square(c[0], c[1], size, line(at(r), at(l)), conf(&[l, r])))
        }
        BodyPart::Body => {
            let [x, y, w, h] = person.bbox()?;
            Some(PartPose {
                x: x + w / 2.0,
                y: y + h / 2.0,
                w,
                h,
                angle: shoulder_angle.unwrap_or(0.0),
                confidence: person.score,
            })
        }
        BodyPart::LeftHand | BodyPart::RightHand => {
            let (elbow, wrist) = if part == BodyPart::LeftHand {
                (L_ELBOW, L_WRIST)
            } else {
                (R_ELBOW, R_WRIST)
            };
            let w = seen(wrist)?;
            match seen(elbow) {
                Some(e) => {
                    // The hand's middle is past the wrist, a third of a
                    // forearm further along it.
                    let (a, b) = (at(e), at(w));
                    let x = b[0] + (b[0] - a[0]) * 0.3;
                    let y = b[1] + (b[1] - a[1]) * 0.3;
                    let size = (dist(a, b) * 0.6).max(scale * 0.3);
                    Some(square(x, y, size, limb(a, b), conf(&[e, w])))
                }
                None => Some(square(p[w][0], p[w][1], scale * 0.4, 0.0, conf(&[w]))),
            }
        }
        BodyPart::LeftElbow | BodyPart::RightElbow => {
            let (shoulder, elbow) = if part == BodyPart::LeftElbow {
                (L_SHOULDER, L_ELBOW)
            } else {
                (R_SHOULDER, R_ELBOW)
            };
            let e = seen(elbow)?;
            let angle = seen(shoulder).map_or(0.0, |s| limb(at(s), at(e)));
            Some(square(p[e][0], p[e][1], scale * 0.4, angle, conf(&[e])))
        }
        BodyPart::LeftKnee | BodyPart::RightKnee => {
            let (hip, knee) = if part == BodyPart::LeftKnee {
                (L_HIP, L_KNEE)
            } else {
                (R_HIP, R_KNEE)
            };
            let k = seen(knee)?;
            let angle = seen(hip).map_or(0.0, |h| limb(at(h), at(k)));
            Some(square(p[k][0], p[k][1], scale * 0.5, angle, conf(&[k])))
        }
        BodyPart::LeftFoot | BodyPart::RightFoot => {
            let (knee, ankle) = if part == BodyPart::LeftFoot {
                (L_KNEE, L_ANKLE)
            } else {
                (R_KNEE, R_ANKLE)
            };
            let a = seen(ankle)?;
            let angle = seen(knee).map_or(0.0, |k| limb(at(k), at(a)));
            Some(square(p[a][0], p[a][1], scale * 0.5, angle, conf(&[a])))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::body::track::KEYPOINTS;

    /// A person facing the camera, arms hanging, in a square frame.
    fn standing() -> Person {
        let mut points = [[0.0f32, 0.0, 0.0]; KEYPOINTS];
        let mut put = |i: usize, x: f32, y: f32| points[i] = [x, y, 0.9];
        put(kp::NOSE, 0.5, 0.15);
        put(kp::R_EYE, 0.48, 0.13);
        put(kp::L_EYE, 0.52, 0.13);
        put(kp::R_EAR, 0.46, 0.14);
        put(kp::L_EAR, 0.54, 0.14);
        put(kp::R_SHOULDER, 0.42, 0.25);
        put(kp::L_SHOULDER, 0.58, 0.25);
        put(kp::R_ELBOW, 0.40, 0.40);
        put(kp::L_ELBOW, 0.60, 0.40);
        put(kp::R_WRIST, 0.40, 0.55);
        put(kp::L_WRIST, 0.60, 0.55);
        put(kp::R_HIP, 0.45, 0.55);
        put(kp::L_HIP, 0.55, 0.55);
        put(kp::R_KNEE, 0.45, 0.72);
        put(kp::L_KNEE, 0.55, 0.72);
        put(kp::R_ANKLE, 0.45, 0.9);
        put(kp::L_ANKLE, 0.55, 0.9);
        Person {
            id: 0,
            score: 0.9,
            points,
        }
    }

    #[test]
    fn parts_sit_where_they_should_and_upright_is_level() {
        let person = standing();
        let hips = pose(&person, BodyPart::Hips, 1.0).unwrap();
        assert!((hips.x - 0.5).abs() < 1e-6 && (hips.y - 0.55).abs() < 1e-6);
        assert!(hips.angle.abs() < 1e-4);
        let chest = pose(&person, BodyPart::Chest, 1.0).unwrap();
        assert!((chest.y - 0.35).abs() < 1e-6);
        assert!((chest.w - 0.16).abs() < 1e-5);
        let head = pose(&person, BodyPart::Head, 1.0).unwrap();
        assert!(head.y < 0.15 && (head.x - 0.5).abs() < 1e-6);
        // A hand hangs straight down: 0°, past the wrist.
        let hand = pose(&person, BodyPart::LeftHand, 1.0).unwrap();
        assert!(hand.angle.abs() < 1e-3, "{}", hand.angle);
        assert!((hand.x - 0.6).abs() < 1e-6 && hand.y > 0.55);
        let body = pose(&person, BodyPart::Body, 1.0).unwrap();
        assert!((body.h - 0.77).abs() < 1e-5);
    }

    #[test]
    fn a_raised_arm_turns_the_hand_and_an_unseen_part_is_none() {
        let mut person = standing();
        // The right forearm pointing to the picture's left (the person's
        // right, held out): the limb's direction is 180°, so -90 + 180.
        person.points[kp::R_WRIST] = [0.25, 0.40, 0.9];
        let hand = pose(&person, BodyPart::RightHand, 1.0).unwrap();
        assert!((hand.angle - 90.0).abs() < 1e-3, "{}", hand.angle);
        person.points[kp::L_WRIST][2] = 0.1;
        assert!(pose(&person, BodyPart::LeftHand, 1.0).is_none());
        // Without hips there is no chest, but there are shoulders.
        person.points[kp::L_HIP][2] = 0.0;
        assert!(pose(&person, BodyPart::Chest, 1.0).is_none());
        assert!(pose(&person, BodyPart::Shoulders, 1.0).is_some());
    }

    #[test]
    fn parts_parse_from_their_names() {
        assert_eq!(BodyPart::parse("left_hand"), Some(BodyPart::LeftHand));
        assert_eq!(BodyPart::parse("Right-Foot"), Some(BodyPart::RightFoot));
        assert_eq!(BodyPart::parse("hips"), Some(BodyPart::Hips));
        assert_eq!(BodyPart::parse("tail"), None);
        for part in BodyPart::ALL {
            let name = serde_json::to_value(part).unwrap();
            assert_eq!(BodyPart::parse(name.as_str().unwrap()), Some(part));
        }
    }
}
