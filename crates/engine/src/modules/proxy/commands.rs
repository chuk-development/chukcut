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
use super::{ProxyCache, ProxyEntry, ProxyPolicy, ProxyQueue};

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

/// Consider several files for proxies, on a background thread.
///
/// What opening a project and importing media call. Each file is probed
/// before the decision rule can run — milliseconds each, longer on a network
/// mount — so this returns at once and the probing happens elsewhere. With the
/// policy Off it does nothing at all. A file without a video stream, or one
/// that has gone missing, is skipped: neither is worth a message here.
pub fn proxy_request_media(paths: Vec<String>) {
    let queue = ProxyQueue::shared();
    if paths.is_empty() || queue.policy() == ProxyPolicy::Off {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("chukcut-proxy-request".into())
        .spawn(move || {
            for path in paths {
                match queue.enqueue(&PathBuf::from(&path)) {
                    Ok(outcome) => tracing::debug!(%path, ?outcome, "considered for a proxy"),
                    Err(error) => tracing::debug!(%path, %error, "not considered for a proxy"),
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the proxy request thread");
    }
}

/// Apply Settings → Proxy media: whether imports are queued, and whether the
/// preview decodes the proxies on disk. See [`super::policy`].
pub fn proxy_set_policy(policy: ProxyPolicy) {
    ProxyQueue::shared().set_policy(policy);
}

/// The policy currently in force.
pub fn proxy_policy() -> ProxyPolicy {
    ProxyQueue::shared().policy()
}

/// Bumped whenever the set of proxies the preview should use changes — a
/// proxy finished, or the policy changed. A preview that caches its media
/// provider folds this into the cache key; see decision 0003.
pub fn proxy_generation() -> u64 {
    ProxyQueue::shared().generation()
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

    /// The shared queue is Off until a shell applies the settings, so a test
    /// run never transcodes into the real cache — and Off answers before it
    /// touches the file at all. (A missing file under a live policy is
    /// `queue::tests::a_missing_file_is_a_sentence_not_a_panic`.)
    #[test]
    fn the_shared_queue_starts_off_and_answers_without_io() {
        assert_eq!(proxy_policy(), ProxyPolicy::Off);
        let outcome = proxy_request("/nonexistent/clip.mp4".into()).expect("off is an answer");
        assert!(
            matches!(outcome, EnqueueOutcome::NotNeeded { ref reason } if reason.contains("off")),
            "{outcome:?}"
        );
    }

    #[test]
    fn the_state_of_an_unknown_file_is_none() {
        assert_eq!(
            proxy_state("/nonexistent/clip.mp4".into()),
            ProxyState::None
        );
    }
}
