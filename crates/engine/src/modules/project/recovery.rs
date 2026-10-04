//! Crash recovery: offering the working copy back after a session that did
//! not end cleanly.
//!
//! [`super::autosave`] keeps a working copy of the open document. This module
//! decides what happens to it at the *next* launch, and the decision is to
//! **offer, not apply** (seam 2 of decision 0004): the app asks "restore the
//! work from last time?" and the user can say no. That matters when the
//! working copy is a project they deliberately walked away from.
//!
//! Three facts make it work:
//!
//! - **A clean exit deletes the working copy** ([`close_at`]). The user has
//!   either saved or chosen to discard by then, so a working copy that
//!   survives to the next launch means the last session crashed or was killed.
//! - **At launch the working copy is moved aside** into a recovery slot
//!   ([`claim_at`]) before anything else can autosave over it. Opening a
//!   project from the command line would otherwise schedule a write that
//!   destroys the very thing the user is about to be offered.
//! - **A running session holds a lock with its process id.** A second instance
//!   started while the first runs must not take the first one's live working
//!   copy for a crash.
//!
//! A working copy that is byte-for-byte the document in the file it came from
//! is not offered: nothing was lost, and asking would teach people to click
//! the prompt away.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::document::Project;

/// What the start screen shows about recoverable work.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryInfo {
    pub name: String,
    /// The user's own file the working copy stands in for, if it had one.
    pub origin: Option<String>,
    /// When the working copy was last written, Unix millis.
    pub written_at: i64,
    /// Clips across all lanes, so the prompt can say how much is at stake.
    pub clips: usize,
    pub duration: super::document::Micros,
}

/// The slot a claimed working copy waits in until the user decides.
pub fn slot_for(working_copy: &Path) -> PathBuf {
    working_copy.with_extension("recovered.chukcut")
}

fn lock_for(working_copy: &Path) -> PathBuf {
    working_copy.with_extension("lock")
}

fn origin_for(file: &Path) -> PathBuf {
    file.with_extension("path")
}

fn modified_millis(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whether the process that wrote `lock` is still running.
///
/// `/proc/<pid>` is enough on Linux, the only platform this targets. A reused
/// pid makes this answer "running" for a dead session, and the cost of that is
/// one launch without the offer; the slot keeps the file for the next one.
fn lock_is_live(lock: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(lock) else {
        return false;
    };
    let Ok(pid) = raw.trim().parse::<u32>() else {
        return false;
    };
    pid != std::process::id() && Path::new(&format!("/proc/{pid}")).exists()
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Whether the working copy holds exactly what its origin file holds.
fn matches_origin(working_copy: &Path) -> bool {
    let Ok(Some(restored)) = super::autosave::read_from(working_copy) else {
        return false;
    };
    let Some(origin) = restored.path else {
        return false;
    };
    let Ok(raw) = std::fs::read_to_string(&origin) else {
        return false;
    };
    let Ok(saved) = super::migrate::load(&raw) else {
        return false;
    };
    serde_json::to_string(&saved.project).ok() == serde_json::to_string(&restored.project).ok()
}

/// Called once at launch. Takes this session's lock and moves a working copy
/// left by a crashed session into the recovery slot. Answers what is waiting
/// in the slot, which may also be work an earlier launch offered and the user
/// put off.
pub fn claim_at(working_copy: &Path) -> Option<RecoveryInfo> {
    let lock = lock_for(working_copy);
    let other_session_running = lock_is_live(&lock);
    if let Some(parent) = lock.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !other_session_running {
        if let Err(error) = std::fs::write(&lock, std::process::id().to_string()) {
            tracing::warn!(%error, "could not write the session lock");
        }
        if working_copy.exists() {
            if matches_origin(working_copy) {
                tracing::info!("the last session's working copy matches its saved file");
                super::autosave::discard_at(working_copy);
            } else {
                let slot = slot_for(working_copy);
                let moved = move_file(working_copy, &slot)
                    .and_then(|()| move_file(&origin_for(working_copy), &origin_for(&slot)));
                match moved {
                    Ok(()) => tracing::info!("the last session did not close cleanly"),
                    Err(error) => tracing::warn!(%error, "could not set the working copy aside"),
                }
            }
        }
    } else {
        tracing::info!("another chukcut is running; its working copy is left alone");
    }
    pending_at(working_copy)
}

/// What waits in the recovery slot, if anything.
pub fn pending_at(working_copy: &Path) -> Option<RecoveryInfo> {
    let slot = slot_for(working_copy);
    let restored = match super::autosave::read_from(&slot) {
        Ok(restored) => restored?,
        Err(error) => {
            tracing::warn!(%error, "the recovered working copy cannot be read");
            return None;
        }
    };
    Some(info_of(&restored.project, restored.path.as_deref(), &slot))
}

fn info_of(project: &Project, origin: Option<&Path>, file: &Path) -> RecoveryInfo {
    RecoveryInfo {
        name: project.name.clone(),
        origin: origin.map(|p| p.to_string_lossy().to_string()),
        written_at: modified_millis(file),
        clips: project.tracks.iter().map(|t| t.segments.len()).sum(),
        duration: project.duration(),
    }
}

/// Take the document out of the recovery slot. The slot is emptied: from
/// here on the restored document is the open one and autosaves as usual.
pub fn take_at(working_copy: &Path) -> Result<super::autosave::Restored, String> {
    let slot = slot_for(working_copy);
    let restored =
        super::autosave::read_from(&slot)?.ok_or("there is no unsaved work to restore")?;
    super::autosave::discard_at(&slot);
    Ok(restored)
}

/// Throw the recovered work away.
pub fn discard_at(working_copy: &Path) {
    super::autosave::discard_at(&slot_for(working_copy));
}

/// A clean end of a document's session: wait for the autosave thread, then
/// delete the working copy. With `exiting`, the session lock goes too.
pub fn close_at(working_copy: &Path, exiting: bool) {
    super::autosave::flush();
    super::autosave::discard_at(working_copy);
    if exiting {
        let lock = lock_for(working_copy);
        if !lock_is_live(&lock) {
            let _ = std::fs::remove_file(lock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::autosave::write_to;
    use crate::modules::project::document::{CanvasConfig, Micros, Segment, Track, TrackKind};

    /// Built from JSON so a field added to `Segment` later does not break it.
    fn clip(start: Micros, duration: Micros) -> Segment {
        serde_json::from_value(serde_json::json!({
            "id": crate::modules::project::document::new_id(),
            "material_id": "material",
            "target_range": { "start": start, "duration": duration },
            "source_range": { "start": 0, "duration": duration },
            "render_index": 0,
        }))
        .expect("a minimal segment")
    }

    /// Per process: another checkout running these tests at the same time
    /// would otherwise delete this directory under a running test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/recovery")
            .join(std::process::id().to_string())
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir.join("autosave.chukcut")
    }

    fn sample(name: &str) -> Project {
        let mut project = Project::new(name, CanvasConfig::default(), 30.0);
        let mut track = Track::new(TrackKind::Video, "V1");
        track.segments.push(clip(0, 2_000_000));
        project.tracks.push(track);
        project
    }

    #[test]
    fn a_crashed_session_is_offered_and_can_be_restored_once() {
        let file = scratch("crash");
        write_to(&file, &sample("afternoon"), None).unwrap();

        let offer = claim_at(&file).expect("the working copy is offered");
        assert_eq!(offer.name, "afternoon");
        assert_eq!(offer.clips, 1);
        assert!(
            !file.exists(),
            "moved aside before anything can overwrite it"
        );

        let restored = take_at(&file).expect("restore");
        assert_eq!(restored.project.name, "afternoon");
        assert!(pending_at(&file).is_none(), "restoring empties the slot");
    }

    #[test]
    fn a_clean_close_leaves_nothing_to_offer() {
        let file = scratch("clean");
        assert!(claim_at(&file).is_none());
        write_to(&file, &sample("done"), None).unwrap();
        close_at(&file, true);
        assert!(claim_at(&file).is_none());
    }

    #[test]
    fn work_that_matches_its_saved_file_is_not_offered() {
        let file = scratch("saved");
        let saved = file.with_file_name("mine.chukcut");
        let project = sample("saved");
        std::fs::write(&saved, serde_json::to_string_pretty(&project).unwrap()).unwrap();
        write_to(&file, &project, Some(&saved)).unwrap();
        assert!(claim_at(&file).is_none());
        assert!(!file.exists());
    }

    #[test]
    fn edits_beyond_the_saved_file_are_offered_with_their_origin() {
        let file = scratch("edited");
        let saved = file.with_file_name("mine.chukcut");
        let project = sample("edited");
        std::fs::write(&saved, serde_json::to_string_pretty(&project).unwrap()).unwrap();
        let mut newer = project.clone();
        newer.tracks[0].segments.push(clip(2_000_000, 1_000_000));
        write_to(&file, &newer, Some(&saved)).unwrap();

        let offer = claim_at(&file).expect("offered");
        assert_eq!(offer.clips, 2);
        assert_eq!(
            offer.origin.as_deref(),
            Some(saved.to_string_lossy().as_ref())
        );
        let restored = take_at(&file).unwrap();
        assert_eq!(restored.path, Some(saved));
    }

    #[test]
    fn put_off_work_waits_for_the_next_launch_and_can_be_discarded() {
        let file = scratch("later");
        write_to(&file, &sample("later"), None).unwrap();
        assert!(claim_at(&file).is_some());
        // The next launch: the slot is still there.
        assert!(claim_at(&file).is_some());
        discard_at(&file);
        assert!(claim_at(&file).is_none());
    }

    #[test]
    fn a_live_session_keeps_its_working_copy() {
        let file = scratch("live");
        // Our parent process is certainly running and is not us.
        let parent = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|stat| stat.split_whitespace().nth(3).map(str::to_string))
            .expect("a parent pid");
        std::fs::write(lock_for(&file), parent).unwrap();
        write_to(&file, &sample("running"), None).unwrap();
        assert!(claim_at(&file).is_none());
        assert!(
            file.exists(),
            "the other session's working copy is untouched"
        );
    }
}
