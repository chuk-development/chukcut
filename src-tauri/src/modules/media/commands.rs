//! IPC commands for media inspection.
//!
//! These are the only way the webview learns anything about a file on disk. It
//! never opens one itself — it passes a path the user chose in a dialog and
//! gets back a description, a stream of thumbnail batches, or an audio
//! envelope.
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
//! ## Why the thumbnail command answers before the work is done
//!
//! Being off the main thread stops the editor freezing; it does not stop the
//! user staring at empty lanes. Three long clips on a cold cache is tens of
//! seconds during which a single-answer command has nothing to say. So
//! `media_thumbnails` returns a **job id** as soon as the arguments are known
//! to be sane, and the tiles arrive on the caller's own `Channel` as they are
//! produced. That also gives the job somewhere to be cancelled from — by id,
//! with `media_thumbnails_cancel`, or implicitly when the channel is dropped
//! and a send fails.
//!
//! Errors come back as `String` and are shown to the user unchanged, so
//! `MediaError`'s `Display` is written as prose with the path in it.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::ipc::Channel;

use super::thumbnails;
use super::{BatchSink, MediaError, MediaInfo, ThumbnailBatch, Waveform};

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

/// Adapts Tauri's channel to the thumbnail job's sink.
///
/// A wrapper rather than an impl on `Channel` itself so `thumbnails.rs` stays
/// free of Tauri and can be driven from a test. A failed send is the webview
/// going away — the window closed, the component unmounted, the user navigated
/// — and unlike an export, there is no reason to finish work whose only product
/// is pixels for a view that no longer exists. So `false` here stops the job.
struct ChannelSink(Channel<ThumbnailBatch>);

impl BatchSink for ChannelSink {
    fn send(&self, batch: ThumbnailBatch) -> bool {
        match self.0.send(batch) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(%error, "a thumbnail batch had nowhere to go");
                false
            }
        }
    }
}

/// Start rendering a strip of `count` thumbnails, `height` pixels tall.
///
/// Returns the job id to cancel it with. Tiles arrive on `on_batch` as they are
/// produced — cached ones immediately, decoded ones in small batches — and the
/// last message of every job carries `complete: true`, whether it finished, was
/// cancelled, or failed. A failure is reported in that message's `error` rather
/// than by this call, which has already returned by then.
#[tauri::command]
pub async fn media_thumbnails(
    path: String,
    count: usize,
    height: u32,
    on_batch: Channel<ThumbnailBatch>,
) -> std::result::Result<String, String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let cancel: Arc<AtomicBool> = thumbnails::begin_job(&job_id);

    let id = job_id.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sink = ChannelSink(on_batch);
        let outcome = thumbnails::thumbnail_stream(
            std::path::Path::new(&path),
            count,
            height,
            &job_id,
            &cancel,
            &sink,
        );
        thumbnails::end_job(&job_id);

        // An argument or probe failure happens before the job can report
        // anything of its own, so the terminal batch is sent here instead.
        // Every job ends with exactly one `complete` message either way.
        if let Err(error) = outcome {
            sink.send(ThumbnailBatch {
                job_id: job_id.clone(),
                total: count,
                produced: 0,
                tiles: Vec::new(),
                complete: true,
                cancelled: false,
                error: Some(stringify(error)),
            });
        }
    });

    Ok(id)
}

/// Ask a running thumbnail job to stop.
///
/// `false` means there was no such job, which is the ordinary outcome of a
/// component unmounting just as its strip finished. Not an error: reporting it
/// as one would put a message in front of the user about a race with no loser.
#[tauri::command]
pub fn media_thumbnails_cancel(job_id: String) -> bool {
    thumbnails::cancel_job(&job_id)
}

/// Stop every running thumbnail job. For window close and app shutdown.
pub fn cancel_all_thumbnails() {
    thumbnails::cancel_all();
}

/// Reduce a file's audio to a drawable envelope of `buckets` columns.
///
/// One bucket per horizontal pixel of the audio lane is the intended call
/// shape. Unlike the old peak array this is cheap to ask for repeatedly: the
/// decode happens once and is cached on disk at a fixed resolution, so changing
/// zoom re-reduces an array rather than re-reading the file.
#[tauri::command]
pub async fn media_waveform(
    path: String,
    buckets: usize,
) -> std::result::Result<Waveform, String> {
    off_thread(move || super::waveform(&path, buckets)).await
}

fn stringify(error: MediaError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_a_job_nobody_started_is_false_and_not_an_error() {
        assert!(!media_thumbnails_cancel("no-such-job".into()));
    }

    #[test]
    fn a_cancellation_never_reaches_the_user_as_prose() {
        // The terminal batch carries `cancelled`, not a message. If this
        // variant ever grew a user-facing string, the lane would start
        // reporting the user's own click back to them as a failure.
        assert!(MediaError::Cancelled.is_cancellation());
        assert!(!MediaError::Invalid("bad".into()).is_cancellation());
    }
}
