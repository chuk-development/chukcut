//! The animated-sticker commands: what the app, the CLI and the MCP server
//! call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::playback::{self, Playback};
use super::Animation;
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

/// What an animated file is: format, size, length. An error for a file that
/// is not animated.
pub fn animated_info(path: &Path) -> Result<super::AnimationInfo, String> {
    super::load(path).map(|a| a.info())
}

/// A clip's playback (loop or once).
pub fn animated_playback(state: &Arc<AppState>, segment_id: String) -> Result<Playback, String> {
    state.with_project(|project| {
        let (_, segment) = project
            .segment(&segment_id)
            .ok_or_else(|| format!("unknown segment {segment_id}"))?;
        Ok(playback::playback_of(&project.materials, segment))
    })?
}

/// Make an animated sticker loop or play once and hold its last frame. One
/// undo step.
pub fn animated_set_playback(
    state: &Arc<AppState>,
    segment_id: String,
    playback: Playback,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = playback::set_playback_command(project, &segment_id, playback)?;
        match entry {
            Some((id, value)) => {
                project.materials.extras.insert(id.clone(), value);
                if let Err(error) = state.history.write().apply(project, command) {
                    project.materials.extras.remove(&id);
                    return Err(error);
                }
            }
            None => state.history.write().apply(project, command)?,
        }
    }
    crate::modules::voice::commands::respond(state)
}

/// How many frames per second a preview GIF has: smooth enough to read as
/// motion in a 96-pixel tile, few enough to stay small.
const PREVIEW_FPS: f64 = 15.0;

/// Write a small looping GIF of the animation in `path` to `out`, `side`
/// pixels on its longer side, for a tile that shows the sticker moving. GPUI
/// plays GIFs in an `img`, so the tile needs nothing else. Kept when it is
/// already there and newer than the source.
pub fn animated_preview(path: &Path, side: u32, out: &Path) -> Result<PathBuf, String> {
    let fresh = |a: &Path, b: &Path| -> Option<bool> {
        Some(
            std::fs::metadata(a).ok()?.modified().ok()?
                >= std::fs::metadata(b).ok()?.modified().ok()?,
        )
    };
    if fresh(out, path) == Some(true) {
        return Ok(out.to_path_buf());
    }
    let animation = super::load(path)?;
    let (w, h) = animation.size();
    let scale = side.max(1) as f64 / w.max(h).max(1) as f64;
    let size = (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    );
    let duration = animation.duration().max(1);
    let count = ((duration as f64 / 1_000_000.0 * PREVIEW_FPS).round() as usize).clamp(1, 120);
    let step = duration / count as i64;
    let ctx = match animation.as_ref() {
        Animation::Lottie(_) => {
            Some(crate::modules::gpu::render_context().ok_or("no GPU for the Lottie preview")?)
        }
        Animation::Frames(_) => None,
    };
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let at = i as i64 * step;
        let rgba = match animation.as_ref() {
            Animation::Lottie(lottie) => {
                lottie.render_rgba(ctx.as_ref().expect("built above"), at, size)?
            }
            Animation::Frames(f) => image::imageops::resize(
                &f.frames[f.index_at(at)],
                size.0,
                size.1,
                image::imageops::FilterType::Triangle,
            )
            .into_raw(),
        };
        let image = image::RgbaImage::from_raw(size.0, size.1, rgba)
            .ok_or("a preview frame had the wrong size")?;
        frames.push(image::Frame::from_parts(
            image,
            0,
            0,
            image::Delay::from_numer_denom_ms((step / 1000).max(20) as u32, 1),
        ));
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Written beside and renamed, so a tile never reads half a file.
    let partial = out.with_extension("gif.part");
    {
        let file =
            std::fs::File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let mut encoder = image::codecs::gif::GifEncoder::new_with_speed(file, 10);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        encoder
            .encode_frames(frames)
            .map_err(|e| format!("the preview GIF could not be written: {e}"))?;
    }
    std::fs::rename(&partial, out).map_err(|e| e.to_string())?;
    Ok(out.to_path_buf())
}
