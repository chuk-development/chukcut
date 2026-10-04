//! Animated pictures: Lottie, animated GIF and animated WebP — the animated
//! stickers.
//!
//! ## In the document
//!
//! An animated sticker is an ordinary image material (`ImageMaterial`) whose
//! file is a Lottie `.json`, a `.gif` or a `.webp` with more than one frame.
//! Nothing in the schema changed: the clip's source time runs from 0 like any
//! still's, and the picture at that time is the animation's frame at
//! [`playback::animation_time`] — looping by default, or played once and held
//! on its last frame, a per-clip setting stored as an `extras` block
//! ([`playback`]). A file that turns out to have one frame is a still.
//!
//! ## Rendering
//!
//! - **Lottie** ([`lottie`]): parsed by velato into its runtime model, drawn
//!   into a vello scene through our own `RenderSink`, and rasterised by vello
//!   on the one wgpu device (`gpu::render_context`), straight into a texture
//!   the compositor samples. Exact time, not the file's frame grid: a 60 fps
//!   animation on a 25 fps export is sampled where each export frame falls.
//!   velato does not draw text layers, embedded images or some effects;
//!   Noto's animated emoji use none of them.
//! - **GIF and WebP** ([`frames`]): decoded whole by the `image` crate and
//!   uploaded per frame. Not FFmpeg: the system FFmpeg (6.1) cannot decode an
//!   animated WebP at all ("image data not found"), and one decoder for both
//!   keeps their frame timing the same.
//!
//! Both are frame-exact in the export: the picture is a pure function of the
//! clip's time, so preview and export draw the same frame for the same
//! instant.
//!
//! ## Loading
//!
//! [`load`] parses or decodes a file once and keeps it, keyed by path, size
//! and modification time, so the renderer's per-frame lookups cost a map
//! lookup and a `stat`.

pub mod commands;
pub mod frames;
pub mod lottie;
pub mod playback;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use parking_lot::Mutex;
use serde::Serialize;

use crate::modules::project::document::{MaterialKind, MaterialPool, Micros, Segment};

/// Which kind of animated file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Lottie,
    Gif,
    WebP,
}

impl Format {
    /// What `path` could be, by its extension. A GIF or WebP may still turn
    /// out to be a single frame; [`load`] decides.
    pub fn of(path: &Path) -> Option<Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "json" => Some(Format::Lottie),
            "gif" => Some(Format::Gif),
            "webp" => Some(Format::WebP),
            _ => None,
        }
    }
}

/// An animation, parsed or decoded.
pub enum Animation {
    Lottie(lottie::LottieAnimation),
    Frames(frames::FrameAnimation),
}

impl Animation {
    pub fn format(&self) -> Format {
        match self {
            Animation::Lottie(_) => Format::Lottie,
            Animation::Frames(f) => f.format,
        }
    }

    /// Its natural size in pixels.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Animation::Lottie(l) => (l.width, l.height),
            Animation::Frames(f) => f.size(),
        }
    }

    /// How long one pass of it lasts.
    pub fn duration(&self) -> Micros {
        match self {
            Animation::Lottie(l) => l.duration(),
            Animation::Frames(f) => f.duration,
        }
    }

    /// Frames per second: the file's own for Lottie, the mean for a GIF or
    /// WebP whose frames may each last differently.
    pub fn fps(&self) -> f64 {
        match self {
            Animation::Lottie(l) => l.frame_rate,
            Animation::Frames(f) => {
                f.frames.len() as f64 * 1_000_000.0 / (f.duration.max(1)) as f64
            }
        }
    }

    pub fn info(&self) -> AnimationInfo {
        let (width, height) = self.size();
        AnimationInfo {
            format: self.format(),
            width,
            height,
            duration: self.duration(),
            fps: self.fps(),
        }
    }
}

/// What a UI, the CLI and the import need to know about an animated file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AnimationInfo {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    pub duration: Micros,
    pub fps: f64,
}

type Stamp = (u64, Option<SystemTime>);

/// A loaded file, or the reason it is not animated, by path.
type Loaded = Result<Arc<Animation>, String>;

fn cache() -> &'static Mutex<HashMap<PathBuf, (Stamp, Loaded)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Stamp, Loaded)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many decoded files stay loaded. Each is at most a few dozen
/// megabytes of frames; a project uses a handful.
const KEEP: usize = 32;

/// The animation in `path`. `Err` for a file that is not one — a still GIF
/// or WebP, a JSON that is not Lottie, a missing file — with the reason.
///
/// Cached by path, size and modification time; the error is cached too, so
/// a still `.webp` drawn on every frame is decoded once.
pub fn load(path: &Path) -> Result<Arc<Animation>, String> {
    let format = Format::of(path).ok_or("not an animated file type")?;
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let stamp = (meta.len(), meta.modified().ok());
    if let Some((cached_stamp, loaded)) = cache().lock().get(path) {
        if *cached_stamp == stamp {
            return loaded.clone();
        }
    }
    // Parsed without the lock: a large GIF takes a while, and two callers
    // loading the same file at once only waste one decode.
    let loaded: Loaded = match format {
        Format::Lottie => lottie::LottieAnimation::open(path).map(Animation::Lottie),
        Format::Gif | Format::WebP => frames::FrameAnimation::open(path, format)
            .and_then(|f| {
                if f.frames.len() > 1 {
                    Ok(f)
                } else {
                    Err("a single frame: a still picture".into())
                }
            })
            .map(Animation::Frames),
    }
    .map(Arc::new);
    let mut cache = cache().lock();
    if cache.len() >= KEEP {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (stamp, loaded.clone()));
    loaded
}

/// [`load`] for a path that is an animation, `None` for anything else.
pub fn animation(path: &Path) -> Option<Arc<Animation>> {
    Format::of(path)?;
    load(path).ok()
}

/// One representative picture of an animated file — the middle of the
/// animation, where a Lottie's first frame is often still empty — at most
/// `max_side` pixels on its longer side, for a thumbnail. `None` for a file
/// that is not an animation.
pub fn still(path: &Path, max_side: u32) -> Option<Result<image::RgbaImage, String>> {
    let animation = animation(path)?;
    Some(match animation.as_ref() {
        Animation::Lottie(lottie) => (|| {
            let ctx = crate::modules::gpu::render_context().ok_or("no GPU for the Lottie")?;
            let size = lottie.fitted((max_side.max(1), max_side.max(1)));
            let rgba = lottie.render_rgba(&ctx, lottie.duration() / 2, size)?;
            image::RgbaImage::from_raw(size.0, size.1, rgba)
                .ok_or_else(|| "a Lottie frame had the wrong size".to_string())
        })(),
        Animation::Frames(frames) => {
            let frame = &frames.frames[frames.frames.len() / 2];
            let (w, h) = frame.dimensions();
            let scale = (max_side.max(1) as f64 / w.max(h) as f64).min(1.0);
            Ok(image::imageops::resize(
                frame,
                ((w as f64 * scale).round() as u32).max(1),
                ((h as f64 * scale).round() as u32).max(1),
                image::imageops::FilterType::Triangle,
            ))
        }
    })
}

/// The time inside the animation an image clip shows at `source_time`:
/// unchanged for a still, looped or held for an animation by the clip's
/// playback setting.
pub fn clip_time(
    materials: &MaterialPool,
    segment: &Segment,
    kind: MaterialKind,
    source_time: Micros,
) -> Micros {
    if kind != MaterialKind::Image {
        return source_time;
    }
    let Some(image) = materials.image(&segment.material_id) else {
        return source_time;
    };
    let Some(animation) = animation(Path::new(&image.path)) else {
        return source_time;
    };
    playback::animation_time(
        playback::playback_of(materials, segment),
        source_time,
        animation.duration(),
    )
}

/// A directory for a test's generated files, inside the crate's ignored
/// fixture directory rather than the system's temporary one.
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/generated/animated")
        .join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("fixture directory");
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_told_by_extension() {
        assert_eq!(Format::of(Path::new("a/b.JSON")), Some(Format::Lottie));
        assert_eq!(Format::of(Path::new("x.gif")), Some(Format::Gif));
        assert_eq!(Format::of(Path::new("x.webp")), Some(Format::WebP));
        assert_eq!(Format::of(Path::new("x.png")), None);
    }

    #[test]
    fn a_missing_file_is_not_an_animation() {
        assert!(load(Path::new("/nowhere/at/all.json")).is_err());
        assert!(animation(Path::new("/nowhere/at/all.gif")).is_none());
    }
}
