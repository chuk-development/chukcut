//! The diagnostic log: what a reader needs from the log file alone.
//!
//! `workspace::logging` decides *where* lines go and keeps the directory
//! bounded. This module decides *what* goes there beyond the ordinary
//! `tracing` events, so that a log a user sends answers three questions
//! without a reproduction:
//!
//! 1. **What hardware is used, and how.** [`startup`] writes one block per
//!    run: version and commit, OS and display server, CPU and RAM, the GPU
//!    adapter, which codecs decode and encode in hardware, FFmpeg's versions,
//!    the settings that choose between them. Grep `startup:`.
//! 2. **Where it went wrong.** Errors and fallbacks are logged where they
//!    happen, as before. [`child`] adds the helper processes' stderr (the ML
//!    worker's provider choice and failures) and [`ffmpeg_log`] adds FFmpeg's
//!    own error messages, both tagged and rate-limited.
//! 3. **Where it was slow.** [`budget`] is the one helper every timing check
//!    goes through: `over budget what=… ms=… budget_ms=…`, at most one line
//!    per place per 10 s. [`resources`] writes the process's memory and CPU
//!    and the card's load every 30 s while the app is busy.
//!
//! Everything here is cheap when nothing is wrong. A check under its budget
//! is a subtraction; a resource sample is a few `/proc` reads.
//!
//! The shells start it: the app calls [`start`] once, with its build. The CLI
//! does not, because it logs to stderr and is not left running.

pub mod budget;
pub mod child;
pub mod ffmpeg_log;
pub mod resources;
pub mod startup;

pub use budget::{budget, check, Budget};
pub use startup::Build;

/// Start the diagnostic log for an interactive shell: the startup block, the
/// resource sampler and FFmpeg's messages. Call after `crate::init`.
pub fn start(build: Build) {
    ffmpeg_log::install();
    startup::report(build);
    resources::start();
}
