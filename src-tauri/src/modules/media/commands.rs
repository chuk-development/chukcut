//! IPC commands for media inspection.
//!
//! These are the only way the webview learns anything about a file on disk. It
//! never opens one itself — it passes a path the user chose in a dialog and
//! gets back a description, a list of cached thumbnail paths, or a peak array.
//!
//! ## Why every one of these is `async`
//!
//! A synchronous `#[tauri::command]` runs **on the main thread**, which is also
//! the thread that draws the window. Decoding a thumbnail strip from a cold
//! cache takes seconds, and doing that on the main thread does not merely make
//! the import slow — it freezes the entire editor while it happens, which is
//! exactly what it did. Making the command `async` moves it onto the async
//! runtime, and `spawn_blocking` moves the FFmpeg work off that runtime's
//! worker threads too, so neither the UI nor the IPC layer ever waits on a
//! decoder.
//!
//! Errors come back as `String` and are shown to the user unchanged, so
//! `MediaError`'s `Display` is written as prose with the path in it.

use super::{MediaError, MediaInfo};

/// Run blocking FFmpeg work off both the main thread and the async runtime.
async fn off_thread<T, F>(work: F) -> std::result::Result<T, String>
where
    F: FnOnce() -> super::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(stringify(error)),
        // The worker thread panicked. Report it rather than silently returning
        // nothing: a panic in a decoder is a bug we want to hear about.
        Err(error) => Err(format!("the media task failed: {error}")),
    }
}

/// Describe a media file: container, duration, and the streams it carries.
#[tauri::command]
pub async fn media_probe(path: String) -> std::result::Result<MediaInfo, String> {
    off_thread(move || super::probe(&path)).await
}

/// Render a strip of `count` thumbnails, `height` pixels tall, and return the
/// cache paths in timeline order.
///
/// Still one call rather than a stream of batches: with the decode off the main
/// thread the interface stays responsive while it runs, so the remaining win
/// from streaming is only showing partial strips sooner. Worth doing, not worth
/// blocking on.
#[tauri::command]
pub async fn media_thumbnails(
    path: String,
    count: usize,
    height: u32,
) -> std::result::Result<Vec<String>, String> {
    off_thread(move || super::thumbnail_strip(&path, count, height)).await
}

/// Reduce a file's audio to `buckets` normalized peaks for waveform drawing.
///
/// One bucket per horizontal pixel of the audio lane is the intended call
/// shape; asking for far more than the lane is wide costs decode time and buys
/// nothing the user can see.
#[tauri::command]
pub async fn media_waveform(
    path: String,
    buckets: usize,
) -> std::result::Result<Vec<f32>, String> {
    off_thread(move || super::waveform(&path, buckets)).await
}

fn stringify(error: MediaError) -> String {
    error.to_string()
}
