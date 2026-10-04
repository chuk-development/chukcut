//! Face landmark tracks for the compositor: the retouch effect's side of
//! `modules::landmarks`.
//!
//! [`FaceTracks::faces`] answers the faces of a media file at a source time
//! from its track file, kept in memory and re-read when the file changes —
//! an analysis flushes every couple of seconds, and the preview picks the
//! new frames up as they land. A frame without a track answers no faces,
//! and the retouch effect draws the clip as it is.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use parking_lot::Mutex;

use crate::modules::landmarks::track::{self, Face, FaceFrame};
use crate::modules::project::document::Micros;

/// How long a track file's modification time is trusted before it is
/// checked again.
const RECHECK_AFTER: Duration = Duration::from_millis(500);

struct Loaded {
    /// `None` when the media file cannot be read (offline media).
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    frames: Arc<Vec<FaceFrame>>,
    checked: Instant,
}

/// Tracks by media path, shared by every render of one compositor.
#[derive(Default)]
pub struct FaceTracks {
    loaded: Mutex<HashMap<String, Loaded>>,
}

impl FaceTracks {
    /// The faces of `media` at `source_time`, frames `period` apart.
    pub fn faces(&self, media: &str, source_time: Micros, period: Micros) -> Vec<Face> {
        let frames = self.frames(media);
        track::faces_at(&frames, source_time, period.max(1) * 2)
    }

    fn frames(&self, media: &str) -> Arc<Vec<FaceFrame>> {
        let mut loaded = self.loaded.lock();
        let entry = loaded.entry(media.to_string()).or_insert_with(|| Loaded {
            path: track::path_for(
                media.as_ref(),
                crate::modules::ml::landmarks::model_version(),
            )
            .ok(),
            modified: None,
            frames: Arc::new(Vec::new()),
            // Long ago, so the first call reads.
            checked: Instant::now() - RECHECK_AFTER * 2,
        });
        if entry.checked.elapsed() >= RECHECK_AFTER {
            entry.checked = Instant::now();
            let modified = entry
                .path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .and_then(|m| m.modified().ok());
            if modified != entry.modified {
                entry.modified = modified;
                entry.frames = Arc::new(
                    entry
                        .path
                        .as_ref()
                        .and_then(|p| track::read(p).ok())
                        .unwrap_or_default(),
                );
            }
        }
        Arc::clone(&entry.frames)
    }
}
