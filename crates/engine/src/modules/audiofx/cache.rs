//! Cached renders for the preview, and the background thread that makes them.
//!
//! The preview mixer works block by block from any position, which is the
//! wrong shape for a phase vocoder or a reverb: both have state, and a seek
//! would start them cold and sound different from playing through. So the
//! preview never runs them. It asks for a render of the clip's spec
//! ([`super::render::RenderSpec`]) at 48 kHz, plays the clip as it would have
//! before this module until the render lands, and then plays the render — the
//! same samples the export computes for the same spec.
//!
//! The renders are content-addressed: the file name is a hash of the spec
//! and the source file's identity (path, size, modification time), so an
//! undo finds the render it had, two clips with the same settings share one,
//! and a changed file is rendered afresh. They are 32-bit float WAV, so a
//! boost from an equaliser is not clipped on the way through the cache.
//!
//! When a render finishes, [`generation`] moves on; the audio engine's fill
//! thread watches it and re-plans, which is how the sound changes under a
//! playing preview without anything polling the disk.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::{Condvar, Mutex};

use crate::modules::audio::decode::FileAudioSource;
use crate::modules::workspace::paths::cache_root;

use super::render::{render, RenderSpec, CHANNELS, RATE, VERSION};

/// Where the render of `spec` lives, whether or not it exists yet.
pub fn cache_path(spec: &RenderSpec) -> PathBuf {
    let (size, modified) = std::fs::metadata(&spec.path)
        .map(|m| {
            let modified = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            (m.len(), modified)
        })
        .unwrap_or((0, 0));
    let key = format!(
        "{VERSION}\u{0}{size}\u{0}{modified}\u{0}{}",
        spec.key_text()
    );
    cache_root()
        .join("audiofx")
        .join(format!("{:016x}.wav", fnv1a(key.as_bytes())))
}

/// FNV-1a, 64 bit: stable across builds, unlike `std`'s hasher, so a cache
/// made yesterday is found today.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Render `spec` into the cache unless it is already there. Blocking.
pub fn render_to_cache(spec: &RenderSpec, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let target = cache_path(spec);
    if target.is_file() {
        return Ok(target);
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the cache: {e}"))?;
    }
    let samples = render(spec, &FileAudioSource, RATE, cancel)?;
    // A unique part name: two threads may render the same spec at once (the
    // preview's worker and a command), and the rename makes either one's
    // result the file.
    let part = target.with_extension(format!("{}.part", uuid::Uuid::new_v4().simple()));
    let written = write_wav_f32(&part, &samples, RATE, CHANNELS as u16);
    match written {
        Ok(()) => {
            std::fs::rename(&part, &target)
                .map_err(|e| format!("cannot finish the rendered audio: {e}"))?;
            Ok(target)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

/// Write interleaved `f32` as a WAVE_FORMAT_IEEE_FLOAT file.
pub fn write_wav_f32(path: &Path, samples: &[f32], rate: u32, channels: u16) -> Result<(), String> {
    let frames = samples.len() / channels.max(1) as usize;
    let mut w = WavF32Writer::create(path, frames, rate, channels)?;
    w.write(samples)?;
    w.finish()
}

/// A WAVE_FORMAT_IEEE_FLOAT file written in pieces, for a mix too long to
/// hold: the length is known up front, so the header is final from the start.
pub struct WavF32Writer {
    path: std::path::PathBuf,
    w: BufWriter<std::fs::File>,
    bytes: Vec<u8>,
}

impl WavF32Writer {
    /// Create `path` for `frames` sample frames of `channels` at `rate`.
    pub fn create(path: &Path, frames: usize, rate: u32, channels: u16) -> Result<Self, String> {
        let io = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
        let data = frames as u64 * channels as u64 * 4;
        if data > u32::MAX as u64 - 50 {
            return Err("the rendered audio is longer than a WAV file can hold".into());
        }
        let file = std::fs::File::create(path).map_err(io)?;
        let mut w = BufWriter::new(file);
        let block = channels as u32 * 4;
        let mut header = Vec::with_capacity(58);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&(50 + data as u32).to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&18u32.to_le_bytes());
        header.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        header.extend_from_slice(&channels.to_le_bytes());
        header.extend_from_slice(&rate.to_le_bytes());
        header.extend_from_slice(&(rate * block).to_le_bytes());
        header.extend_from_slice(&(block as u16).to_le_bytes());
        header.extend_from_slice(&32u16.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // cbSize
                                                       // Non-PCM WAVE files carry a fact chunk with the frame count.
        header.extend_from_slice(b"fact");
        header.extend_from_slice(&4u32.to_le_bytes());
        header.extend_from_slice(&(frames as u32).to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&(data as u32).to_le_bytes());
        w.write_all(&header).map_err(io)?;
        Ok(Self {
            path: path.to_path_buf(),
            w,
            bytes: Vec::with_capacity(64 * 1024),
        })
    }

    pub fn write(&mut self, samples: &[f32]) -> Result<(), String> {
        let path = &self.path;
        let io = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
        for chunk in samples.chunks(16 * 1024) {
            self.bytes.clear();
            for s in chunk {
                self.bytes.extend_from_slice(&s.to_le_bytes());
            }
            self.w.write_all(&self.bytes).map_err(io)?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        let path = &self.path;
        self.w
            .flush()
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

// ---------------------------------------------------------------------------
// The background renderer
// ---------------------------------------------------------------------------

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Moves on every time a render lands. The audio engine re-plans when it
/// does.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// Say that some other cached render the plan reads has landed (a compound
/// clip's mixed-down sound, `sequence::bounce`), so the preview re-plans.
pub(crate) fn notify_landed() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

struct Queue {
    /// Newest request per owner (a segment id): an older one for the same
    /// clip is superseded, so dragging a slider renders the value it was
    /// dropped at and not every value it passed.
    pending: VecDeque<(String, RenderSpec)>,
    /// Cache files being rendered or already failed, by path.
    busy: HashSet<PathBuf>,
    failed: HashMap<PathBuf, String>,
}

struct Renderer {
    queue: Mutex<Queue>,
    wake: Condvar,
}

fn renderer() -> &'static Arc<Renderer> {
    static RENDERER: OnceLock<Arc<Renderer>> = OnceLock::new();
    RENDERER.get_or_init(|| {
        let r = Arc::new(Renderer {
            queue: Mutex::new(Queue {
                pending: VecDeque::new(),
                busy: HashSet::new(),
                failed: HashMap::new(),
            }),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&r);
        std::thread::Builder::new()
            .name("chukcut-audiofx-render".into())
            .spawn(move || worker_loop(worker))
            .expect("spawn the audio render thread");
        r
    })
}

fn worker_loop(r: Arc<Renderer>) {
    let never = AtomicBool::new(false);
    loop {
        let (spec, path) = {
            let mut q = r.queue.lock();
            loop {
                if let Some((_, spec)) = q.pending.pop_front() {
                    let path = cache_path(&spec);
                    if path.is_file() || q.busy.contains(&path) || q.failed.contains_key(&path) {
                        continue;
                    }
                    q.busy.insert(path.clone());
                    break (spec, path);
                }
                r.wake.wait(&mut q);
            }
        };
        let started = std::time::Instant::now();
        // Contained: a bug in one clip's render fails that clip (it plays
        // unprocessed) instead of ending the thread every render waits on.
        let result =
            crate::lifecycle::contained("The audio render", || render_to_cache(&spec, &never));
        let mut q = r.queue.lock();
        q.busy.remove(&path);
        match result {
            Ok(_) => {
                tracing::debug!(
                    path = %spec.path,
                    ms = started.elapsed().as_millis() as u64,
                    "audio render ready"
                );
                drop(q);
                GENERATION.fetch_add(1, Ordering::AcqRel);
            }
            Err(error) => {
                tracing::warn!(path = %spec.path, %error, "audio render failed; the clip plays unprocessed");
                q.failed.insert(path, error);
            }
        }
    }
}

/// Ask for `spec` to be rendered in the background on behalf of `owner` (a
/// segment id). Returns at once.
pub fn request(owner: &str, spec: RenderSpec) {
    let r = renderer();
    let mut q = r.queue.lock();
    q.pending.retain(|(o, _)| o != owner);
    q.pending.push_back((owner.to_string(), spec));
    r.wake.notify_one();
}

/// Where the preview should read `spec` from: its cached render if it exists.
/// Otherwise asks for the render and answers `None`.
pub fn ready_or_request(owner: &str, spec: &RenderSpec) -> Option<PathBuf> {
    let path = cache_path(spec);
    if path.is_file() {
        return Some(path);
    }
    request(owner, spec.clone());
    None
}

/// Whether a render for `spec` failed, and why.
pub fn failure(spec: &RenderSpec) -> Option<String> {
    let r = renderer();
    let q = r.queue.lock();
    q.failed.get(&cache_path(spec)).cloned()
}
