//! Proxy media — small, cheap-to-decode stand-ins for footage this machine
//! cannot play back at full resolution.
//!
//! This is the standard answer to 4K, and every editor that survives contact
//! with real footage builds it. `docs/decisions/0001-keep-the-custom-media-stack.md`
//! names proxies as one of the things neither GES nor MLT gives away: Pitivi
//! transcodes its own on top of GES, Kdenlive and Shotcut theirs on top of MLT.
//! So it is ours to write, and this module is it.
//!
//! ## The four questions, kept apart
//!
//! 1. **Is a proxy worth making?** [`decision`]. Not "is it 4K" — the input is
//!    resolution *and* codec *and*, when we have one, a measured decode cost.
//!    H.264 at 1080p decodes inside the frame budget on this machine; HEVC at
//!    2160p does not; an intra-only source may not need one at any size,
//!    because what a proxy really buys on those is nothing.
//! 2. **What does the proxy look like?** [`generate`]. All-intra H.264 at
//!    720-ish, hardware-encoded when [`crate::modules::export::hwaccel`] says
//!    an encoder on this machine actually works, software otherwise.
//! 3. **When does it get built?** [`queue`]. On a background thread, one file
//!    at a time, cancellable, reporting progress. **Never** on the import path:
//!    the user edits with the original from the first second and the proxy
//!    swaps itself in when it is ready.
//! 4. **Which file does a given consumer open?** [`switch`]. Preview may use
//!    the proxy. Export may not, ever, and the type system is what says so.
//!
//! ## The one bug this module exists to not have
//!
//! A proxy reaching the export is a silent 720p deliverable: nothing fails,
//! nothing warns, and it is discovered after the upload. Every editor that has
//! shipped proxies has shipped this bug at least once.
//!
//! So the guard is structural rather than careful. [`switch::PreviewSource`] is
//! the only type that can carry a proxy path, and nothing in the export path
//! accepts one. [`switch::ExportSource`] is the type the export path accepts,
//! it is constructed from an original, and its constructor **refuses any path
//! underneath [`crate::modules::workspace::paths::proxies_dir`]** — so even a
//! future caller that reaches around the types and hands over a raw `PathBuf`
//! gets an error rather than a soft file. Both halves are tested.
//!
//! ## Cache
//!
//! [`cache`], under `paths::proxies_dir()`. Keyed on the source's path, byte
//! size and modification time together, so re-exporting a source over its own
//! filename invalidates its proxy instead of silently editing against the old
//! picture. Capped in total bytes with least-recently-used eviction, because a
//! cache without a ceiling is a disk-full bug on a long enough timeline.

use std::path::PathBuf;

pub mod cache;
pub mod commands;
pub mod decision;
pub mod generate;
pub mod policy;
pub mod queue;
pub mod switch;

#[cfg(test)]
mod tests;

pub use cache::{CacheStats, ProxyCache, ProxyEntry, SourceKey};
pub use decision::{decide, CodecClass, Decision, SourceProfile};
pub use generate::{generate, plan, GeneratedProxy, ProxySpec};
pub use policy::{decide_with_policy, ProxyPolicy};
pub use queue::{
    EnqueueOutcome, FfmpegTranscoder, FnSink, ProxyEvent, ProxyJobView, ProxyProgressSink,
    ProxyQueue, ProxyStage, ProxyState, QueueStatus, Transcoder,
};
pub use switch::{is_inside_proxy_cache, ExportSource, PreviewSource};

/// Everything that can stop a proxy being made or used.
///
/// As with `ExportError`, the `Display` strings are written to be shown to a
/// person unchanged — the command layer only calls `to_string()`.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Media(#[from] crate::modules::media::MediaError),

    #[error("{0}")]
    Encode(#[from] crate::modules::export::ExportError),

    /// The source has nothing to make a proxy of.
    #[error("{0} has no video stream, so there is nothing to proxy")]
    NoVideo(PathBuf),

    /// Not a failure — the user asked to stop. Carried as an error so the
    /// frame loop unwinds with `?`, and turned back into an outcome at the top.
    #[error("the proxy was cancelled")]
    Cancelled,

    /// The load-bearing refusal. See the module docs.
    #[error(
        "{0} is a proxy, and a proxy must never be exported — the export always \
         reads the original file"
    )]
    ProxyInDeliverable(PathBuf),

    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ProxyError>;

impl ProxyError {
    pub fn is_cancellation(&self) -> bool {
        matches!(self, ProxyError::Cancelled)
    }

    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> ProxyError {
        let path = path.into();
        move |source| ProxyError::Io { path, source }
    }
}
