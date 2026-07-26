//! The working copy.
//!
//! Every edit writes the open document to `workspace::paths::autosave_file()`,
//! and the first thing the app asks for after launch gets whatever is there.
//! Without it, closing the window — or a crash, or a rebuild during
//! development — costs the whole session, because a project that has never
//! been saved lives only in `AppState`.
//!
//! Three things about it are deliberate.
//!
//! **It is not an undo step.** `History` records commands the user performed;
//! autosaving is not one of them, and putting it on the stack would make
//! Ctrl+Z undo a write nobody asked for. Nothing here touches `History`.
//!
//! **It is not the user's file.** Saving explicitly still writes wherever they
//! chose. This is a crash-recovery copy, which is why it lives in the config
//! directory rather than next to their media, and why the path they last saved
//! to is remembered beside it — restoring a session and then not knowing where
//! Ctrl+S goes is only half a restore.
//!
//! **It happens off the calling thread.** An edit command must not wait on the
//! disk. Writes go to one background thread through a single-slot mailbox: a
//! burst of edits collapses to one write, the newest wins, and two writes can
//! never land out of order.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::{Condvar, Mutex};

use super::document::Project;
use crate::modules::workspace::paths;

/// A restored working copy and where it came from.
pub struct Restored {
    pub project: Project,
    /// The file the user last saved to, if they had saved at all.
    pub path: Option<PathBuf>,
    /// Anything the load had to repair or migrate, from `migrate::load`.
    pub warnings: Vec<String>,
}

/// The working copy's own path.
pub fn file() -> PathBuf {
    paths::autosave_file()
}

/// Where the path of the user's real file is remembered.
///
/// A sibling rather than a field inside the document, so that the working copy
/// stays a plain project file: it can be opened, diffed and handed to `chukcut`
/// like any other.
fn origin_file(file: &Path) -> PathBuf {
    file.with_extension("path")
}

/// Write `project` to `file` now, on this thread.
///
/// Same temp-file-and-rename as an explicit save: a crash during the write
/// leaves the previous working copy intact rather than a truncated one, which
/// matters more here than anywhere else — this file is only ever read when
/// something already went wrong.
pub fn write_to(file: &Path, project: &Project, origin: Option<&Path>) -> Result<(), String> {
    let json = serde_json::to_string_pretty(project).map_err(|e| e.to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = file.with_extension("autosaving");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| e.to_string())?;

    let origin_file = origin_file(file);
    match origin {
        Some(path) => std::fs::write(&origin_file, path.to_string_lossy().as_bytes())
            .map_err(|e| e.to_string())?,
        // Explicitly removed rather than left behind: a stale origin would
        // point a restored session's next save at a file it is not.
        None => match std::fs::remove_file(&origin_file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        },
    }
    Ok(())
}

/// Read the working copy at `file`, or `Ok(None)` when there is none.
///
/// Goes through `migrate::load` like any other project file, so a working copy
/// left by an older build is migrated and one holding a value that cannot be
/// written back is repaired rather than lost.
pub fn read_from(file: &Path) -> Result<Option<Restored>, String> {
    let raw = match std::fs::read_to_string(file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read the working copy: {e}")),
    };
    let loaded = super::migrate::load(&raw)?;
    let path = std::fs::read_to_string(origin_file(file))
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    Ok(Some(Restored {
        project: loaded.project,
        path,
        warnings: loaded.warnings,
    }))
}

/// Forget the working copy. Called when its content is safely somewhere else.
pub fn discard_at(file: &Path) {
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(origin_file(file));
}

// ---------------------------------------------------------------------------
// The background writer
// ---------------------------------------------------------------------------

/// Where to write, what to write, and the user's own file it stands in for.
struct Job {
    file: PathBuf,
    project: Project,
    origin: Option<PathBuf>,
}

struct Pending {
    job: Option<Job>,
    writing: bool,
}

struct Writer {
    pending: Mutex<Pending>,
    signal: Condvar,
}

static WRITER: OnceLock<&'static Writer> = OnceLock::new();

fn writer() -> &'static Writer {
    *WRITER.get_or_init(|| {
        let writer: &'static Writer = Box::leak(Box::new(Writer {
            pending: Mutex::new(Pending {
                job: None,
                writing: false,
            }),
            signal: Condvar::new(),
        }));
        std::thread::Builder::new()
            .name("chukcut-autosave".into())
            .spawn(move || loop {
                let job = {
                    let mut pending = writer.pending.lock();
                    while pending.job.is_none() {
                        writer.signal.wait(&mut pending);
                    }
                    pending.writing = true;
                    pending.job.take().expect("checked above")
                };

                if let Err(error) = write_to(&job.file, &job.project, job.origin.as_deref()) {
                    // A failed autosave is not worth interrupting an edit for,
                    // but it is worth knowing about: it usually means the
                    // config directory is unwritable or the disk is full.
                    tracing::warn!(%error, path = %job.file.display(), "could not write the working copy");
                }

                let mut pending = writer.pending.lock();
                pending.writing = false;
                writer.signal.notify_all();
            })
            .expect("the autosave thread starts");
        writer
    })
}

/// Persist `project` in the background, replacing any write that has not
/// started yet.
pub fn schedule(project: &Project, origin: Option<PathBuf>) {
    schedule_to(file(), project, origin);
}

/// [`schedule`], to a path of the caller's choosing.
pub fn schedule_to(file: PathBuf, project: &Project, origin: Option<PathBuf>) {
    let writer = writer();
    let mut pending = writer.pending.lock();
    pending.job = Some(Job {
        file,
        project: project.clone(),
        origin,
    });
    writer.signal.notify_all();
}

/// Block until nothing is queued or in flight. For shutdown and for tests.
pub fn flush() {
    let Some(writer) = WRITER.get() else {
        return;
    };
    let mut pending = writer.pending.lock();
    while pending.job.is_some() || pending.writing {
        writer.signal.wait(&mut pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, Track, TrackKind};

    /// A scratch path per test. `CARGO_TARGET_TMPDIR` is only defined for
    /// integration tests, and the real autosave path must not be touched by a
    /// test run — it is the user's working copy.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("chukcut-autosave-tests");
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir.join(format!("{name}.chukcut"))
    }

    fn sample(name: &str) -> Project {
        let mut project = Project::new(name, CanvasConfig::default(), 30.0);
        project.tracks.push(Track::new(TrackKind::Video, "V1"));
        project
    }

    #[test]
    fn a_working_copy_comes_back_with_the_document_and_where_it_came_from() {
        let file = scratch("round_trip");
        discard_at(&file);

        let project = sample("in progress");
        let origin = PathBuf::from("/projects/in progress.chukcut");
        write_to(&file, &project, Some(&origin)).expect("write");

        let restored = read_from(&file).expect("read").expect("there is one");
        assert_eq!(restored.project.id, project.id);
        assert_eq!(restored.project.tracks.len(), 1);
        assert_eq!(restored.path, Some(origin));
        assert!(restored.warnings.is_empty());

        discard_at(&file);
        assert!(read_from(&file).expect("read").is_none(), "and it is gone");
    }

    #[test]
    fn a_project_that_has_never_been_saved_restores_without_a_path() {
        let file = scratch("no_origin");
        discard_at(&file);

        write_to(&file, &sample("untitled"), Some(Path::new("/old/place.chukcut"))).unwrap();
        // Saving under a new name, then not having one at all, must not leave
        // the old one behind to be saved over.
        write_to(&file, &sample("untitled"), None).unwrap();

        let restored = read_from(&file).expect("read").expect("there is one");
        assert_eq!(restored.path, None);
        discard_at(&file);
    }

    #[test]
    fn there_is_nothing_to_restore_when_nothing_was_written() {
        let file = scratch("absent");
        discard_at(&file);
        assert!(read_from(&file).expect("a missing file is not an error").is_none());
    }

    #[test]
    fn a_damaged_working_copy_is_repaired_rather_than_dropped() {
        // The working copy is read exactly when something already went wrong,
        // so it goes through the same repair the open path uses.
        let file = scratch("damaged");
        let mut project = sample("damaged");
        project.fps = f64::NAN;
        write_to(&file, &project, None).unwrap();

        let raw = std::fs::read_to_string(&file).unwrap();
        assert!(raw.contains("null"), "a NaN is written as null");

        let restored = read_from(&file).expect("read").expect("there is one");
        assert_eq!(restored.project.fps, 30.0);
        assert_eq!(restored.warnings.len(), 1);
        discard_at(&file);
    }

    #[test]
    fn the_writer_collapses_a_burst_and_keeps_the_newest() {
        // The property that matters is ordering: whatever the last scheduled
        // document was, that is what ends up on disk. Twenty edits in a row is
        // a second of dragging a clip around.
        let file = scratch("burst");
        discard_at(&file);

        for n in 0..20 {
            schedule_to(file.clone(), &sample(&format!("edit {n}")), None);
        }
        flush();

        let restored = read_from(&file).expect("read").expect("there is one");
        assert_eq!(restored.project.name, "edit 19");
        discard_at(&file);
    }
}
