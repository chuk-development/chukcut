//! One frame of the timeline, at full canvas resolution, as a PNG.
//!
//! This is the export module's little sibling: the same compositor, the same
//! strictness about sources, one frame instead of thousands. It deliberately
//! does **not** ask the preview for its current picture — the preview renders
//! at the player panel's size and the quality ladder may have given up
//! resolution to keep playback smooth, and neither of those belongs in a file
//! the user saves. A snapshot is rendered fresh, at the canvas, every time.
//!
//! PNG rather than JPEG because a still is judged at 100% zoom: the one frame
//! somebody pulls out of a cut is a thumbnail, a poster or a reference, and
//! recompressing what the codec already compressed is the artefact people
//! notice. The file is bigger; nobody saves thousands of them.

use std::path::{Path, PathBuf};

use crate::modules::project::document::{Micros, Project};
use crate::modules::render::{Compositor, SourceProvider};

/// The size a snapshot renders at: the canvas, rounded down to even.
///
/// Even not for the encoder's sake — PNG has no chroma subsampling — but so a
/// snapshot is pixel-for-pixel the frame an export of this project would
/// contain, which is what makes it trustworthy as a reference.
pub fn snapshot_size(project: &Project) -> (u32, u32) {
    let even = |v: u32| (v.max(2)) & !1;
    (even(project.canvas.width), even(project.canvas.height))
}

/// Clamp a requested instant onto the timeline.
///
/// The last representable instant is one microsecond before the end: the
/// duration itself is the first instant *after* the final frame, and rendering
/// there composites an empty canvas.
pub fn clamp_time(project: &Project, time: Micros) -> Result<Micros, String> {
    let duration = project.duration();
    if duration <= 0 {
        return Err("the timeline is empty, so there is no frame to save".into());
    }
    Ok(time.clamp(0, duration - 1))
}

/// Render the frame at `time` and encode it as PNG bytes.
///
/// `time` is clamped to the timeline first, so a playhead parked at the very
/// end still answers with the last frame rather than with black.
pub fn snapshot_png(
    project: &Project,
    time: Micros,
    compositor: &Compositor,
    sources: &dyn SourceProvider,
) -> Result<Vec<u8>, String> {
    let time = clamp_time(project, time)?;
    let size = snapshot_size(project);
    let rgba = compositor
        .render_frame(project, time, size, sources)
        .map_err(|e| format!("rendering the frame failed: {e}"))?;
    encode_png(rgba, size.0, size.1)
}

/// Tightly packed RGBA8 to PNG bytes.
pub fn encode_png(rgba: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "the rendered frame is not the size it claims".to_string())?;
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("encoding the PNG failed: {e}"))?;
    Ok(bytes)
}

/// Render, encode and write, returning the path actually written.
///
/// The extension is forced to `.png` the same way an export's is forced to
/// match its container: the bytes decide what the file is, and a `.jpg` full
/// of PNG data is a lie some viewers refuse to read.
pub fn write_png(
    project: &Project,
    time: Micros,
    compositor: &Compositor,
    sources: &dyn SourceProvider,
    output: &Path,
) -> Result<PathBuf, String> {
    let bytes = snapshot_png(project, time, compositor, sources)?;
    let path = match output.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("png") => output.to_path_buf(),
        _ => output.with_extension("png"),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::CanvasConfig;

    fn project(width: u32, height: u32) -> Project {
        Project::new(
            "snap",
            CanvasConfig {
                width,
                height,
                background: [0.0, 0.0, 0.0, 1.0],
            },
            30.0,
        )
    }

    #[test]
    fn the_snapshot_size_is_the_canvas_rounded_down_to_even() {
        assert_eq!(snapshot_size(&project(1920, 1080)), (1920, 1080));
        // An odd canvas loses its half pixel, exactly as an export would.
        assert_eq!(snapshot_size(&project(1081, 607)), (1080, 606));
    }

    #[test]
    fn time_is_clamped_onto_the_timeline() {
        use crate::modules::project::document::{Track, TrackKind, VideoMaterial};

        let mut project = project(320, 240);
        project.materials.videos.push(VideoMaterial {
            id: "v1".into(),
            path: "/tmp/v1.mp4".into(),
            width: 320,
            height: 240,
            duration: 2_000_000,
            fps: 30.0,
            has_audio: false,
            rotation: 0,
        });
        let mut track = Track::new(TrackKind::Video, "V1");
        track
            .segments
            .push(crate::modules::project::document::Segment {
                id: "s1".into(),
                material_id: "v1".into(),
                target_range: crate::modules::project::document::TimeRange::new(0, 2_000_000),
                source_range: crate::modules::project::document::TimeRange::new(0, 2_000_000),
                render_index: 0,
                speed: 1.0,
                volume: 1.0,
                transform: Default::default(),
                crop: None,
                extras: Vec::new(),
                keyframes: Vec::new(),
            });
        project.tracks.push(track);

        assert_eq!(clamp_time(&project, -5), Ok(0));
        assert_eq!(clamp_time(&project, 1_000_000), Ok(1_000_000));
        // Parked at or past the end still means "the last frame".
        assert_eq!(clamp_time(&project, 2_000_000), Ok(1_999_999));
        assert_eq!(clamp_time(&project, 99_000_000), Ok(1_999_999));
    }

    #[test]
    fn an_empty_timeline_has_no_frame_to_save() {
        let error = clamp_time(&project(320, 240), 0).unwrap_err();
        assert!(error.contains("empty"));
    }

    #[test]
    fn the_png_encoder_round_trips_pixels_exactly() {
        // A 4x2 with distinct corners: PNG is lossless, so what went in must
        // come back byte for byte — which is the whole reason it was chosen.
        let (width, height) = (4u32, 2u32);
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        rgba[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let last = rgba.len() - 4;
        rgba[last..].copy_from_slice(&[0, 255, 0, 255]);

        let bytes = encode_png(rgba.clone(), width, height).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (width, height));
        assert_eq!(decoded.as_raw(), &rgba);
    }

    #[test]
    fn a_buffer_of_the_wrong_size_is_refused_not_misread() {
        assert!(encode_png(vec![0u8; 16], 4, 4).is_err());
    }
}
