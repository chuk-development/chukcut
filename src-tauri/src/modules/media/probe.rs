//! Reading what a media file *is* without decoding any of it.
//!
//! Probing is what turns a path the user picked into a `VideoMaterial`: the
//! import path needs duration, dimensions, frame rate and rotation before it
//! can place a single segment on the timeline.
//!
//! Everything here is read out of `AVCodecParameters` and the container's
//! stream table rather than by opening a decoder. Opening a decoder to learn a
//! width costs a codec init per file, and fails outright on a stream we have no
//! decoder for — which should still be *listed*, with a clear error later,
//! rather than making the whole import fail.

use std::path::Path;

use ffmpeg_next as ffmpeg;
use serde::Serialize;

use super::{ensure_initialized, ts_to_micros, MediaError, Result};
use crate::modules::project::Micros;

/// Everything the import path needs to know about a file.
#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub path: String,
    /// The demuxer's short name, e.g. `"mov,mp4,m4a,3gp,3g2,mj2"` or
    /// `"matroska,webm"`. Useful for diagnostics and for deciding whether a
    /// file is worth building a proxy for.
    pub format: String,
    /// Length of the longest stream, in microseconds. `0` when the container
    /// declares no duration (some live captures and raw streams).
    pub duration: Micros,
    pub file_size: u64,
    pub has_video: bool,
    pub has_audio: bool,
    pub video: Option<VideoStreamInfo>,
    pub audio: Option<AudioStreamInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoStreamInfo {
    /// Index into the container's stream table, so a decoder can reopen the
    /// same stream without repeating the "best stream" heuristic.
    pub index: usize,
    /// Coded dimensions, *before* display rotation is applied.
    pub width: u32,
    pub height: u32,
    /// Dimensions as the user expects to see them, i.e. with `rotation`
    /// applied. A portrait phone video is 1920x1080 coded and 1080x1920 here.
    pub display_width: u32,
    pub display_height: u32,
    pub fps: f64,
    pub codec: String,
    /// Display rotation from the container's display matrix, in degrees
    /// clockwise, always one of 0/90/180/270.
    pub rotation: i32,
    pub duration: Micros,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioStreamInfo {
    pub index: usize,
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: String,
    pub duration: Micros,
}

/// Inspect `path` and report its container, streams and duration.
pub fn probe(path: impl AsRef<Path>) -> Result<MediaInfo> {
    ensure_initialized();

    let path = path.as_ref();
    let input = ffmpeg::format::input(&path).map_err(|source| MediaError::Open {
        path: path.to_path_buf(),
        source,
    })?;

    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let format = input.format().name().to_string();

    let video = input
        .streams()
        .best(ffmpeg::media::Type::Video)
        .map(|stream| video_info(&stream));
    let audio = input
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .map(|stream| audio_info(&stream));

    // The container duration is already in `AV_TIME_BASE` units, which is
    // microseconds — but it is absent often enough (fragmented MP4, raw
    // streams) that falling back to the longest stream is worth the code.
    let container_duration = normalize_container_duration(input.duration());
    let duration = container_duration
        .max(video.as_ref().map(|v| v.duration).unwrap_or(0))
        .max(audio.as_ref().map(|a| a.duration).unwrap_or(0));

    Ok(MediaInfo {
        path: path.to_string_lossy().into_owned(),
        format,
        duration,
        file_size,
        has_video: video.is_some(),
        has_audio: audio.is_some(),
        video,
        audio,
    })
}

/// Probe a file that must have a video stream, erroring with a sentence the
/// user can act on when it does not.
pub(crate) fn probe_video(path: &Path) -> Result<(MediaInfo, VideoStreamInfo)> {
    let info = probe(path)?;
    let video = info
        .video
        .clone()
        .ok_or_else(|| MediaError::NoVideoStream(path.to_path_buf()))?;
    Ok((info, video))
}

fn video_info(stream: &ffmpeg::Stream) -> VideoStreamInfo {
    let parameters = stream.parameters();
    // SAFETY: the parameters block belongs to the input context, which outlives
    // this borrow, and these fields are plain integers on every FFmpeg 6 build.
    let (width, height) = unsafe {
        let raw = parameters.as_ptr();
        ((*raw).width.max(0) as u32, (*raw).height.max(0) as u32)
    };

    let rotation = display_rotation(stream);
    let (display_width, display_height) = if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    };

    VideoStreamInfo {
        index: stream.index(),
        width,
        height,
        display_width,
        display_height,
        fps: frame_rate(stream),
        codec: parameters.id().name().to_string(),
        rotation,
        duration: stream_duration(stream),
    }
}

fn audio_info(stream: &ffmpeg::Stream) -> AudioStreamInfo {
    let parameters = stream.parameters();
    // SAFETY: as above. `channels` is deprecated in FFmpeg 6 but still
    // populated; the replacement `ch_layout` is not exposed by ffmpeg-next 6.1.
    let (sample_rate, channels) = unsafe {
        let raw = parameters.as_ptr();
        ((*raw).sample_rate.max(0) as u32, (*raw).channels.max(0) as u16)
    };

    AudioStreamInfo {
        index: stream.index(),
        sample_rate,
        channels,
        codec: parameters.id().name().to_string(),
        duration: stream_duration(stream),
    }
}

/// Frame rate as a float, preferring the container's average over the
/// theoretical maximum.
///
/// `r_frame_rate` is the lowest rate that can represent every timestamp
/// exactly, which for a variable-rate phone recording is often 600 or 1000 —
/// not a number anyone wants shown in a media panel. `avg_frame_rate` is the
/// honest answer when the container knows it.
fn frame_rate(stream: &ffmpeg::Stream) -> f64 {
    let avg = f64::from(stream.avg_frame_rate());
    if avg.is_finite() && avg > 0.0 {
        return avg;
    }
    let real = f64::from(stream.rate());
    if real.is_finite() && real > 0.0 {
        return real;
    }
    0.0
}

fn stream_duration(stream: &ffmpeg::Stream) -> Micros {
    let raw = stream.duration();
    if raw <= 0 {
        return 0;
    }
    ts_to_micros(raw, stream.time_base())
}

fn normalize_container_duration(duration: i64) -> Micros {
    // FFmpeg reports an unknown duration as AV_NOPTS_VALUE, which is i64::MIN.
    if duration <= 0 {
        0
    } else {
        duration
    }
}

/// Display rotation in degrees clockwise, read from the container's display
/// matrix side data.
///
/// Phones record landscape and tag the file with a rotation instead of
/// re-encoding, so a preview that ignores this shows every vertical video on
/// its side. `av_display_rotation_get` returns the *counter-clockwise* angle
/// the matrix applies, which is why the sign flips here — the same convention
/// `ffmpeg` itself uses when it prints `rotate: 90`.
fn display_rotation(stream: &ffmpeg::Stream) -> i32 {
    for side_data in stream.side_data() {
        if side_data.kind() != ffmpeg::packet::side_data::Type::DisplayMatrix {
            continue;
        }
        let bytes = side_data.data();
        if bytes.len() < 9 * std::mem::size_of::<i32>() {
            continue;
        }
        // SAFETY: a display matrix is nine `int32_t`, and the buffer is at
        // least that long. FFmpeg allocates it aligned for the type.
        let degrees = unsafe { ffmpeg::ffi::av_display_rotation_get(bytes.as_ptr() as *const i32) };
        return normalize_rotation(-degrees);
    }
    0
}

/// Snap an arbitrary rotation angle to the 0/90/180/270 the renderer supports.
///
/// Display matrices are floats and encoders are sloppy: 89.999 and -90 and 270
/// all mean the same thing, and a NaN matrix (seen in the wild in files written
/// by broken muxers) must not poison the frame path.
pub fn normalize_rotation(degrees: f64) -> i32 {
    if !degrees.is_finite() {
        return 0;
    }
    let quarters = (degrees / 90.0).round() as i64;
    (quarters.rem_euclid(4) * 90) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_snaps_to_quarter_turns() {
        assert_eq!(normalize_rotation(0.0), 0);
        assert_eq!(normalize_rotation(90.0), 90);
        assert_eq!(normalize_rotation(180.0), 180);
        assert_eq!(normalize_rotation(270.0), 270);
    }

    #[test]
    fn rotation_wraps_negatives_and_overflow() {
        assert_eq!(normalize_rotation(-90.0), 270);
        assert_eq!(normalize_rotation(-180.0), 180);
        assert_eq!(normalize_rotation(-270.0), 90);
        assert_eq!(normalize_rotation(360.0), 0);
        assert_eq!(normalize_rotation(450.0), 90);
    }

    #[test]
    fn rotation_tolerates_encoder_sloppiness() {
        assert_eq!(normalize_rotation(89.9999), 90);
        assert_eq!(normalize_rotation(-89.9999), 270);
        assert_eq!(normalize_rotation(0.4), 0);
    }

    #[test]
    fn rotation_of_a_broken_matrix_is_upright() {
        assert_eq!(normalize_rotation(f64::NAN), 0);
        assert_eq!(normalize_rotation(f64::INFINITY), 0);
    }

    #[test]
    fn unknown_container_duration_reads_as_zero() {
        assert_eq!(normalize_container_duration(i64::MIN), 0);
        assert_eq!(normalize_container_duration(-1), 0);
        assert_eq!(normalize_container_duration(5_000_000), 5_000_000);
    }
}
