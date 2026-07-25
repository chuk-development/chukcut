//! IPC commands for media inspection.
//!
//! These are the only way the webview learns anything about a file on disk. It
//! never opens one itself — it passes a path the user chose in a dialog and
//! gets back a description, a list of cached thumbnail paths, or a peak array.
//!
//! Errors come back as `String` and are shown to the user unchanged, so
//! `MediaError`'s `Display` is written as prose with the path in it. There is
//! nothing to translate here; the mapping is deliberately a plain
//! `to_string()`.

use super::{MediaError, MediaInfo};

/// Describe a media file: container, duration, and the streams it carries.
///
/// Fast enough to stay synchronous — probing reads the container header and
/// FFmpeg's stream-info probe, which is a handful of milliseconds even on a
/// cold cache, and no decoder is opened.
#[tauri::command]
pub fn media_probe(path: String) -> std::result::Result<MediaInfo, String> {
    super::probe(&path).map_err(stringify)
}

/// Render a strip of `count` thumbnails, `height` pixels tall, and return the
/// cache paths in timeline order.
///
/// TODO: this must move to a `Channel<ThumbnailBatch>` before the media library
/// ships. A hundred thumbnails of a long clip is seconds of decoding, and the
/// IPC contract is explicit that a command must not block for more than a few
/// milliseconds — the webview should get frames as they land and be able to
/// abandon the job when the user scrolls away. It is synchronous now only
/// because nothing calls it yet, and a blocking version is easier to verify
/// against real files first.
#[tauri::command]
pub fn media_thumbnails(
    path: String,
    count: usize,
    height: u32,
) -> std::result::Result<Vec<String>, String> {
    super::thumbnail_strip(&path, count, height).map_err(stringify)
}

/// Reduce a file's audio to `buckets` normalized peaks for waveform drawing.
///
/// One bucket per horizontal pixel of the audio lane is the intended call
/// shape; asking for far more than the lane is wide costs decode time and buys
/// nothing the user can see.
#[tauri::command]
pub fn media_waveform(path: String, buckets: usize) -> std::result::Result<Vec<f32>, String> {
    super::waveform(&path, buckets).map_err(stringify)
}

fn stringify(error: MediaError) -> String {
    error.to_string()
}
