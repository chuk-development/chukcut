//! The preparation commands the app calls (`modules::prepare`).
//!
//! - [`prepare_start`] — after a project opened: bake what it is missing,
//!   in the background, one clip at a time.
//! - [`prepare_status`] — the combined progress, for one status line.
//! - [`prepare_stop`] — the status line's Stop.
//! - [`prepare_missing`] — what a project lacks, counted, without baking.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::run::{self, Run};
use crate::modules::project::Project;
use crate::state::AppState;

pub use super::PrepareStatus;

/// Start preparing the open project: wait [`super::GRACE`], read what its
/// clips are missing — in every timeline and inside compound clips — and
/// bake it in the background, one clip at a time. A run already going (for
/// the project open before) is stopped first. Answers the run's number.
pub fn prepare_start(state: &Arc<AppState>) -> u64 {
    run::start(Arc::clone(state), super::GRACE)
}

/// The current run's status; `None` before the first run.
pub fn prepare_status() -> Option<PrepareStatus> {
    run::current().map(|r| r.status.lock().clone())
}

/// Stop the current run: the running bake is cancelled (the frames made so
/// far stay) and the rest is not started.
pub fn prepare_stop() {
    if let Some(r) = run::current() {
        stop(&r);
    }
}

/// What `project` lacks, counted the way [`prepare_start`] counts it — in
/// every timeline and inside compound clips — without baking anything.
/// Reads the cache's directories: call it off the UI thread.
pub fn prepare_missing(project: &Project) -> PrepareStatus {
    let mut status = PrepareStatus {
        scanned: true,
        finished: true,
        ..PrepareStatus::default()
    };
    run::count(&run::scan(project), &mut status);
    status
}

fn stop(r: &Run) {
    r.cancel.store(true, Ordering::Relaxed);
    r.status.lock().stopped = true;
    run::cancel_current(r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sentence_names_frames_and_sounds() {
        let mut s = PrepareStatus {
            frames: 240,
            ..PrepareStatus::default()
        };
        assert_eq!(s.sentence(), "Preparing 240 frames");
        s.sounds = 1;
        assert_eq!(
            s.sentence(),
            "Preparing 240 frames and the sound of 1 compound clip"
        );
        s.frames = 0;
        s.sounds = 2;
        assert_eq!(s.sentence(), "Preparing the sound of 2 compound clips");
        s.sounds_done = 1;
        assert!((s.fraction() - 0.5).abs() < 1e-6);
        assert!(s.busy());
        s.finished = true;
        assert!(!s.busy());
        let v = PrepareStatus {
            frames: 90,
            voices: 1,
            ..PrepareStatus::default()
        };
        assert_eq!(v.sentence(), "Preparing 90 frames and 1 voice");
        assert!(v.busy());
        let all = PrepareStatus {
            frames: 2,
            voices: 2,
            sounds: 1,
            ..PrepareStatus::default()
        };
        assert_eq!(
            all.sentence(),
            "Preparing 2 frames, 2 voices and the sound of 1 compound clip"
        );
    }
}
