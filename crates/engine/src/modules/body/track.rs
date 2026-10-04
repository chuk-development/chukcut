//! A clip's body track: the people of every analysed source frame, each
//! with COCO's 17 keypoints, and the cache file it lives in.
//!
//! ## Units
//!
//! Points are fractions of the **displayed** source frame (rotation
//! applied), `0,0` top-left — the convention of the face track
//! (`landmarks::track`) and of motion tracks.
//!
//! ## Identity
//!
//! Each person carries an `id`, given in the order people are first seen
//! (the largest first on a frame where several appear together) and kept
//! from frame to frame by box overlap ([`Identities`]). "Person 1" is id 0
//! on every frame they are in, also after a frame where they were missed.
//!
//! ## The file
//!
//! `<cache>/chukcut/landmarks/<media digest>-rtmpose-<version>.bdy`, one per
//! media file and model version, whatever range of the file was analysed:
//!
//! ```text
//! b"CCBDY001"  u32 keypoints per person (17)
//! per frame:   i64 source time µs, u8 people,
//!              per person: u8 id, f32 score,
//!                          keypoints × (u16 x, u16 y as fraction × 65535,
//!                                       u8 confidence × 255)
//! ```
//!
//! 90 bytes per person per frame: a minute of one person at 30 fps is
//! 160 kB. Little-endian, sorted by time. Cache, not document (decision
//! 0019); a follower's motion track is what goes into the document.

use std::path::{Path, PathBuf};

use crate::modules::project::document::Micros;
use crate::modules::proxy::cache::SourceKey;

const MAGIC: &[u8; 8] = b"CCBDY001";
/// Keypoints per person in the file: COCO's 17.
pub const KEYPOINTS: usize = 17;
/// How long a person who was not seen keeps their id for a match.
pub const MEMORY: Micros = 2_000_000;
/// The box overlap at which a new box is taken for a known person.
const SAME_PERSON: f32 = 0.2;

/// One person in one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    pub id: u8,
    /// The mean keypoint confidence, about 0..1.
    pub score: f32,
    /// x, y (fractions of the displayed frame), confidence (0..1, below
    /// `rtmpose::SEEN` = 0.3 not seen), in COCO order.
    pub points: [[f32; 3]; KEYPOINTS],
}

impl Person {
    /// The box the seen keypoints span, x, y, w, h in fractions; `None`
    /// with fewer than two seen.
    pub fn bbox(&self) -> Option<[f32; 4]> {
        chukcut_ml_worker::rtmpose::bbox(&self.points)
    }
}

/// The people of one source frame, by id.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyFrame {
    pub t: Micros,
    pub people: Vec<Person>,
}

/// The track file of `media` for model `version`. Reads the file's head and
/// tail for its digest, so callers keep the answer.
pub fn path_for(media: &Path, version: &str) -> Result<PathBuf, String> {
    let key = SourceKey::of(media).map_err(|e| format!("cannot read {}: {e}", media.display()))?;
    let version: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(crate::modules::landmarks::track::root()
        .join(format!("{}-rtmpose-{version}.bdy", key.digest())))
}

fn quantise(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// The file's bytes.
pub fn encode(frames: &[BodyFrame]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + frames.len() * (9 + 5 + KEYPOINTS * 5));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(KEYPOINTS as u32).to_le_bytes());
    for frame in frames {
        out.extend_from_slice(&frame.t.to_le_bytes());
        out.push(frame.people.len().min(255) as u8);
        for person in frame.people.iter().take(255) {
            out.push(person.id);
            out.extend_from_slice(&person.score.to_le_bytes());
            for p in &person.points {
                out.extend_from_slice(&quantise(p[0]).to_le_bytes());
                out.extend_from_slice(&quantise(p[1]).to_le_bytes());
                out.push((p[2].clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
    }
    out
}

/// Parse a file's bytes. A file cut short keeps its whole frames.
pub fn decode(bytes: &[u8]) -> Result<Vec<BodyFrame>, String> {
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err("not a body track".into());
    }
    let points = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes")) as usize;
    if points != KEYPOINTS {
        return Err(format!("a track of {points}-point bodies, not {KEYPOINTS}"));
    }
    let mut frames = Vec::new();
    let mut at = 12;
    let take = |at: &mut usize, n: usize| -> Option<&[u8]> {
        let slice = bytes.get(*at..*at + n)?;
        *at += n;
        Some(slice)
    };
    'frames: while at < bytes.len() {
        let Some(t) = take(&mut at, 8) else { break };
        let t = i64::from_le_bytes(t.try_into().expect("8 bytes"));
        let Some(&[count]) = take(&mut at, 1) else {
            break;
        };
        let mut people = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let Some(&[id]) = take(&mut at, 1) else {
                break 'frames;
            };
            let Some(score) = take(&mut at, 4) else {
                break 'frames;
            };
            let score = f32::from_le_bytes(score.try_into().expect("4 bytes"));
            let Some(raw) = take(&mut at, KEYPOINTS * 5) else {
                break 'frames;
            };
            let points = std::array::from_fn(|i| {
                let b = &raw[i * 5..i * 5 + 5];
                [
                    u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0,
                    u16::from_le_bytes([b[2], b[3]]) as f32 / 65535.0,
                    b[4] as f32 / 255.0,
                ]
            });
            people.push(Person { id, score, points });
        }
        frames.push(BodyFrame { t, people });
    }
    Ok(frames)
}

/// The frames in the file at `path`; none when there is no file.
pub fn read(path: &Path) -> Result<Vec<BodyFrame>, String> {
    match std::fs::read(path) {
        Ok(bytes) => decode(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// `new` merged into what `path` holds (a frame analysed again replaces
/// the old one), written atomically.
pub fn merge_into(path: &Path, new: Vec<BodyFrame>) -> Result<(), String> {
    let mut frames = read(path).unwrap_or_default();
    frames.extend(new);
    // Stable: of two entries for one time, the later (new) one is kept.
    frames.sort_by_key(|f| f.t);
    let mut merged: Vec<BodyFrame> = Vec::with_capacity(frames.len());
    for frame in frames {
        match merged.last_mut() {
            Some(last) if last.t == frame.t => *last = frame,
            _ => merged.push(frame),
        }
    }
    crate::modules::workspace::atomic::write_atomically(path, &encode(&merged))
}

/// The people at `source_time`: between two analysed frames no more than
/// `max_gap` apart, each person in both mixed linearly (keypoint by
/// keypoint; one unseen in either frame takes the lower confidence);
/// otherwise the nearer frame within `max_gap`; otherwise none.
pub fn people_at(frames: &[BodyFrame], source_time: Micros, max_gap: Micros) -> Vec<Person> {
    if frames.is_empty() {
        return Vec::new();
    }
    let i = frames.partition_point(|f| f.t <= source_time);
    let before = i.checked_sub(1).map(|i| &frames[i]);
    let after = frames.get(i);
    match (before, after) {
        (Some(a), Some(b)) if b.t - a.t <= max_gap && b.t > a.t => {
            let k = (source_time - a.t) as f32 / (b.t - a.t) as f32;
            let near = if k < 0.5 { a } else { b };
            near.people
                .iter()
                .map(|p| {
                    let (Some(pa), Some(pb)) = (
                        a.people.iter().find(|q| q.id == p.id),
                        b.people.iter().find(|q| q.id == p.id),
                    ) else {
                        return p.clone();
                    };
                    Person {
                        id: p.id,
                        score: pa.score + (pb.score - pa.score) * k,
                        points: std::array::from_fn(|j| {
                            let (u, v) = (pa.points[j], pb.points[j]);
                            [
                                u[0] + (v[0] - u[0]) * k,
                                u[1] + (v[1] - u[1]) * k,
                                u[2].min(v[2]),
                            ]
                        }),
                    }
                })
                .collect()
        }
        _ => {
            let near = [before, after]
                .into_iter()
                .flatten()
                .filter(|f| (f.t - source_time).abs() <= max_gap)
                .min_by_key(|f| (f.t - source_time).abs());
            near.map(|f| f.people.clone()).unwrap_or_default()
        }
    }
}

/// How much of `[start, end)` the frames cover, as the share of `period`
/// steps that have an analysed frame within half a period.
pub fn coverage(
    frames: &[BodyFrame],
    start: Micros,
    end: Micros,
    period: Micros,
) -> (usize, usize) {
    let period = period.max(1);
    let (mut total, mut have) = (0, 0);
    let mut t = start;
    while t < end {
        total += 1;
        let i = frames.partition_point(|f| f.t < t - period / 2);
        if frames.get(i).is_some_and(|f| (f.t - t).abs() <= period / 2) {
            have += 1;
        }
        t += period;
    }
    (have, total)
}

/// Gives the people of each frame their ids: a box overlapping a person
/// seen within [`MEMORY`] is that person; anyone else is new.
#[derive(Debug, Default)]
pub struct Identities {
    /// Each known person's id, last box and when it was seen.
    known: Vec<(u8, [f32; 4], Micros)>,
    next: u16,
}

impl Identities {
    /// Ids for the boxes of the frame at `t`, in their order.
    pub fn assign(&mut self, t: Micros, boxes: &[[f32; 4]]) -> Vec<u8> {
        use chukcut_ml_worker::rtmpose::iou;
        self.known.retain(|k| t - k.2 <= MEMORY);
        let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
        for (i, b) in boxes.iter().enumerate() {
            for (j, k) in self.known.iter().enumerate() {
                let overlap = iou(*b, k.1);
                if overlap >= SAME_PERSON {
                    pairs.push((overlap, i, j));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut ids: Vec<Option<u8>> = vec![None; boxes.len()];
        let mut taken = vec![false; self.known.len()];
        for (_, i, j) in pairs {
            if ids[i].is_none() && !taken[j] {
                ids[i] = Some(self.known[j].0);
                taken[j] = true;
            }
        }
        let ids: Vec<u8> = ids
            .into_iter()
            .map(|id| {
                id.unwrap_or_else(|| {
                    // Past 255 people the last id is shared; a clip with
                    // that many is not one anyone follows a hand in.
                    let id = self.next.min(255) as u8;
                    self.next += 1;
                    id
                })
            })
            .collect();
        for (id, b) in ids.iter().zip(boxes) {
            match self.known.iter_mut().find(|k| k.0 == *id) {
                Some(k) => *k = (*id, *b, t),
                None => self.known.push((*id, *b, t)),
            }
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(id: u8, x: f32) -> Person {
        Person {
            id,
            score: 0.8,
            points: std::array::from_fn(|i| [x, i as f32 / KEYPOINTS as f32, 0.9]),
        }
    }

    #[test]
    fn a_track_round_trips_through_its_file_format() {
        let frames = vec![
            BodyFrame {
                t: 0,
                people: vec![person(0, 0.25), person(1, 0.75)],
            },
            BodyFrame {
                t: 33_333,
                people: vec![],
            },
            BodyFrame {
                t: 66_667,
                people: vec![person(1, 0.5)],
            },
        ];
        let bytes = encode(&frames);
        let back = decode(&bytes).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[2].people[0].id, 1);
        for (a, b) in frames[0].people[1]
            .points
            .iter()
            .zip(&back[0].people[1].points)
        {
            assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4);
            assert!((a[2] - b[2]).abs() < 0.003);
        }
        assert_eq!(decode(&bytes[..bytes.len() - 10]).unwrap().len(), 2);
        assert!(decode(b"CCLMK001\x0e\x01\0\0").is_err());
    }

    #[test]
    fn between_frames_a_person_is_mixed_by_id_and_far_from_any_there_is_none() {
        let frames = vec![
            BodyFrame {
                t: 0,
                people: vec![person(0, 0.2), person(1, 0.9)],
            },
            BodyFrame {
                t: 100,
                // Person 1 first this time: the mix goes by id, not order.
                people: vec![person(1, 0.7), person(0, 0.4)],
            },
        ];
        let mid = people_at(&frames, 25, 200);
        let zero = mid.iter().find(|p| p.id == 0).unwrap();
        assert!((zero.points[0][0] - 0.25).abs() < 1e-6);
        let one = mid.iter().find(|p| p.id == 1).unwrap();
        assert!((one.points[0][0] - 0.85).abs() < 1e-6);
        assert!(people_at(&frames, 1_000, 200).is_empty());
        assert!((people_at(&frames, 30, 50)[0].points[0][0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn identities_follow_boxes_and_a_newcomer_gets_the_next_id() {
        let mut ids = Identities::default();
        assert_eq!(
            ids.assign(0, &[[0.1, 0.1, 0.3, 0.8], [0.6, 0.1, 0.2, 0.6]]),
            vec![0, 1]
        );
        // Order swapped and both moved a little: same people.
        assert_eq!(
            ids.assign(33_333, &[[0.62, 0.1, 0.2, 0.6], [0.12, 0.1, 0.3, 0.8]]),
            vec![1, 0]
        );
        // Person 0 missed for a frame, then back; a third walks in.
        assert_eq!(ids.assign(66_667, &[[0.62, 0.1, 0.2, 0.6]]), vec![1]);
        assert_eq!(
            ids.assign(100_000, &[[0.13, 0.1, 0.3, 0.8], [0.85, 0.5, 0.1, 0.3]]),
            vec![0, 2]
        );
        // Gone longer than the memory: a new person.
        assert_eq!(ids.assign(5_000_000, &[[0.13, 0.1, 0.3, 0.8]]), vec![3]);
    }
}
