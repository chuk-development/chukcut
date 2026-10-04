//! A clip's landmark track: the faces of every analysed source frame, with
//! their 478 points, and the cache file it lives in.
//!
//! ## Units
//!
//! Points are fractions of the **displayed** source frame (rotation applied),
//! `0,0` top-left — the tracking module's convention (`tracking::model`), so
//! a track survives a proxy, an analysis size and a re-import at another
//! resolution.
//!
//! ## The file
//!
//! `<cache>/chukcut/landmarks/<media digest>-facemesh-<version>.lmk`, one per
//! media file and model version, whatever range of the file was analysed:
//!
//! ```text
//! b"CCLMK001"  u32 points per face (478)
//! per frame:   i64 source time µs, u8 faces,
//!              per face: f32 score, points × (u16 x, u16 y) as fraction × 65535
//! ```
//!
//! 1.9 kB per face per frame: a minute at 30 fps is 3.4 MB. Little-endian,
//! sorted by time. Cache, not document (decision 0019's rule for derived
//! pixels and dense analysis): the document records only what uses it.

use std::path::{Path, PathBuf};

use crate::modules::project::document::Micros;
use crate::modules::proxy::cache::SourceKey;

const MAGIC: &[u8; 8] = b"CCLMK001";
/// Points per face in the file: MediaPipe's 468 plus two irises of five.
pub const POINTS: usize = 478;

/// One face in one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    pub score: f32,
    /// [`POINTS`] points, fractions of the displayed frame.
    pub points: Vec<[f32; 2]>,
}

/// The faces of one source frame, largest first.
#[derive(Debug, Clone, PartialEq)]
pub struct FaceFrame {
    pub t: Micros,
    pub faces: Vec<Face>,
}

/// The cache directory.
pub fn root() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("landmarks")
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
    Ok(root().join(format!("{}-facemesh-{version}.lmk", key.digest())))
}

fn quantise(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// The file's bytes.
pub fn encode(frames: &[FaceFrame]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + frames.len() * (9 + POINTS * 4 + 4));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(POINTS as u32).to_le_bytes());
    for frame in frames {
        out.extend_from_slice(&frame.t.to_le_bytes());
        out.push(frame.faces.len().min(255) as u8);
        for face in frame.faces.iter().take(255) {
            out.extend_from_slice(&face.score.to_le_bytes());
            for i in 0..POINTS {
                let p = face.points.get(i).copied().unwrap_or([0.0, 0.0]);
                out.extend_from_slice(&quantise(p[0]).to_le_bytes());
                out.extend_from_slice(&quantise(p[1]).to_le_bytes());
            }
        }
    }
    out
}

/// Parse a file's bytes. A file cut short (a crash mid-write cannot make
/// one, the writes are atomic, but a full disk might) keeps its whole
/// frames.
pub fn decode(bytes: &[u8]) -> Result<Vec<FaceFrame>, String> {
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err("not a landmark track".into());
    }
    let points = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes")) as usize;
    if points != POINTS {
        return Err(format!("a track of {points}-point faces, not {POINTS}"));
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
        let mut faces = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let Some(score) = take(&mut at, 4) else {
                break 'frames;
            };
            let score = f32::from_le_bytes(score.try_into().expect("4 bytes"));
            let Some(raw) = take(&mut at, POINTS * 4) else {
                break 'frames;
            };
            let points = raw
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| {
                    [
                        u16::from_le_bytes([b[0], b[1]]) as f32 / 65535.0,
                        u16::from_le_bytes([b[2], b[3]]) as f32 / 65535.0,
                    ]
                })
                .collect();
            faces.push(Face { score, points });
        }
        frames.push(FaceFrame { t, faces });
    }
    Ok(frames)
}

/// The frames in the file at `path`; none when there is no file.
pub fn read(path: &Path) -> Result<Vec<FaceFrame>, String> {
    match std::fs::read(path) {
        Ok(bytes) => decode(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("cannot read {}: {e}", path.display())),
    }
}

/// `new` merged into what `path` holds (a frame analysed again replaces
/// the old one), written atomically.
pub fn merge_into(path: &Path, new: Vec<FaceFrame>) -> Result<(), String> {
    let mut frames = read(path).unwrap_or_default();
    frames.extend(new);
    // Stable: of two entries for one time, the later (new) one is kept.
    frames.sort_by_key(|f| f.t);
    let mut merged: Vec<FaceFrame> = Vec::with_capacity(frames.len());
    for frame in frames {
        match merged.last_mut() {
            Some(last) if last.t == frame.t => *last = frame,
            _ => merged.push(frame),
        }
    }
    crate::modules::workspace::atomic::write_atomically(path, &encode(&merged))
}

/// The faces at `source_time`: between two analysed frames no more than
/// `max_gap` apart, the points mixed linearly (faces matched by order, both
/// frames having the same count); otherwise the nearer frame within
/// `max_gap`; otherwise none.
pub fn faces_at(frames: &[FaceFrame], source_time: Micros, max_gap: Micros) -> Vec<Face> {
    if frames.is_empty() {
        return Vec::new();
    }
    let i = frames.partition_point(|f| f.t <= source_time);
    let before = i.checked_sub(1).map(|i| &frames[i]);
    let after = frames.get(i);
    match (before, after) {
        (Some(a), Some(b))
            if b.t - a.t <= max_gap && a.faces.len() == b.faces.len() && b.t > a.t =>
        {
            let k = (source_time - a.t) as f32 / (b.t - a.t) as f32;
            a.faces
                .iter()
                .zip(&b.faces)
                .map(|(fa, fb)| Face {
                    score: fa.score + (fb.score - fa.score) * k,
                    points: fa
                        .points
                        .iter()
                        .zip(&fb.points)
                        .map(|(p, q)| [p[0] + (q[0] - p[0]) * k, p[1] + (q[1] - p[1]) * k])
                        .collect(),
                })
                .collect()
        }
        _ => {
            let near = [before, after]
                .into_iter()
                .flatten()
                .filter(|f| (f.t - source_time).abs() <= max_gap)
                .min_by_key(|f| (f.t - source_time).abs());
            near.map(|f| f.faces.clone()).unwrap_or_default()
        }
    }
}

/// How much of `[start, end)` the frames cover, as the share of `period`
/// steps that have an analysed frame within half a period.
pub fn coverage(
    frames: &[FaceFrame],
    start: Micros,
    end: Micros,
    period: Micros,
) -> (usize, usize) {
    let period = period.max(1);
    let mut total = 0;
    let mut have = 0;
    let mut t = start;
    // A step counts while a frame can start in it. The period is rounded
    // (33 333 µs at 30 fps), so the steps fall a third of a microsecond
    // short each; `t < end` then counted one step past the last frame (31
    // for a 1 s clip of 30 frames), and a short clip never reached
    // `DONE_SHARE` and was analysed again on every request.
    while t + period / 2 < end {
        total += 1;
        let i = frames.partition_point(|f| f.t < t - period / 2);
        if frames.get(i).is_some_and(|f| (f.t - t).abs() <= period / 2) {
            have += 1;
        }
        t += period;
    }
    (have, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32) -> Face {
        Face {
            score: 0.9,
            points: (0..POINTS).map(|i| [x, i as f32 / POINTS as f32]).collect(),
        }
    }

    #[test]
    fn a_track_round_trips_through_its_file_format() {
        let frames = vec![
            FaceFrame {
                t: 0,
                faces: vec![face(0.25), face(0.75)],
            },
            FaceFrame {
                t: 33_333,
                faces: vec![],
            },
            FaceFrame {
                t: 66_667,
                faces: vec![face(0.5)],
            },
        ];
        let back = decode(&encode(&frames)).unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[1].faces.len(), 0);
        assert_eq!(back[0].faces.len(), 2);
        for (a, b) in frames[0].faces[1]
            .points
            .iter()
            .zip(&back[0].faces[1].points)
        {
            assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4);
        }
        // A file cut short keeps its whole frames.
        let bytes = encode(&frames);
        assert_eq!(decode(&bytes[..bytes.len() - 10]).unwrap().len(), 2);
        assert!(decode(b"nonsense").is_err());
    }

    #[test]
    fn between_frames_the_points_are_mixed_and_far_from_any_there_is_none() {
        let frames = vec![
            FaceFrame {
                t: 0,
                faces: vec![face(0.2)],
            },
            FaceFrame {
                t: 100,
                faces: vec![face(0.4)],
            },
        ];
        let mid = faces_at(&frames, 25, 200);
        assert!((mid[0].points[0][0] - 0.25).abs() < 1e-6);
        assert!(faces_at(&frames, 1_000, 200).is_empty());
        assert!((faces_at(&frames, 150, 200)[0].points[0][0] - 0.4).abs() < 1e-6);
        // Too far apart to mix: the nearer one.
        assert!((faces_at(&frames, 30, 50)[0].points[0][0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn merging_replaces_a_frame_analysed_again_and_coverage_counts() {
        let dir = std::env::temp_dir().join(format!("chukcut-lmk-{}", std::process::id()));
        let path = dir.join("t.lmk");
        let _ = std::fs::remove_file(&path);
        let at = |t: Micros, x: f32| FaceFrame {
            t,
            faces: vec![face(x)],
        };
        merge_into(&path, vec![at(0, 0.1), at(100, 0.1)]).unwrap();
        merge_into(&path, vec![at(100, 0.9), at(200, 0.9)]).unwrap();
        let frames = read(&path).unwrap();
        assert_eq!(
            frames.iter().map(|f| f.t).collect::<Vec<_>>(),
            vec![0, 100, 200]
        );
        assert!(frames[1].faces[0].points[0][0] > 0.8);
        assert_eq!(coverage(&frames, 0, 300, 100), (3, 3));
        assert_eq!(coverage(&frames, 0, 600, 100), (3, 6));
    }

    #[test]
    fn a_whole_clip_at_30_fps_counts_its_own_frames() {
        // 30 frames of a 1 s clip at their exact times; the step is the
        // rounded 33 333 µs.
        let frames: Vec<FaceFrame> = (0..30)
            .map(|k| FaceFrame {
                t: (k as f64 * 1_000_000.0 / 30.0).round() as Micros,
                faces: Vec::new(),
            })
            .collect();
        assert_eq!(coverage(&frames, 0, 1_000_000, 33_333), (30, 30));
        assert_eq!(coverage(&frames, 0, 4_000_000, 33_333).1, 120);
    }
}
