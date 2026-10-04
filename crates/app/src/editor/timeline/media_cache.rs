//! Filmstrips and waveforms for the clips on the timeline.
//!
//! Both come from the engine (`media::thumbnail_strip`, `media::waveform`),
//! which caches its results on disk; this keeps the decoded, drawable form in
//! memory. Nothing here runs on the UI thread except the bookkeeping: the
//! editor asks [`MediaCache::wanted_strip`] / [`MediaCache::wanted_wave`]
//! while drawing, runs the returned job in the background, and hands the
//! result back with `finish_*`.
//!
//! A strip is `count` frames spread evenly over the whole material. The count
//! comes from the zoom, rounded up to a power of two, so zooming in and out
//! reuses a handful of strips instead of decoding a new one per step, and a
//! strip that is too coarse for the current zoom still draws while the finer
//! one loads.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chukcut_engine::modules::media::{self, Waveform};
use chukcut_engine::modules::project::Micros;
use gpui::RenderImage;

/// Height the thumbnails are decoded at, in pixels. About twice the height
/// they are drawn at, so they stay sharp on a 2x display.
pub(crate) const THUMB_PX: u32 = 84;

/// Most frames in one strip. A strip spans the whole material; past this the
/// tiles of a long clip at high zoom repeat, which reads fine and keeps one
/// strip's decode bounded.
const MAX_TILES: usize = 256;

/// Strips kept per material. The newest one and its neighbours cover zooming
/// back and forth; older ones are dropped from the GPU atlas.
const KEEP_PER_MATERIAL: usize = 3;

/// Background decodes running at once. Each strip decode already spreads over
/// rayon's pool, so more than this only queues work behind itself.
const MAX_JOBS: usize = 2;

/// What the timeline knows about a material's picture: a file to decode, and
/// whether it is a still (one frame, no strip).
#[derive(Clone)]
pub(crate) struct Picture {
    pub path: String,
    pub still: bool,
    /// Width over height as displayed, rotation applied.
    pub aspect: f32,
    pub duration: Micros,
}

/// Evenly spaced frames of one material, ready to draw.
pub(crate) struct Strip {
    pub tiles: Vec<Arc<RenderImage>>,
}

/// The tile count to use for a material `duration` long, drawn with tiles
/// `tile_w` pixels wide at `zoom` pixels per second.
pub(crate) fn strip_count(duration: Micros, zoom: f32, tile_w: f32) -> usize {
    let needed = (duration as f64 / 1_000_000.0 * zoom as f64 / tile_w.max(1.0) as f64).ceil();
    (needed.max(1.0) as usize)
        .next_power_of_two()
        .min(MAX_TILES)
}

#[derive(Default)]
pub(crate) struct MediaCache {
    strips: HashMap<(String, usize), Strip>,
    waves: HashMap<String, Arc<Waveform>>,
    /// Keys being worked on, or that failed: neither is asked for again.
    pending: HashSet<(String, usize)>,
    failed: HashSet<(String, usize)>,
    jobs: usize,
}

/// Waveforms share the key space with strips under this count.
const WAVE: usize = 0;

impl MediaCache {
    /// The best strip there is for `material` at `count` tiles: that one if
    /// it is ready, otherwise the closest count that is, otherwise — for a
    /// compound clip, whose key carries a digest of its contents — the strip
    /// of its contents before the last edit, while the new one renders.
    pub fn strip(&self, material: &str, count: usize) -> Option<&Strip> {
        if let Some(strip) = self.strips.get(&(material.to_string(), count)) {
            return Some(strip);
        }
        let closest = |same: &dyn Fn(&str) -> bool| {
            self.strips
                .iter()
                .filter(|((id, _), _)| same(id))
                .min_by_key(|((_, n), _)| n.abs_diff(count))
                .map(|(_, strip)| strip)
        };
        closest(&|id| id == material).or_else(|| {
            let family = family(material)?;
            closest(&|id| family_of(id) == Some(family))
        })
    }

    pub fn wave(&self, material: &str) -> Option<&Arc<Waveform>> {
        self.waves.get(material)
    }

    /// A strip to start decoding, if this one is missing and a worker is
    /// free. The caller runs [`load_strip`] in the background and passes the
    /// result to [`Self::finish_strip`].
    pub fn wanted_strip(&mut self, material: &str, count: usize) -> Option<(String, usize)> {
        let key = (material.to_string(), count);
        if self.strips.contains_key(&key)
            || self.pending.contains(&key)
            || self.failed.contains(&key)
            || self.jobs >= MAX_JOBS
        {
            return None;
        }
        self.pending.insert(key.clone());
        self.jobs += 1;
        Some(key)
    }

    /// Store a finished strip. Returns the images of strips it evicted, which
    /// the caller drops from the GPU atlas.
    pub fn finish_strip(
        &mut self,
        key: (String, usize),
        result: Result<Strip, String>,
    ) -> Vec<Arc<RenderImage>> {
        self.pending.remove(&key);
        self.jobs = self.jobs.saturating_sub(1);
        let strip = match result {
            Ok(strip) => strip,
            Err(error) => {
                tracing::warn!(material = %key.0, %error, "no filmstrip for this clip");
                self.failed.insert(key);
                return Vec::new();
            }
        };
        let (material, count) = key.clone();
        self.strips.insert(key, strip);
        let mut evicted = Vec::new();
        // A compound clip's strip of new contents retires the old contents'.
        if let Some(family) = family(&material) {
            let stale: Vec<(String, usize)> = self
                .strips
                .keys()
                .filter(|(id, _)| *id != material && family_of(id) == Some(family))
                .cloned()
                .collect();
            for key in stale {
                if let Some(strip) = self.strips.remove(&key) {
                    evicted.extend(strip.tiles);
                }
            }
        }

        // Keep the strips whose counts are closest to the one just made.
        let mut counts: Vec<usize> = self
            .strips
            .keys()
            .filter(|(id, _)| *id == material)
            .map(|(_, n)| *n)
            .collect();
        counts.sort_by_key(|n| n.abs_diff(count));
        for n in counts.into_iter().skip(KEEP_PER_MATERIAL) {
            if let Some(strip) = self.strips.remove(&(material.clone(), n)) {
                evicted.extend(strip.tiles);
            }
        }
        evicted
    }

    /// Whether `material`'s waveform should be loaded now.
    pub fn wanted_wave(&mut self, material: &str) -> bool {
        let key = (material.to_string(), WAVE);
        if self.waves.contains_key(material)
            || self.pending.contains(&key)
            || self.failed.contains(&key)
            || self.jobs >= MAX_JOBS
        {
            return false;
        }
        self.pending.insert(key);
        self.jobs += 1;
        true
    }

    pub fn finish_wave(&mut self, material: String, result: Result<Waveform, String>) {
        let key = (material.clone(), WAVE);
        self.pending.remove(&key);
        self.jobs = self.jobs.saturating_sub(1);
        match result {
            Ok(wave) => {
                self.waves.insert(material, Arc::new(wave));
            }
            Err(error) => {
                tracing::warn!(%material, %error, "no waveform for this clip");
                self.failed.insert(key);
            }
        }
    }
}

/// The compound clip a strip key names, for a compound clip's key
/// (`sequence::thumbs::strip_key`: `sequence#digest`); `None` for a file.
fn family(key: &str) -> Option<&str> {
    key.split_once('#').map(|(id, _)| id)
}

fn family_of(key: &str) -> Option<&str> {
    family(key)
}

/// Render a compound clip's strip: its sequence, `count` frames over its
/// whole length. Blocking: run it off the UI thread.
pub(crate) fn load_compound_strip(
    project: &chukcut_engine::modules::project::Project,
    sequence_id: &str,
    count: usize,
) -> Result<Strip, String> {
    let tiles = chukcut_engine::modules::sequence::thumbs::sequence_strip(
        project,
        sequence_id,
        count,
        THUMB_PX,
    )?;
    tiles
        .into_iter()
        .map(|tile| {
            let image = image::RgbaImage::from_raw(tile.width, tile.height, tile.bgra)
                .ok_or("a tile came back the wrong size")?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
        })
        .collect::<Result<Vec<_>, String>>()
        .map(|tiles| Strip { tiles })
}

/// Decode a strip. Blocking: run it off the UI thread.
pub(crate) fn load_strip(picture: &Picture, count: usize) -> Result<Strip, String> {
    let paths: Vec<PathBuf> = if picture.still {
        vec![PathBuf::from(&picture.path)]
    } else {
        media::thumbnail_strip(&picture.path, count, THUMB_PX)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(PathBuf::from)
            .collect()
    };
    let mut tiles = Vec::with_capacity(paths.len());
    for path in paths {
        // An animated sticker (a Lottie has no pixels to open) shows the
        // middle of its animation.
        let decoded = match chukcut_engine::modules::animated::still(&path, THUMB_PX * 2) {
            Some(still) => image::DynamicImage::ImageRgba8(still?),
            None => image::open(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        };
        // Stills arrive at full size; the atlas only needs a thumbnail.
        let decoded = if decoded.height() > THUMB_PX * 2 {
            decoded.thumbnail(u32::MAX, THUMB_PX * 2)
        } else {
            decoded
        };
        let mut rgba = decoded.to_rgba8();
        // gpui's images are BGRA.
        for pixel in rgba.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        tiles.push(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])));
    }
    if tiles.is_empty() {
        return Err("the strip came back empty".into());
    }
    Ok(Strip { tiles })
}

/// Columns per second of audio the timeline keeps in memory: one per 10 ms,
/// finer than any zoom short of single frames draws.
const WAVE_COLUMNS_PER_SECOND: f64 = 100.0;

/// Load a waveform. Blocking: run it off the UI thread.
pub(crate) fn load_wave(path: &str, duration: Micros) -> Result<Waveform, String> {
    let buckets =
        ((duration as f64 / 1_000_000.0 * WAVE_COLUMNS_PER_SECOND) as usize).clamp(16, 20_000);
    media::waveform(path, buckets).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_counts_come_in_powers_of_two_and_are_bounded() {
        // Ten seconds at 60 px/s with 70 px tiles needs nine tiles.
        assert_eq!(strip_count(10_000_000, 60.0, 70.0), 16);
        assert_eq!(strip_count(1, 1.0, 70.0), 1);
        assert_eq!(strip_count(3_600_000_000, 2000.0, 20.0), MAX_TILES);
    }

    #[test]
    fn a_compound_clips_new_strip_retires_the_old_contents_strip() {
        let mut cache = MediaCache::default();
        let key = cache.wanted_strip("seq#01", 8).unwrap();
        cache.finish_strip(key, Ok(Strip { tiles: Vec::new() }));
        // After an edit inside: the old strip stands in while the new loads.
        assert!(cache.strip("seq#02", 8).is_some());
        assert!(cache.strip("other#02", 8).is_none());
        let key = cache.wanted_strip("seq#02", 8).unwrap();
        cache.finish_strip(key, Ok(Strip { tiles: Vec::new() }));
        assert!(!cache.strips.contains_key(&("seq#01".to_string(), 8)));
        assert!(cache.strips.contains_key(&("seq#02".to_string(), 8)));
    }

    #[test]
    fn a_coarser_strip_stands_in_while_the_right_one_loads() {
        let mut cache = MediaCache::default();
        let key = cache.wanted_strip("m", 8).expect("a free worker");
        assert!(cache.wanted_strip("m", 8).is_none(), "asked for twice");
        cache.finish_strip(key, Ok(Strip { tiles: Vec::new() }));
        assert!(cache.strip("m", 32).is_some());
        assert!(cache.strip("other", 8).is_none());
    }
}
