//! The IPC surface for proxies.
//!
//! Six commands. The frontend's job is small on purpose: ask for a proxy after
//! an import, subscribe once to the event stream, and show a badge per media
//! item. Nothing here decides anything — the rule is in [`super::decision`] and
//! the queue is in [`super::queue`].
//!
//! The one thing worth noticing is what is *not* here: there is no command that
//! hands the frontend a proxy path to do anything with. The switch happens in
//! Rust, where `PreviewSource` and `ExportSource` can enforce it. A path
//! crossing to TypeScript could come back attached to anything.

use std::path::PathBuf;
use std::sync::Arc;

use crate::shell::Channel;

use super::cache::CacheStats;
use super::queue::{EnqueueOutcome, ProxyEvent, ProxyProgressSink, ProxyState, QueueStatus};
use super::{ProxyCache, ProxyEntry, ProxyQueue};

/// Adapts Tauri's channel to the queue's sink.
struct ChannelSink(Channel<ProxyEvent>);

impl ProxyProgressSink for ChannelSink {
    fn send(&self, event: ProxyEvent) {
        // A failed send means the window went away. Not a reason to stop
        // transcoding: the proxy is still worth having when it comes back.
        if let Err(error) = self.0.send(event) {
            tracing::debug!(%error, "proxy progress had nowhere to go");
        }
    }
}

/// Consider `path` for a proxy under the user's policy: what import and
/// project open call for every video.
///
/// `Off` makes nothing. `Auto` asks [`super::decide`], which builds one for
/// footage this machine cannot decode inside the frame budget — 4K HEVC, say,
/// and not 1080p H.264. `Always` builds one for every file a proxy would be
/// meaningfully smaller than, by telling the rule the file is too slow.
/// Returns at once; the transcode runs on the proxy queue's thread.
pub fn proxy_consider(
    path: String,
    policy: crate::modules::workspace::settings::ProxyPolicy,
) -> Result<Option<EnqueueOutcome>, String> {
    use crate::modules::workspace::settings::ProxyPolicy;
    let queue = ProxyQueue::shared();
    let source = PathBuf::from(path);
    let outcome = match policy {
        ProxyPolicy::Off => return Ok(None),
        ProxyPolicy::Auto => queue.enqueue(&source),
        // A measured cost far past any frame budget: "too slow" by fiat.
        ProxyPolicy::Always => queue.enqueue_with(&source, Some(1.0e9)),
    };
    outcome.map(Some).map_err(|e| e.to_string())
}

/// Consider a file for a proxy. Returns immediately; the transcode, if any,
/// runs in the background.
///
/// Called by the import path for every video it adds. Answering
/// `notNeeded` is the common case and is not a failure.
pub fn proxy_request(path: String) -> Result<EnqueueOutcome, String> {
    ProxyQueue::shared()
        .enqueue(&PathBuf::from(path))
        .map_err(|error| error.to_string())
}

/// Stop a proxy job, queued or running.
///
/// `false` when there is no such job, which includes one that has already
/// finished — closing a panel on the last frame is a race nobody can win.
pub fn proxy_cancel(job_id: String) -> bool {
    ProxyQueue::shared().cancel(&job_id)
}

/// Subscribe to every proxy job's progress.
///
/// Called once, at startup. One stream for all proxy activity rather than one
/// per job: proxies are ambient background work and the UI for them is a single
/// indicator.
pub fn proxy_watch(on_event: Channel<ProxyEvent>) {
    ProxyQueue::shared().subscribe(Arc::new(ChannelSink(on_event)));
}

/// What is queued and what is building.
pub fn proxy_queue_status() -> QueueStatus {
    ProxyQueue::shared().status()
}

/// What the media panel should show next to one file.
pub fn proxy_state(path: String) -> ProxyState {
    ProxyQueue::shared().state_of(&PathBuf::from(path))
}

/// Everything cached, plus the totals, for a settings pane.
pub fn proxy_cache_info() -> ProxyCacheInfo {
    let cache = ProxyCache::shared();
    ProxyCacheInfo {
        stats: cache.stats(),
        entries: cache.entries(),
    }
}

/// Throw every proxy away. Safe by construction — they are all rebuildable.
pub fn proxy_cache_clear() -> Result<(), String> {
    // Whatever is running would otherwise write its output back into a
    // directory the user just asked to be empty.
    ProxyQueue::shared().cancel_all();
    ProxyCache::shared()
        .clear()
        .map_err(|error| error.to_string())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCacheInfo {
    pub stats: CacheStats,
    pub entries: Vec<ProxyEntry>,
}

/// Cancel every proxy job. For window close and app shutdown, alongside
/// `export::commands::cancel_all_exports`.
pub fn cancel_all_proxies() {
    ProxyQueue::shared().cancel_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_an_unknown_job_is_not_an_error() {
        assert!(!proxy_cancel("no-such-job".into()));
    }

    #[test]
    fn asking_about_a_file_that_does_not_exist_is_a_sentence_not_a_panic() {
        let error = proxy_request("/nonexistent/clip.mp4".into())
            .expect_err("a missing file cannot be proxied");
        assert!(error.contains("/nonexistent/clip.mp4"), "{error}");
    }

    #[test]
    fn the_state_of_an_unknown_file_is_none() {
        assert_eq!(
            proxy_state("/nonexistent/clip.mp4".into()),
            ProxyState::None
        );
    }
}
