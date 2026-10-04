//! A compound clip's sound, mixed down, for the effects it carries itself.
//!
//! Sound is a sum, so a compound clip is normally heard by laying its
//! contents out on lanes of their own (`super::audio`). That stops working
//! the moment the compound clip processes its sound itself: an equaliser on
//! the sum is the sum of the equalised parts, but a compressor, a reverb
//! tail, noise reduction or a loudness gain measured on the whole are not.
//! So a compound clip with its own audio effects or voice cleanup is
//! **mixed down** first — its sequence mixed, front to back, the way the
//! export mixes a timeline — into a cached file, and that file is then the
//! compound clip's source: an audio material whose time is the sequence's
//! time, which is the compound clip's source time. From there every existing
//! piece applies unchanged: voice cleanup denoises and normalises that file
//! (`voice::effective_source`), `audiofx` renders it through the compound
//! clip's speed, curve and effect stack, and the mixers place it with the
//! compound clip's volume and keyframes.
//!
//! The file is content-addressed by `super::digest::sound_digest` — the
//! sequence's lanes, the pool entries its clips name, and the identity of
//! every file they read — so an edit inside makes a new mix-down, an undo
//! finds the old one, and two clips of one sequence share it.
//!
//! The export and every measurement render a missing mix-down before they
//! mix ([`render`]); the preview asks for it in the background
//! ([`heard`]) and hears the contents dry until it lands, as it does with
//! any other processed clip (`audiofx::cache`).

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock};

use parking_lot::{Condvar, Mutex};

use crate::modules::audio::decode::FileAudioSource;
use crate::modules::audiofx::cache::write_wav_f32;
use crate::modules::audiofx::render::{CHANNELS, RATE};
use crate::modules::project::{AudioMaterial, Micros, Project, Segment};
use crate::modules::workspace::paths::cache_root;

/// Bumped whenever a mix-down of the same document would sound different.
const VERSION: u32 = 1;

/// The id prefix of the audio material a mix-down is heard through. These
/// materials exist only in the mixers' flattened copy, never in a document.
pub const MATERIAL_PREFIX: &str = "compound-mix:";

/// Whether the compound clip `segment` processes its sound itself: an audio
/// effect, voice cleanup, or a speed change whose pitch is to follow.
pub fn processes_sound(project: &Project, segment: &Segment) -> bool {
    let fx = crate::modules::audiofx::fx_or_default(project, segment);
    let retimed =
        project.materials.speed_curve_of(segment).is_some() || (segment.speed - 1.0).abs() > 1e-6;
    !fx.active_effects().is_empty()
        || (fx.pitch_follows_speed && retimed)
        || crate::modules::voice::cleanup::cleanup_of(project, segment)
            .is_some_and(|(_, c)| !c.is_identity())
}

/// Where the mix-down of sequence `id` lives, and how long it is. `None` for
/// a sequence that is not parked.
pub fn source_of(project: &Project, id: &str) -> Option<(PathBuf, Micros)> {
    let key = super::digest::sound_digest(&project.materials, id)?;
    let duration = super::duration_of(project, id)?;
    let path = cache_root()
        .join("compound-mix")
        .join(format!("{key:016x}-v{VERSION}.wav"));
    Some((path, duration))
}

/// The audio material the mixers hear a mix-down through.
fn material(path: &std::path::Path, duration: Micros) -> AudioMaterial {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    AudioMaterial {
        id: format!("{MATERIAL_PREFIX}{name}"),
        path: path.to_string_lossy().into_owned(),
        duration,
        sample_rate: RATE,
        channels: CHANNELS as u16,
    }
}

/// Mix sequence `id` down into the cache unless it is there. Blocking: it
/// decodes everything the sequence plays, and renders the processed clips
/// inside it first.
pub fn render(project: &Project, id: &str, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let (path, _) = source_of(project, id).ok_or("that compound clip's contents are gone")?;
    if path.is_file() {
        return Ok(path);
    }
    let view = super::nested(project, id).ok_or("that compound clip's contents are gone")?;
    render_view(&view, &path, cancel)?;
    Ok(path)
}

fn render_view(view: &Project, path: &std::path::Path, cancel: &AtomicBool) -> Result<(), String> {
    crate::modules::voice::denoise::ensure_rendered(view, cancel)?;
    let samples = crate::modules::export::mix_timeline_unclamped(
        view,
        &FileAudioSource,
        RATE,
        CHANNELS as u16,
        cancel,
    )
    .map_err(|e| e.to_string())?;
    let dir = path.parent().ok_or("the cache has no directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the cache: {e}"))?;
    // A unique part name: the preview's worker and an export may mix the
    // same sequence at once, and the rename makes either one's result the
    // file.
    let part = path.with_extension(format!("{}.part", uuid::Uuid::new_v4().simple()));
    match write_wav_f32(&part, &samples, RATE, CHANNELS as u16) {
        Ok(()) => std::fs::rename(&part, path)
            .map_err(|e| format!("cannot finish the compound clip's sound: {e}")),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

/// How a flattening pass treats a mix-down that is not cached yet.
#[derive(Clone, Copy)]
pub enum Wait<'a> {
    /// Ask for it in the background and hear the contents dry meanwhile.
    No,
    /// Render it now; the flag cancels.
    Yes(&'a AtomicBool),
}

/// The material compound clip `segment` (of sequence `id`, processing its
/// own sound) is heard through, or `None` while the preview waits for it.
pub fn heard(
    project: &Project,
    segment: &Segment,
    wait: Wait<'_>,
) -> Result<Option<AudioMaterial>, String> {
    let id = &segment.material_id;
    let Some((path, duration)) = source_of(project, id) else {
        return Ok(None);
    };
    match wait {
        Wait::Yes(cancel) => {
            render(project, id, cancel)?;
            Ok(Some(material(&path, duration)))
        }
        Wait::No => {
            let denoise = crate::modules::voice::cleanup::cleanup_of(project, segment)
                .and_then(|(_, c)| c.denoise)
                .map(|d| d.strength);
            let denoised = |path: &std::path::Path| {
                denoise.is_none_or(|strength| {
                    crate::modules::voice::denoise::cache_path(
                        &path.to_string_lossy(),
                        strength,
                        crate::modules::voice::denoise::ENGINE,
                    )
                    .is_file()
                })
            };
            if path.is_file() {
                if !denoised(&path) {
                    request(path.clone(), duration, None, denoise);
                }
                return Ok(Some(material(&path, duration)));
            }
            if let Some(view) = super::nested(project, id) {
                request(path, duration, Some(view), denoise);
            }
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// The background mixer, for the preview
// ---------------------------------------------------------------------------

struct Job {
    path: PathBuf,
    duration: Micros,
    /// The sequence to mix, when the mix-down itself is missing.
    view: Option<Project>,
    /// Denoise the mix-down at this strength too.
    denoise: Option<f32>,
}

struct Queue {
    pending: VecDeque<Job>,
    busy: HashSet<PathBuf>,
    failed: HashSet<(PathBuf, u32)>,
}

struct Worker {
    queue: Mutex<Queue>,
    wake: Condvar,
}

fn worker() -> &'static Arc<Worker> {
    static WORKER: OnceLock<Arc<Worker>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let w = Arc::new(Worker {
            queue: Mutex::new(Queue {
                pending: VecDeque::new(),
                busy: HashSet::new(),
                failed: HashSet::new(),
            }),
            wake: Condvar::new(),
        });
        let thread = Arc::clone(&w);
        std::thread::Builder::new()
            .name("chukcut-compound-mix".into())
            .spawn(move || worker_loop(thread))
            .expect("spawn the compound mix-down thread");
        w
    })
}

/// A key for "this job failed", so a broken mix-down is not retried on
/// every re-plan.
fn failure_key(job: &Job) -> (PathBuf, u32) {
    (
        job.path.clone(),
        job.denoise.map_or(u32::MAX, |s| (s * 1000.0) as u32),
    )
}

fn request(path: PathBuf, duration: Micros, view: Option<Project>, denoise: Option<f32>) {
    let w = worker();
    let mut q = w.queue.lock();
    let job = Job {
        path,
        duration,
        view,
        denoise,
    };
    if q.busy.contains(&job.path) || q.failed.contains(&failure_key(&job)) {
        return;
    }
    // The newest request for a file replaces an older one.
    q.pending.retain(|j| j.path != job.path);
    q.pending.push_back(job);
    w.wake.notify_one();
}

fn worker_loop(w: Arc<Worker>) {
    let never = AtomicBool::new(false);
    loop {
        let job = {
            let mut q = w.queue.lock();
            loop {
                if let Some(job) = q.pending.pop_front() {
                    q.busy.insert(job.path.clone());
                    break job;
                }
                w.wake.wait(&mut q);
            }
        };
        let result = (|| -> Result<(), String> {
            if let Some(view) = &job.view {
                if !job.path.is_file() {
                    render_view(view, &job.path, &never)?;
                }
            }
            if let Some(strength) = job.denoise {
                crate::modules::voice::denoise::render(
                    &job.path.to_string_lossy(),
                    job.duration,
                    strength,
                    &never,
                    &|_| {},
                )?;
            }
            Ok(())
        })();
        let mut q = w.queue.lock();
        q.busy.remove(&job.path);
        match result {
            Ok(()) => {
                drop(q);
                crate::modules::audiofx::cache::notify_landed();
            }
            Err(error) => {
                tracing::warn!(path = %job.path.display(), %error, "a compound clip's sound could not be mixed down; it plays dry");
                q.failed.insert(failure_key(&job));
            }
        }
    }
}
