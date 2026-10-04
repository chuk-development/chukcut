//! "Optical flow (AI)": frames between source frames, made by RIFE in the
//! ML worker and baked into the cache, for slowed clips and speed curves.
//!
//! The setting is the third frame-blending mode (`speed::blend`,
//! `FrameBlend::Flow`). Everything here is cache, keyed like the mattes
//! (`matting::cache`), never document:
//!
//! ```text
//! <cache>/chukcut/flow/<media digest>-<model>-<version>-<provider>-<long side>/
//!     00000042-16.jpg     between source frames 42 and 43, 16/64 of the way
//!     00000042-32.jpg
//!     …
//! ```
//!
//! - **What a frame is called.** A blended instant falls between source
//!   frames `index` and `index + 1` at some `weight` (`blend::blend_sample`).
//!   The weight is rounded to 64ths ([`STEPS`]): a constant 0.25x gives 16,
//!   32 and 48 exactly, a 24 fps clip on a 30 fps timeline gives its fifths
//!   within 1/128 of a frame, and a speed curve's arbitrary phases land on
//!   a grid that a re-render of the same instant finds again. A weight that
//!   rounds to 0 or 64 is the source frame itself.
//! - **The provider and the size are part of the key**, as for mattes: CUDA
//!   and CPU frames differ in the last bits, and a clip draws from one
//!   directory, the one with the most frames ([`best`]).
//! - **JPEG at quality 95, 4:4:4**, through libjpeg-turbo: a 1080p frame is
//!   0.5–1 MB on disk and decodes in a few milliseconds on the render thread,
//!   where a PNG of it was 3 MB and 25 ms. The new frame is drawn between two
//!   lossy decoded ones, so the loss is below what the source already has.
//! - **Size.** Made at the source's display size, at most [`MAX_LONG_SIDE`]
//!   on the long side; a 4K clip's in-between frames are made at 1080p and
//!   drawn scaled, which RIFE's own authors recommend for 4K anyway.
//!
//! Baking ([`bake`]) decodes each pair once and asks for all of its phases
//! in one request. Frames count towards the cache limit and are deleted with
//! the cache; the open project's are kept by a trim (`workspace::trim`).

pub mod bake;
pub mod jobs;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::modules::project::document::{Micros, Project, Segment, SAMPLE_SLACK};
use crate::modules::project::MaterialPool;
use crate::modules::proxy::cache::SourceKey;

use super::blend::{self, BlendSample, FrameBlend};

/// Phases are rounded to this many steps between two source frames.
pub const STEPS: u32 = 64;

/// The long side, in pixels, frames are made at most at.
pub const MAX_LONG_SIDE: u32 = 1920;

/// JPEG quality of a stored frame.
const QUALITY: i32 = 95;

/// One frame to make: between source frames `index` and `index + 1`,
/// `step / STEPS` of the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FlowSample {
    pub index: i64,
    pub step: u32,
}

impl FlowSample {
    /// The sample a blend falls on; `None` when its weight rounds to one of
    /// the source frames themselves.
    pub fn of(blend: &BlendSample) -> Option<FlowSample> {
        let step = (blend.weight.clamp(0.0, 1.0) * STEPS as f32).round() as u32;
        (step > 0 && step < STEPS).then_some(FlowSample {
            index: blend.index,
            step,
        })
    }

    /// The phase the model is asked for.
    pub fn phase(self) -> f32 {
        self.step as f32 / STEPS as f32
    }

    fn file_name(self) -> String {
        format!("{:08}-{:02}.jpg", self.index.max(0), self.step)
    }

    fn parse(name: &str) -> Option<FlowSample> {
        let stem = name.strip_suffix(".jpg")?;
        let (index, step) = stem.split_once('-')?;
        Some(FlowSample {
            index: index.parse().ok()?,
            step: step.parse().ok()?,
        })
    }
}

/// The root of all optical-flow frames.
pub fn root() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("flow")
}

/// The part of a directory name that says what made the frames, without
/// the provider and the size: `<digest>-<model>-<version>`. Reads the media
/// file's head and tail; callers keep the answer.
pub fn key_for(media: &Path) -> Result<String, String> {
    let key = SourceKey::of(media).map_err(|e| format!("cannot read {}: {e}", media.display()))?;
    let model = crate::modules::ml::interpolate::MODEL;
    let version = crate::modules::ml::interpolate::model_version();
    Ok(format!("{}-{model}-{version}", key.digest()))
}

/// The prefix of every flow directory of `media`: what a trim protects for
/// the open project.
pub fn media_prefix(media: &Path) -> Result<String, String> {
    crate::modules::matting::cache::media_prefix(media)
}

/// The long side frames of a `width × height` source are made at.
pub fn long_side(width: u32, height: u32) -> u32 {
    width.max(height).clamp(16, MAX_LONG_SIDE)
}

/// The directory `provider` bakes `key`'s frames into, at `long` pixels.
pub fn dir_in(key: &str, provider: &str, long: u32) -> PathBuf {
    root().join(format!(
        "{key}-{}-{long}",
        crate::modules::matting::cache::provider_token(provider)
    ))
}

/// Every directory holding frames of `key`, whatever provider or size.
pub fn dirs_of(key: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root()) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(key)) else {
                return false;
            };
            // `-<provider>-<long side>` and nothing else, so one version's
            // directory is never read as another's.
            matches!(rest.split('-').collect::<Vec<_>>().as_slice(),
                ["", provider, size] if !provider.is_empty() && size.parse::<u32>().is_ok())
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// The frames present in `dir`. Empty when it does not exist.
pub fn list(dir: &Path) -> BTreeSet<FlowSample> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|e| FlowSample::parse(e.file_name().to_str()?))
        .collect()
}

/// The directory to draw `key` from and its frames: the one with the most,
/// so a clip shows one provider's frames, never a mix.
pub fn best(key: &str) -> Option<(PathBuf, BTreeSet<FlowSample>)> {
    dirs_of(key)
        .into_iter()
        .map(|dir| {
            let frames = list(&dir);
            (dir, frames)
        })
        .filter(|(_, frames)| !frames.is_empty())
        .max_by_key(|(_, frames)| frames.len())
}

/// The file of `sample` in `dir`.
pub fn frame_path(dir: &Path, sample: FlowSample) -> PathBuf {
    dir.join(sample.file_name())
}

/// Write one frame (RGBA8, `width × height`) atomically.
pub fn write(
    dir: &Path,
    sample: FlowSample,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let image = turbojpeg::Image {
        pixels: rgba,
        width: width as usize,
        pitch: width as usize * 4,
        height: height as usize,
        format: turbojpeg::PixelFormat::RGBA,
    };
    let mut compressor = turbojpeg::Compressor::new().map_err(|e| format!("libjpeg-turbo: {e}"))?;
    compressor
        .set_quality(QUALITY)
        .and_then(|_| compressor.set_subsamp(turbojpeg::Subsamp::None))
        .map_err(|e| format!("libjpeg-turbo: {e}"))?;
    let bytes = compressor
        .compress_to_vec(image)
        .map_err(|e| format!("libjpeg-turbo: {e}"))?;
    let path = frame_path(dir, sample);
    let part = path.with_extension("jpg.part");
    std::fs::write(&part, bytes).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    std::fs::rename(&part, &path).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Read one frame: width, height, RGBA8.
pub fn read(dir: &Path, sample: FlowSample) -> Result<(u32, u32, Vec<u8>), String> {
    let path = frame_path(dir, sample);
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let image: turbojpeg::Image<Vec<u8>> =
        turbojpeg::decompress(&bytes, turbojpeg::PixelFormat::RGBA)
            .map_err(|e| format!("cannot decode {}: {e}", path.display()))?;
    Ok((image.width as u32, image.height as u32, image.pixels))
}

/// Whether `segment` is a video clip with optical flow on.
pub fn is_on(materials: &MaterialPool, segment: &Segment) -> bool {
    !segment.extras.is_empty()
        && blend::frame_blend_of(materials, segment) == FrameBlend::Flow
        && materials.video(&segment.material_id).is_some()
}

/// The frames `segment` shows between its source frames at the timeline
/// instants `times` (those outside the clip are skipped).
pub fn samples_at(
    materials: &MaterialPool,
    segment: &Segment,
    times: impl IntoIterator<Item = Micros>,
) -> BTreeSet<FlowSample> {
    let Some(video) = materials.video(&segment.material_id) else {
        return BTreeSet::new();
    };
    let map = materials.time_map(segment);
    times
        .into_iter()
        .filter_map(|t| map.source_time_at(t))
        .filter_map(|st| blend::blend_sample(video.fps, video.duration, st))
        .filter_map(|b| FlowSample::of(&b))
        .collect()
}

/// The frame instants the preview plays `segment` at: the project's frame
/// grid over the clip, each with the slack every renderer adds.
pub fn preview_times(project: &Project, segment: &Segment) -> Vec<Micros> {
    use crate::modules::preview::clock::{frame_at, frame_time};
    let fps = project.fps;
    let range = segment.target_range;
    let first = frame_at(range.start, fps);
    let last = frame_at((range.end() - 1).max(range.start), fps);
    (first..=last)
        .map(|n| frame_time(n, fps) + SAMPLE_SLACK)
        .filter(|&t| range.contains(t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weight_rounds_to_a_sixty_fourth_and_the_ends_are_source_frames() {
        let at = |weight: f32| BlendSample {
            index: 7,
            first: 0,
            second: 0,
            weight,
        };
        assert_eq!(
            FlowSample::of(&at(0.25)),
            Some(FlowSample { index: 7, step: 16 })
        );
        assert_eq!(FlowSample::of(&at(0.4)).unwrap().step, 26);
        assert_eq!(FlowSample::of(&at(0.005)), None);
        assert_eq!(FlowSample::of(&at(0.995)), None);
        let sample = FlowSample { index: 42, step: 9 };
        assert_eq!(FlowSample::parse(&sample.file_name()), Some(sample));
        assert_eq!(FlowSample::parse("00000042-09.jpg.part"), None);
        assert!((sample.phase() - 9.0 / 64.0).abs() < 1e-6);
    }

    #[test]
    fn a_quarter_speed_clip_needs_three_frames_per_source_frame() {
        use crate::modules::project::document::{
            CanvasConfig, TimeRange, Track, TrackKind, Transform, VideoMaterial,
        };
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.videos.push(VideoMaterial {
            id: "v".into(),
            path: "/nowhere.mp4".into(),
            width: 64,
            height: 64,
            duration: 2_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(Segment {
            id: "s".into(),
            material_id: "v".into(),
            target_range: TimeRange::new(0, 400_000),
            source_range: TimeRange::new(0, 100_000),
            render_index: 0,
            speed: 0.25,
            volume: 1.0,
            transform: Transform::default(),
            crop: None,
            extras: Vec::new(),
            keyframes: Vec::new(),
        });
        p.tracks.push(track);
        let segment = p.tracks[0].segments[0].clone();
        let times = preview_times(&p, &segment);
        assert_eq!(times.len(), 12);
        let samples = samples_at(&p.materials, &segment, times);
        let steps: Vec<(i64, u32)> = samples.iter().map(|s| (s.index, s.step)).collect();
        // Three source frames, each followed by its quarter, half and three
        // quarters; the last frame's are missing only when no frame follows.
        assert_eq!(
            steps,
            vec![
                (0, 16),
                (0, 32),
                (0, 48),
                (1, 16),
                (1, 32),
                (1, 48),
                (2, 16),
                (2, 32),
                (2, 48)
            ]
        );
    }

    #[test]
    fn a_frame_survives_the_round_trip_and_is_listed() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/flow-cache-round-trip");
        let _ = std::fs::remove_dir_all(&dir);
        let (w, h) = (32u32, 16u32);
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % w * 8) as u8, (i / w * 16) as u8, 128, 255])
            .collect();
        let sample = FlowSample { index: 3, step: 32 };
        write(&dir, sample, w, h, &rgba).unwrap();
        assert_eq!(list(&dir).into_iter().collect::<Vec<_>>(), vec![sample]);
        let (rw, rh, back) = read(&dir, sample).unwrap();
        assert_eq!((rw, rh), (w, h));
        let worst = rgba
            .iter()
            .zip(&back)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(worst <= 6, "quality 95 4:4:4 is close: worst {worst}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
