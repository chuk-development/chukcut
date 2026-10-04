//! Speech denoise, rendered once per file and strength into the cache.
//!
//! RNNoise (via `nnnoiseless`) runs at 48 kHz on 10 ms frames and is far
//! faster than real time on one core, but it is a *recurrent* network: its
//! output depends on everything before it, so it cannot be evaluated at an
//! arbitrary seek point the way a gain can. Running it inside the preview
//! mixer would make a seek sound different from playing through. So the
//! whole file is rendered front to back into a WAV in the cache, and both
//! mixers simply read that file instead of the original — the same samples in
//! preview and export, by construction.
//!
//! The render keeps the original's timeline exactly: RNNoise delays its
//! output by one frame (480 samples), and that frame is dropped here, so
//! source time `t` in the cache file is source time `t` in the original and
//! the picture stays in sync. A test pins it.
//!
//! The cache key is the file's identity (path, size, modification time), the
//! engine id and the strength, quantised to 5 % so a slider does not litter
//! the cache with near-identical renders.

use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::modules::audio::decode::{AudioClipReader, ClipReader};
use crate::modules::export::audio::frames_for;
use crate::modules::project::document::{Micros, Project};
use crate::modules::workspace::paths::cache_root;

use super::cleanup::{cleanup_of, original_source};

/// The engine and model this build renders with. Recorded in the document.
pub const ENGINE: &str = "rnnoise-1";
/// Rate of the rendered file. RNNoise only runs at 48 kHz.
pub const RATE: u32 = 48_000;
/// Channels of the rendered file: the mixers are stereo.
pub const CHANNELS: usize = 2;
/// RNNoise's frame, and its output delay, in sample frames.
const FRAME: usize = nnnoiseless::DenoiseState::FRAME_SIZE;
/// Frames decoded per read: about four seconds.
const CHUNK: usize = FRAME * 400;

/// `strength` on the 5 % grid the cache is keyed on.
pub fn quantize(strength: f32) -> f32 {
    let s = if strength.is_finite() { strength } else { 1.0 };
    (s.clamp(0.0, 1.0) * 20.0).round() / 20.0
}

/// Where the render of `source` at `strength` lives, whether or not it exists.
pub fn cache_path(source: &str, strength: f32, engine: &str) -> PathBuf {
    let (size, modified) = std::fs::metadata(source)
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
        "{source}\u{0}{size}\u{0}{modified}\u{0}{engine}\u{0}{:.2}",
        quantize(strength)
    );
    cache_root()
        .join("voice")
        .join(format!("{:016x}.wav", fnv1a(key.as_bytes())))
}

/// FNV-1a, 64 bit: stable across builds and toolchains, unlike `std`'s hasher,
/// so a cache made yesterday is found today.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One channel's denoiser and the input frame its next output belongs to.
struct Lane {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    /// The previous input frame: what the next output frame lines up with,
    /// and the dry half of the strength mix.
    previous: Vec<f32>,
    primed: bool,
}

/// A streaming denoiser for interleaved stereo at [`RATE`].
///
/// [`Self::push`] takes whole frames and returns the denoised frames that line
/// up with input already seen — one frame behind, which [`Self::flush`]
/// catches up at the end. Output frame `n` is aligned with input frame `n`.
pub struct Denoiser {
    lanes: Vec<Lane>,
    strength: f32,
    pending: Vec<f32>,
    /// Real input samples taken and aligned samples handed back, so the
    /// flush returns exactly the difference and the render is exactly as
    /// long as its source.
    taken: usize,
    given: usize,
}

impl Denoiser {
    pub fn new(strength: f32) -> Self {
        Self {
            lanes: (0..CHANNELS)
                .map(|_| Lane {
                    state: nnnoiseless::DenoiseState::new(),
                    previous: vec![0.0; FRAME],
                    primed: false,
                })
                .collect(),
            strength: quantize(strength),
            pending: Vec::new(),
            taken: 0,
            given: 0,
        }
    }

    /// Feed interleaved samples; get back as many aligned samples as are
    /// ready.
    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(interleaved);
        self.taken += interleaved.len();
        let whole = self.pending.len() / (FRAME * CHANNELS) * FRAME * CHANNELS;
        let input: Vec<f32> = self.pending.drain(..whole).collect();
        let mut out = Vec::with_capacity(whole);
        for frame in input.as_chunks::<{ FRAME * CHANNELS }>().0 {
            self.frame(frame, &mut out);
        }
        self.given += out.len();
        out
    }

    /// Pad the last partial frame, push one frame of silence through to get
    /// the delayed output out, and return what is still owed.
    pub fn flush(&mut self) -> Vec<f32> {
        let owed = self.taken - self.given;
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let mut tail = std::mem::take(&mut self.pending);
            tail.resize(FRAME * CHANNELS, 0.0);
            self.frame(&tail, &mut out);
        }
        let silence = vec![0.0; FRAME * CHANNELS];
        self.frame(&silence, &mut out);
        out.truncate(owed);
        self.given += out.len();
        out
    }

    fn frame(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        let mut wet = vec![[0.0f32; FRAME]; CHANNELS];
        let mut dry = vec![[0.0f32; FRAME]; CHANNELS];
        let mut produced = true;
        for (c, lane) in self.lanes.iter_mut().enumerate() {
            let input: Vec<f32> = (0..FRAME)
                .map(|i| interleaved[i * CHANNELS + c] * 32_768.0)
                .collect();
            let mut output = [0.0f32; FRAME];
            lane.state.process_frame(&mut output, &input);
            for i in 0..FRAME {
                wet[c][i] = output[i] / 32_768.0;
                dry[c][i] = lane.previous[i];
            }
            for (i, s) in input.iter().enumerate() {
                lane.previous[i] = s / 32_768.0;
            }
            if !lane.primed {
                // The first output belongs to the frame before the file
                // started: it is the delay, and it is dropped.
                lane.primed = true;
                produced = false;
            }
        }
        if !produced {
            return;
        }
        let s = self.strength;
        for i in 0..FRAME {
            for c in 0..CHANNELS {
                out.push((s * wet[c][i] + (1.0 - s) * dry[c][i]).clamp(-1.0, 1.0));
            }
        }
    }
}

/// Render `source` (whose material lasts `duration`) at `strength` into the
/// cache, unless it is already there. Returns the cache file.
///
/// Blocking: decodes and denoises the whole file. `progress` gets `0..=1`.
pub fn render(
    source: &str,
    duration: Micros,
    strength: f32,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<PathBuf, String> {
    let target = cache_path(source, strength, ENGINE);
    if target.is_file() {
        return Ok(target);
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the cache: {e}"))?;
    }
    let part = target.with_extension("part");
    let result = render_into(source, duration, strength, &part, cancel, progress);
    match result {
        Ok(()) => {
            std::fs::rename(&part, &target)
                .map_err(|e| format!("cannot finish the cleaned audio: {e}"))?;
            Ok(target)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

fn render_into(
    source: &str,
    duration: Micros,
    strength: f32,
    out: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), String> {
    let mut reader =
        AudioClipReader::open(source, RATE, CHANNELS as u16).map_err(|e| e.to_string())?;
    let total = frames_for(duration, RATE);
    let mut writer = WavWriter::create(out, RATE, CHANNELS as u16)?;
    let mut denoiser = Denoiser::new(strength);
    let mut buffer = vec![0.0f32; CHUNK * CHANNELS];
    let mut read = 0usize;
    while read < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let frames = CHUNK.min(total - read);
        let chunk = &mut buffer[..frames * CHANNELS];
        reader
            .read(read as i64, frames, chunk)
            .map_err(|e| e.to_string())?;
        writer.write(&denoiser.push(chunk))?;
        read += frames;
        progress(read as f32 / total.max(1) as f32);
    }
    writer.write(&denoiser.flush())?;
    writer.finish()
}

/// Render every voice isolation and denoise cache `project` refers to that
/// is missing.
///
/// The export calls this before it mixes. It is what "export must never
/// silently differ from the preview" costs when a cache was cleared: the
/// render is redone with the engine and strength the document records.
///
/// Compound clips count: the clips inside them are walked as the mixers
/// hear them (`sequence::audio::flatten_audio_rendered`), and a compound clip
/// that is denoised itself has its mix-down rendered and then denoised.
pub fn ensure_rendered(project: &Project, cancel: &AtomicBool) -> Result<(), String> {
    let flat = crate::modules::sequence::audio::flatten_audio_rendered(project, cancel)?;
    let project = flat.as_ref();
    let mut done = std::collections::BTreeSet::new();
    for track in &project.tracks {
        for segment in &track.segments {
            let Some((_, cleanup)) = cleanup_of(project, segment) else {
                continue;
            };
            let Some((path, duration)) = original_source(project, segment) else {
                continue;
            };
            // Isolation first: the denoise reads what it kept.
            if let Some(isolate) = &cleanup.isolate {
                super::isolate::render(&path, duration, isolate, cancel, &|_| {})
                    .map_err(|e| format!("could not isolate the voice of {path}: {e}"))?;
            }
            let Some(denoise) = cleanup.denoise.clone() else {
                continue;
            };
            let path = super::cleanup::denoise_input(&path, &cleanup);
            if denoise.engine != ENGINE {
                tracing::warn!(
                    engine = %denoise.engine,
                    "a clip was cleaned with an engine this build does not have; rendering with {ENGINE}"
                );
            }
            let key = (path.clone(), (quantize(denoise.strength) * 100.0) as i32);
            if !done.insert(key) {
                continue;
            }
            render(&path, duration, denoise.strength, cancel, &|_| {})?;
        }
    }
    Ok(())
}

/// The smallest WAV writer that works: 16-bit PCM, sizes patched on finish.
pub(crate) struct WavWriter {
    file: BufWriter<std::fs::File>,
    frames: u64,
    channels: u16,
}

impl WavWriter {
    pub(crate) fn create(path: &Path, rate: u32, channels: u16) -> Result<Self, String> {
        let file =
            std::fs::File::create(path).map_err(|e| format!("cannot write {path:?}: {e}"))?;
        let mut writer = Self {
            file: BufWriter::new(file),
            frames: 0,
            channels,
        };
        let block = channels as u32 * 2;
        let mut header = Vec::with_capacity(44);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16u32.to_le_bytes());
        header.extend_from_slice(&1u16.to_le_bytes());
        header.extend_from_slice(&channels.to_le_bytes());
        header.extend_from_slice(&rate.to_le_bytes());
        header.extend_from_slice(&(rate * block).to_le_bytes());
        header.extend_from_slice(&(block as u16).to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(b"data");
        header.extend_from_slice(&0u32.to_le_bytes());
        writer.file.write_all(&header).map_err(|e| e.to_string())?;
        Ok(writer)
    }

    pub(crate) fn write(&mut self, interleaved: &[f32]) -> Result<(), String> {
        let mut bytes = Vec::with_capacity(interleaved.len() * 2);
        for s in interleaved {
            let v = (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.frames += (interleaved.len() / self.channels.max(1) as usize) as u64;
        self.file.write_all(&bytes).map_err(|e| e.to_string())
    }

    pub(crate) fn finish(mut self) -> Result<(), String> {
        let data = self.frames * self.channels as u64 * 2;
        if data > u32::MAX as u64 - 36 {
            return Err("the cleaned audio is longer than a WAV file can hold".into());
        }
        let io = |e: std::io::Error| e.to_string();
        self.file.seek(SeekFrom::Start(4)).map_err(io)?;
        self.file
            .write_all(&(36 + data as u32).to_le_bytes())
            .map_err(io)?;
        self.file.seek(SeekFrom::Start(40)).map_err(io)?;
        self.file
            .write_all(&(data as u32).to_le_bytes())
            .map_err(io)?;
        self.file.flush().map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click train in noise-free stereo: an impulse every 4800 frames.
    fn clicks(frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * CHANNELS];
        for f in (1_000..frames).step_by(4_800) {
            out[f * CHANNELS] = 0.9;
            out[f * CHANNELS + 1] = 0.9;
        }
        out
    }

    fn run(denoiser: &mut Denoiser, input: &[f32], piece: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for chunk in input.chunks(piece) {
            out.extend(denoiser.push(chunk));
        }
        out.extend(denoiser.flush());
        out
    }

    #[test]
    fn the_output_is_exactly_as_long_as_the_input_whatever_the_chunking() {
        for (frames, piece) in [(48_000, 7_777), (48_000 + 123, 960), (100, 64)] {
            let input = clicks(frames);
            let out = run(&mut Denoiser::new(1.0), &input, piece);
            assert_eq!(
                out.len(),
                input.len(),
                "{frames} frames in pieces of {piece}"
            );
        }
    }

    #[test]
    fn strength_zero_is_the_input_unchanged_and_in_place() {
        // The dry path proves the alignment: if the delay were not removed,
        // every click would come out one frame late.
        let input = clicks(48_000);
        let out = run(&mut Denoiser::new(0.0), &input, 4_096);
        for (a, b) in input.iter().zip(&out) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn the_denoised_signal_is_aligned_with_the_original() {
        // A voiced, vowel-like signal: 150 Hz with harmonics, swelling at
        // 4 Hz. Its period is 320 samples, so a copy one RNNoise frame (480)
        // late is half a period out and anti-correlates — the correlation at
        // lag 0 against lag 480 says which one we got.
        let frames = 96_000;
        let mut input = vec![0.0f32; frames * CHANNELS];
        for f in 0..frames {
            let t = f as f32 / 48_000.0;
            let swell = 0.5 + 0.5 * (t * 4.0 * std::f32::consts::TAU).sin();
            let v: f32 = (1..8)
                .map(|h| (t * 150.0 * h as f32 * std::f32::consts::TAU).sin() / h as f32)
                .sum::<f32>()
                * 0.2
                * swell;
            input[f * CHANNELS] = v;
            input[f * CHANNELS + 1] = v;
        }
        let out = run(&mut Denoiser::new(1.0), &input, 4_800);
        let corr = |lag: usize| -> f32 {
            (4_800..frames - 1_000)
                .map(|f| input[f * CHANNELS] * out[(f + lag) * CHANNELS])
                .sum()
        };
        let aligned = corr(0);
        let late = corr(FRAME);
        // In phase at lag 0, half a period out (so negative) one frame late.
        // A render that kept RNNoise's delay would show the opposite signs.
        assert!(
            aligned > 0.0 && late < 0.0,
            "aligned {aligned}, one frame late {late}"
        );
    }

    #[test]
    fn strength_is_quantised_for_the_cache_key() {
        assert_eq!(quantize(0.81), 0.8);
        assert_eq!(quantize(2.0), 1.0);
        assert_eq!(quantize(f32::NAN), 1.0);
        assert_eq!(
            cache_path("/x.wav", 0.81, ENGINE),
            cache_path("/x.wav", 0.79, ENGINE)
        );
        assert_ne!(
            cache_path("/x.wav", 0.8, ENGINE),
            cache_path("/x.wav", 0.5, ENGINE)
        );
    }
}
