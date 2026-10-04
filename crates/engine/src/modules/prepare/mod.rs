//! Getting an opened project ready: the frames and sounds its clips need
//! that are not in the cache yet, made in the background.
//!
//! Four kinds of derived media are cache, not document (decisions 0025,
//! 0028, 0029, 0024's amendment): a removed background's mattes, optical-flow
//! in-between frames, remade frames (Remove object, Enhance quality) and a
//! compound clip's mix-down of its own sound. After every edit the app asks
//! each module for what is missing. Opening a project is not an edit, so a
//! project opened on a machine whose cache was cleared — or another machine
//! altogether — played its matted clips uncut and its slow motion blended
//! until the first edit. This module is the open path.
//!
//! It is **throttled**, so opening stays fast: it waits a moment
//! ([`GRACE`]) for the first frame, the thumbnails and the waveforms, then
//! scans the cache off the UI thread, and bakes **one clip at a time**,
//! kind after kind (sound first, it is the cheapest; then mattes, flow
//! frames, remade frames). The bakes are the modules' own background jobs,
//! so a clip the user edits meanwhile joins the running bake instead of
//! starting a second one, and the preview shows frames as they land.
//!
//! What the app sees is one combined status ([`commands::prepare_status`]):
//! "Preparing N frames", how far, and a Stop. Stop cancels the running bake
//! the way the inspector's Stop does (the modules then leave that clip alone
//! after later edits) and drops the rest of the queue. A project closed or
//! replaced while it runs ends it.

pub mod commands;
mod run;

use serde::Serialize;

/// How long after opening the preparation waits before it reads the cache.
pub const GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// The preparation of the open project, as one status line shows it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PrepareStatus {
    /// Which run this is; a new one starts with every opened project.
    pub run: u64,
    /// Frames missing when the cache was read: mattes, in-between frames
    /// and remade frames together. 0 until the scan is done.
    pub frames: u32,
    /// How many of them are made.
    pub frames_done: u32,
    /// Compound clips whose sound has to be mixed down.
    pub sounds: u32,
    pub sounds_done: u32,
    /// What runs now ("Removing backgrounds"), while something does.
    pub stage: Option<String>,
    /// The cache has been read; `frames` and `sounds` are final.
    pub scanned: bool,
    /// The run has ended: everything made, stopped, or the project closed.
    pub finished: bool,
    /// Ended by [`commands::prepare_stop`].
    pub stopped: bool,
    /// What could not be made, one sentence per clip that failed.
    pub failures: Vec<String>,
}

impl PrepareStatus {
    /// Whether there is anything to show: work found and not finished.
    pub fn busy(&self) -> bool {
        !self.finished && (self.frames > 0 || self.sounds > 0)
    }

    /// "Preparing 240 frames", "Preparing the sound of 2 compound clips".
    pub fn sentence(&self) -> String {
        let plural =
            |n: u32, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        match (self.frames, self.sounds) {
            (0, 0) => "Preparing".into(),
            (0, sounds) => format!(
                "Preparing the sound of {}",
                plural(sounds, "compound clip", "compound clips")
            ),
            (frames, 0) => format!("Preparing {}", plural(frames, "frame", "frames")),
            (frames, sounds) => format!(
                "Preparing {} and the sound of {}",
                plural(frames, "frame", "frames"),
                plural(sounds, "compound clip", "compound clips")
            ),
        }
    }

    /// How far, `0..=1`, counting a compound clip's sound as one frame.
    pub fn fraction(&self) -> f32 {
        let total = self.frames + self.sounds;
        if total == 0 {
            return 0.0;
        }
        ((self.frames_done + self.sounds_done) as f32 / total as f32).clamp(0.0, 1.0)
    }
}
