//! The active timeline reduced to what interchange formats can say: tracks
//! of clips cut from files, their speed, the dissolves at their cuts, and
//! the markers.

use std::path::Path;

use super::Report;
use crate::modules::project::{MaterialKind, Micros, Project, TimeRange, TrackKind};

/// The timeline's frame rate and the frame arithmetic every format needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rate {
    pub fps: f64,
}

/// The broadcast rates whose frames are not a whole number of milliseconds,
/// as `(fps, numerator, denominator)` of the frame duration.
const NTSC: [(f64, i64, i64); 4] = [
    (24_000.0 / 1_001.0, 1_001, 24_000),
    (30_000.0 / 1_001.0, 1_001, 30_000),
    (48_000.0 / 1_001.0, 1_001, 48_000),
    (60_000.0 / 1_001.0, 1_001, 60_000),
];

impl Rate {
    pub fn new(fps: f64) -> Self {
        Self {
            fps: if fps.is_finite() && fps > 0.0 {
                fps
            } else {
                30.0
            },
        }
    }

    /// The frame at or nearest to `t`.
    pub fn frames(&self, t: Micros) -> i64 {
        (t as f64 * self.fps / 1_000_000.0).round() as i64
    }

    /// `t` in frames, not rounded: OTIO keeps sub-frame times as fractions.
    pub fn frames_exact(&self, t: Micros) -> f64 {
        t as f64 * self.fps / 1_000_000.0
    }

    /// One frame's duration as a fraction of a second, `(num, den)`:
    /// `1001/30000` for 29.97, `1/25` for 25. FCPXML writes every time this
    /// way.
    pub fn frame_duration(&self) -> (i64, i64) {
        for (fps, num, den) in NTSC {
            if (self.fps - fps).abs() < 0.005 {
                return (num, den);
            }
        }
        let whole = self.fps.round();
        if (self.fps - whole).abs() < 0.001 {
            return (1, whole as i64);
        }
        // An odd rate: milliframes, which every reader accepts.
        (1_000, (self.fps * 1_000.0).round() as i64)
    }

    /// The whole number timecode counts frames in: 30 for 29.97.
    pub fn timecode_base(&self) -> i64 {
        (self.fps.round() as i64).max(1)
    }

    /// 29.97 and 59.94 are written in drop-frame timecode, so the clock on
    /// the wall and the timecode agree.
    pub fn drop_frame(&self) -> bool {
        let (num, den) = self.frame_duration();
        num == 1_001 && (den == 30_000 || den == 60_000)
    }

    /// `frame` as SMPTE timecode, `;` before the frames in drop-frame.
    pub fn timecode(&self, frame: i64) -> String {
        let base = self.timecode_base();
        let mut f = frame.max(0);
        if self.drop_frame() {
            // Two frame numbers (four at 59.94) are skipped at the start of
            // every minute except every tenth.
            let drop = if base == 60 { 4 } else { 2 };
            let per_10min = base * 600 - drop * 9;
            let per_min = base * 60 - drop;
            let tens = f / per_10min;
            let rem = f % per_10min;
            f += drop * 9 * tens;
            if rem > drop {
                f += drop * ((rem - drop) / per_min);
            }
        }
        let (h, m, s, fr) = (
            f / (base * 3_600),
            (f / (base * 60)) % 60,
            (f / base) % 60,
            f % base,
        );
        let sep = if self.drop_frame() { ';' } else { ':' };
        format!("{h:02}:{m:02}:{s:02}{sep}{fr:02}")
    }
}

/// What a clip shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    Video,
    Image,
    Audio,
}

/// One clip, cut from a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// The file's name without its folder and extension.
    pub name: String,
    pub path: String,
    pub kind: ClipKind,
    /// Where it sits on the timeline.
    pub record: TimeRange,
    /// Where in the file it starts.
    pub source_start: Micros,
    /// Constant speed; 1 is normal, 2 twice as fast.
    pub speed: f32,
    /// The whole file's length, for the formats that describe the media.
    /// `None` for a still.
    pub media_duration: Option<Micros>,
    /// Whether the file has sound as well as picture.
    pub has_audio: bool,
    /// A transition centred on this clip's first frame, of this length:
    /// chukcut's transitions sit on the incoming clip.
    pub transition_in: Option<Micros>,
    /// The track's lane is muted or hidden.
    pub enabled: bool,
}

impl Clip {
    /// How much of the file the clip plays.
    pub fn source_duration(&self) -> Micros {
        (self.record.duration as f64 * self.speed as f64).round() as Micros
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub kind: Kind,
    pub name: String,
    /// In time order, never overlapping (the document's own invariant).
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub time: Micros,
    pub label: String,
    /// Upper case English, the colour names editors share: `BLUE`, `RED`.
    pub color: &'static str,
}

/// The timeline, ready for a writer. Picture tracks bottom to top (the
/// first is chukcut's main lane), then sound tracks top to bottom.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub name: String,
    pub rate: Rate,
    pub width: u32,
    pub height: u32,
    pub duration: Micros,
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
}

impl Timeline {
    pub fn video_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == Kind::Video)
    }

    pub fn audio_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(|t| t.kind == Kind::Audio)
    }
}

fn stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

fn color_name(color: crate::modules::project::MarkerColor) -> &'static str {
    use crate::modules::project::MarkerColor::*;
    match color {
        Blue => "BLUE",
        Green => "GREEN",
        Yellow => "YELLOW",
        Orange => "ORANGE",
        Red => "RED",
        Purple => "PURPLE",
    }
}

/// Reduce the project's active timeline, noting in `report` what is left
/// behind.
pub fn collect(project: &Project, report: &mut Report) -> Timeline {
    let pool = &project.materials;
    let mut tracks = Vec::new();
    let mut audio = Vec::new();
    let (mut v, mut a) = (0, 0);
    for track in &project.tracks {
        let kind = match track.kind {
            TrackKind::Video => Kind::Video,
            TrackKind::Audio => Kind::Audio,
            TrackKind::Text => {
                if !track.segments.is_empty() {
                    report.warn("titles and captions are not written: export the captions as .srt for the other editor");
                }
                continue;
            }
            TrackKind::Sticker | TrackKind::Effect => {
                if !track.segments.is_empty() {
                    report.warn("stickers and effect clips are not written");
                }
                continue;
            }
        };
        let enabled = !(track.muted || track.hidden);
        let mut clips = Vec::new();
        for segment in &track.segments {
            let (path, clip_kind, media_duration, has_audio) = match pool
                .kind_of(&segment.material_id)
            {
                Some(MaterialKind::Video) => {
                    let Some(m) = pool.video(&segment.material_id) else {
                        continue;
                    };
                    (
                        m.path.clone(),
                        ClipKind::Video,
                        Some(m.duration),
                        m.has_audio,
                    )
                }
                Some(MaterialKind::Image) => {
                    let Some(m) = pool.image(&segment.material_id) else {
                        continue;
                    };
                    (m.path.clone(), ClipKind::Image, None, false)
                }
                Some(MaterialKind::Audio) => {
                    let Some(m) = pool.audio(&segment.material_id) else {
                        continue;
                    };
                    (m.path.clone(), ClipKind::Audio, Some(m.duration), true)
                }
                Some(MaterialKind::Sequence) => {
                    report.warn("compound clips are not written: open the clip menu and choose Put clips back first");
                    continue;
                }
                _ => {
                    if pool.is_effect_clip(segment) {
                        report.warn("stickers and effect clips are not written");
                    }
                    continue;
                }
            };
            if pool.speed_curve_of(segment).is_some() {
                report.warn("speed curves are written as the clip's average speed");
            }
            if !segment.keyframes.is_empty() || pool.color_adjust_of(segment).is_some() {
                report.warn(
                    "looks, keyframes and effects stay in chukcut; the other editor gets the cut",
                );
            }
            let transition_in = pool
                .transition_of(segment)
                .map(|t| t.duration)
                .filter(|d| *d > 0);
            // The speed the source range implies, which is the curve's mean
            // for a curved clip and the constant speed for any other.
            let speed = if segment.target_range.duration > 0 {
                (segment.source_range.duration as f64 / segment.target_range.duration as f64) as f32
            } else {
                1.0
            };
            clips.push(Clip {
                name: stem(&path),
                path,
                kind: clip_kind,
                record: segment.target_range,
                source_start: segment.source_range.start,
                speed: if (speed - 1.0).abs() < 1e-4 {
                    1.0
                } else {
                    speed
                },
                media_duration,
                has_audio,
                transition_in,
                enabled,
            });
        }
        let name = if track.name.is_empty() {
            match kind {
                Kind::Video => {
                    v += 1;
                    format!("V{v}")
                }
                Kind::Audio => {
                    a += 1;
                    format!("A{a}")
                }
            }
        } else {
            track.name.clone()
        };
        let out = Track { kind, name, clips };
        match kind {
            Kind::Video => tracks.push(out),
            Kind::Audio => audio.push(out),
        }
    }
    // Sound tracks in the document are top to bottom below the picture,
    // which is the order every format numbers them in already.
    tracks.extend(audio);
    let mut markers: Vec<Marker> = project
        .markers
        .iter()
        .map(|m| Marker {
            time: m.time,
            label: m.label.clone(),
            color: color_name(m.color),
        })
        .collect();
    markers.sort_by_key(|m| m.time);
    Timeline {
        name: if project.name.is_empty() {
            "chukcut".to_string()
        } else {
            project.name.clone()
        },
        rate: Rate::new(project.fps),
        width: project.canvas.width,
        height: project.canvas.height,
        duration: project.duration(),
        tracks,
        markers,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::modules::project::{
        AudioMaterial, CanvasConfig, Marker as DocMarker, MarkerColor, Segment, Track as DocTrack,
        TransitionKind, TransitionMaterial, VideoMaterial,
    };

    pub(crate) fn segment(material: &str, start: Micros, dur: Micros, src: Micros) -> Segment {
        Segment {
            id: crate::modules::project::new_id(),
            material_id: material.into(),
            target_range: TimeRange::new(start, dur),
            source_range: TimeRange::new(src, dur),
            render_index: 0,
            speed: 1.0,
            volume: 1.0,
            transform: Default::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// Two shots on the main lane with a 1 s dissolve between them, an
    /// overlay at double speed on V2, the music on A1, a marker.
    pub(crate) fn project() -> Project {
        let mut p = Project::new(
            "Trailer",
            CanvasConfig {
                width: 1920,
                height: 1080,
                ..Default::default()
            },
            25.0,
        );
        p.tracks.clear();
        for (id, path) in [("a", "/m/shot a.mp4"), ("b", "/m/b.mov")] {
            p.materials.videos.push(VideoMaterial {
                id: id.into(),
                path: path.into(),
                width: 1920,
                height: 1080,
                duration: 60_000_000,
                fps: 25.0,
                has_audio: true,
                rotation: 0,
            });
        }
        p.materials.audios.push(AudioMaterial {
            id: "m".into(),
            path: "/m/music.wav".into(),
            duration: 120_000_000,
            sample_rate: 48_000,
            channels: 2,
        });
        let dissolve = TransitionMaterial::new(TransitionKind::Dissolve, 1_000_000);
        let mut main = DocTrack::new(TrackKind::Video, "");
        main.segments.push(segment("a", 0, 4_000_000, 10_000_000));
        let mut second = segment("b", 4_000_000, 4_000_000, 2_000_000);
        second.extras.push(dissolve.id.clone());
        main.segments.push(second);
        p.materials.transitions.push(dissolve);
        let mut overlay = DocTrack::new(TrackKind::Video, "");
        let mut fast = segment("b", 2_000_000, 2_000_000, 0);
        fast.source_range.duration = 4_000_000;
        fast.speed = 2.0;
        overlay.segments.push(fast);
        let mut music = DocTrack::new(TrackKind::Audio, "");
        music.segments.push(segment("m", 0, 8_000_000, 0));
        p.tracks = vec![main, overlay, music];
        let mut marker = DocMarker::new(6_000_000);
        marker.label = "drop".into();
        marker.color = MarkerColor::Red;
        p.markers.push(marker);
        p
    }

    #[test]
    fn the_flattened_timeline_keeps_order_speed_and_transitions() {
        let mut report = Report::default();
        let t = collect(&project(), &mut report);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let names: Vec<&str> = t.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["V1", "V2", "A1"]);
        assert_eq!(t.tracks[0].clips[0].name, "shot a");
        assert_eq!(t.tracks[0].clips[1].transition_in, Some(1_000_000));
        assert_eq!(t.tracks[1].clips[0].speed, 2.0);
        assert_eq!(t.tracks[1].clips[0].source_duration(), 4_000_000);
        assert_eq!(t.markers[0].color, "RED");
        assert_eq!(t.duration, 8_000_000);
    }

    #[test]
    fn timecode_counts_frames_and_drops_them_at_29_97() {
        let pal = Rate::new(25.0);
        assert_eq!(pal.timecode(25 * 3_600 + 26), "01:00:01:01");
        assert_eq!(pal.frame_duration(), (1, 25));
        let ntsc = Rate::new(30_000.0 / 1_001.0);
        assert!(ntsc.drop_frame());
        assert_eq!(ntsc.frame_duration(), (1_001, 30_000));
        // Frame 1800 is the first of minute one, which skips ;00 and ;01.
        assert_eq!(ntsc.timecode(1_799), "00:00:59;29");
        assert_eq!(ntsc.timecode(1_800), "00:01:00;02");
        // Every tenth minute keeps its first two numbers.
        assert_eq!(ntsc.timecode(17_982), "00:10:00;00");
        // An hour of 29.97 is 107 892 frames and reads 01:00:00;00.
        assert_eq!(ntsc.timecode(107_892), "01:00:00;00");
        assert!(!Rate::new(24_000.0 / 1_001.0).drop_frame());
    }
}
