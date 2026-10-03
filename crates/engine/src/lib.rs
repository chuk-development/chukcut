//! chukcut engine — media, timeline, GPU compositor, audio, preview and export.
//!
//! No UI lives here. The native app in `crates/app` is the shell: it owns the
//! window and calls into these modules directly. The command layer under
//! `modules/*/commands.rs` is the shell-facing API; [`shell`] supplies the two
//! primitives it needs (a blocking-task spawner and an event channel).

pub mod modules;
pub mod shell;
pub mod state;

use modules::audio::FileAudioSource;

/// Process-wide setup every shell runs once at start.
///
/// Logging to stdout and to a file under the state directory, a clean preview
/// cache (frames left from a previous run describe a document that no longer
/// exists), and the exporter's audio source.
pub fn init() {
    modules::workspace::logging::init();

    if let Err(error) =
        std::fs::remove_dir_all(modules::workspace::paths::cache_root().join("preview"))
    {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%error, "could not clear the preview cache");
        }
    }

    // The proxy policy and the cache limit. Also the startup cache trim, on a
    // thread of its own.
    modules::workspace::commands::workspace_settings_apply(&modules::workspace::Settings::load());

    // Until this was registered the exporter mixed a valid but silent audio
    // track, because no implementation of its `AudioSource` seam existed.
    modules::export::job::register_audio_source(std::sync::Arc::new(FileAudioSource));
}
