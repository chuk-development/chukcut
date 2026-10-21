//! Commands for media inspection.
//!
//! These are the only way the webview learns anything about a file on disk. It
//! never opens one itself — it passes a path the user chose in a dialog and
//! gets back a description, a stream of thumbnail batches, or an audio
//! envelope.
//!
//! ## Why every one of these is `async`
//!
//! A synchronous `command` runs **on the main thread**, which is also
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

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Serialize;

use super::relink;
use crate::shell::Channel;
use crate::state::AppState;

use super::thumbnails;
use super::{BatchSink, MediaError, MediaInfo, ThumbnailBatch, Waveform};

/// Run blocking FFmpeg work off both the main thread and the async runtime.
async fn off_thread<T, F>(work: F) -> std::result::Result<T, String>
where
    F: FnOnce() -> super::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match crate::shell::spawn_blocking(work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(stringify(error)),
        // The worker thread panicked. Report it rather than silently returning
        // nothing: a panic in a decoder is a bug we want to hear about.
        Err(error) => Err(format!("the media task failed: {error}")),
    }
}

/// Describe a media file: container, duration, and the streams it carries.
pub async fn media_probe(path: String) -> std::result::Result<MediaInfo, String> {
    off_thread(move || super::probe(&path)).await
}

/// Which of `paths` are gone from disk right now.
///
/// How the library cards and the timeline clips learn to draw their missing
/// state — the webview may not touch the filesystem itself. Async and off the
/// runtime for the same reason as everything here: a `stat` is instant on a
/// local disk and unbounded on a dead network mount, and the window must not
/// wait on either.
pub async fn media_missing_files(paths: Vec<String>) -> std::result::Result<Vec<String>, String> {
    crate::shell::spawn_blocking(move || {
        paths
            .into_iter()
            .filter(|path| !std::path::Path::new(path).exists())
            .collect()
    })
    .await
    .map_err(|error| format!("the media task failed: {error}"))
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
pub async fn media_thumbnails(
    path: String,
    count: usize,
    height: u32,
    on_batch: Channel<ThumbnailBatch>,
) -> std::result::Result<String, String> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let cancel: Arc<AtomicBool> = thumbnails::begin_job(&job_id);

    let id = job_id.clone();
    // Fire and forget: the job reports everything, its end included, on
    // `on_batch`, so nothing waits for this future.
    let _job = crate::shell::spawn_blocking(move || {
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
pub async fn media_waveform(path: String, buckets: usize) -> std::result::Result<Waveform, String> {
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

// ---------------------------------------------------------------------------
// Relinking moved media (`relink.rs`)
// ---------------------------------------------------------------------------

/// One material pointed at a new file.
#[derive(Debug, Clone, Serialize)]
pub struct Relinked {
    pub material_id: String,
    pub from: String,
    pub to: String,
}

/// A file that has the right name but is not the original.
#[derive(Debug, Clone, Serialize)]
pub struct Refused {
    /// The path the project wanted.
    pub wanted: String,
    pub candidate: String,
    pub reasons: Vec<String>,
}

/// What a relink did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RelinkReport {
    pub relinked: Vec<Relinked>,
    pub refused: Vec<Refused>,
    /// Files still missing afterwards.
    pub still_missing: usize,
}

/// The project's files that are not on disk: what the media panel's
/// "missing" banner counts and what a relink looks for. A `stat` per file;
/// a dead network mount can make that slow, so the app asks off its UI
/// thread.
pub fn media_missing(state: &Arc<AppState>) -> std::result::Result<Vec<relink::Missing>, String> {
    let pool = state.with_project(|p| p.materials.clone())?;
    Ok(relink::missing(&pool)
        .into_iter()
        .filter(|m| !crate::modules::template::assets::is_template_asset(&m.path))
        .collect())
}

/// A planned relink: the material, its place in the pool, the new path.
struct Plan {
    material: crate::modules::timeline::ops::PoolMaterial,
    index: usize,
    from: String,
    to: String,
}

/// Apply plans as one undo step and say what happened.
fn apply_relinks(
    state: &Arc<AppState>,
    plans: Vec<Plan>,
    refused: Vec<Refused>,
) -> std::result::Result<RelinkReport, String> {
    let mut report = RelinkReport {
        refused,
        ..Default::default()
    };
    if !plans.is_empty() {
        let mut commands = Vec::new();
        for plan in &plans {
            commands.extend(relink::relink_edit(
                plan.material.clone(),
                plan.index,
                &plan.to,
            ));
        }
        crate::modules::timeline::commands::timeline_apply(
            state,
            crate::modules::timeline::ops::EditCommand::Composite {
                label: "Relink media".into(),
                commands,
            },
        )?;
        report.relinked = plans
            .into_iter()
            .map(|p| Relinked {
                material_id: p.material.id().to_string(),
                from: p.from,
                to: p.to,
            })
            .collect();
    }
    report.still_missing = media_missing(state)?.len();
    Ok(report)
}

/// Point material `material_id` at the file `path`, after checking it is
/// the same recording (picture, sound, size, length) unless `force`. The
/// folder move this implies is tried on every other missing file, so
/// relinking one file of a moved project folder relinks them all. One undo
/// step.
pub async fn media_relink(
    state: Arc<AppState>,
    material_id: String,
    path: String,
    force: bool,
) -> std::result::Result<RelinkReport, String> {
    let pool = state.with_project(|p| p.materials.clone())?;
    let (material, index, want) = relink::expected(&pool, &material_id)
        .ok_or_else(|| format!("no video, sound or picture with the id {material_id}"))?;
    let worker_state = Arc::clone(&state);
    let (plans, refused) = crate::shell::spawn_blocking(move || {
        let found = relink::Found::probe(Path::new(&path))?;
        let problems = relink::check(&want, &found);
        if !problems.is_empty() && !force {
            return Err(format!(
                "{} does not look like the original: {}",
                path,
                problems.join("; ")
            ));
        }
        let mut plans = vec![Plan {
            material,
            index,
            from: want.path.clone(),
            to: path.clone(),
        }];
        let mut refused = Vec::new();
        if let Some((from, to)) = relink::derive_remap(&want.path, &path) {
            for gone in media_missing(&worker_state)?
                .into_iter()
                .filter(|m| m.material_id != material_id)
            {
                let Some(candidate) = relink::apply_remap(&gone.path, &from, &to) else {
                    continue;
                };
                if !Path::new(&candidate).exists() {
                    continue;
                }
                let Some((m, i, w)) = relink::expected(&pool, &gone.material_id) else {
                    continue;
                };
                match relink::Found::probe(Path::new(&candidate)) {
                    Ok(f) if relink::check(&w, &f).is_empty() => plans.push(Plan {
                        material: m,
                        index: i,
                        from: gone.path,
                        to: candidate,
                    }),
                    Ok(f) => refused.push(Refused {
                        wanted: gone.path,
                        candidate,
                        reasons: relink::check(&w, &f),
                    }),
                    Err(e) => refused.push(Refused {
                        wanted: gone.path,
                        candidate,
                        reasons: vec![e],
                    }),
                }
            }
        }
        Ok((plans, refused))
    })
    .await
    .map_err(|e| format!("the relink failed: {e}"))??;
    apply_relinks(&state, plans, refused)
}

/// How many files a folder search looks at before it stops.
const SEARCH_LIMIT: usize = 200_000;

/// Look in `folder` (and below) for every missing file by its name, and
/// relink each to the first file that is the same recording. One undo step.
pub async fn media_relink_folder(
    state: Arc<AppState>,
    folder: PathBuf,
) -> std::result::Result<RelinkReport, String> {
    let pool = state.with_project(|p| p.materials.clone())?;
    let gone = media_missing(&state)?;
    if gone.is_empty() {
        return Ok(RelinkReport::default());
    }
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    let (plans, refused) = crate::shell::spawn_blocking(move || {
        let index = relink::index_folder(&folder, SEARCH_LIMIT);
        let mut plans = Vec::new();
        let mut refused = Vec::new();
        for g in gone {
            let Some((m, i, want)) = relink::expected(&pool, &g.material_id) else {
                continue;
            };
            let mut why = None;
            for candidate in relink::candidates(&index, &g.path) {
                let reasons = match relink::Found::probe(candidate) {
                    Ok(found) => relink::check(&want, &found),
                    Err(e) => vec![e],
                };
                if reasons.is_empty() {
                    plans.push(Plan {
                        material: m.clone(),
                        index: i,
                        from: g.path.clone(),
                        to: candidate.to_string_lossy().into_owned(),
                    });
                    why = None;
                    break;
                }
                why.get_or_insert((candidate.to_string_lossy().into_owned(), reasons));
            }
            if let Some((candidate, reasons)) = why {
                refused.push(Refused {
                    wanted: g.path,
                    candidate,
                    reasons,
                });
            }
        }
        (plans, refused)
    })
    .await
    .map_err(|e| format!("the search failed: {e}"))?;
    apply_relinks(&state, plans, refused)
}
