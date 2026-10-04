//! Isolate voice: the speech of a recording without the music and noise
//! under it — or the reverse, the music without the voice.
//!
//! Rendered like the denoise ([`super::denoise`]) and for the same reason:
//! the model (HTDemucs in the ML worker, `ml::separate`) sees seconds of
//! sound at once, so it cannot run inside a mixer that seeks. Two cache
//! files per recording:
//!
//! 1. **The voice stem**: the model's vocals for the whole file, 44.1 kHz
//!    stereo. The slow part — keyed by the file and the model's version,
//!    so it is made once whatever the strength.
//! 2. **The mix**: the stem and the original mixed by the strength
//!    (`voice + (1 − strength) · rest`, or the reverse for "keep the
//!    background"). Quick, one pass over two files; keyed by the strength
//!    on the 5 % grid as well.
//!
//! The mixers read the mix through [`super::effective_source`]. Until it
//! exists the clip plays as it was (the preview never waits); an export
//! renders it first (`denoise::ensure_rendered`) and fails in words if the
//! worker cannot.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::denoise::{fnv1a, quantize, WavWriter};
use crate::modules::audio::decode::{AudioClipReader, ClipReader};
use crate::modules::export::audio::frames_for;
use crate::modules::ml::separate::{self, Separator, HOP, RATE, SEGMENT};
use crate::modules::project::document::Micros;
use crate::modules::workspace::paths::cache_root;

/// What to keep of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Keep {
    /// The voice; music and noise go. CapCut's "Isolate voice".
    #[default]
    Voice,
    /// Everything but the voice: "Remove vocals" for a song, or the room
    /// without the speaker.
    Background,
}

/// The setting on a clip, inside its voice cleanup block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Isolate {
    /// `0..=1`: how much of what is not kept goes. 1 removes it, 0.5 halves
    /// it. On the 5 % grid.
    pub strength: f32,
    #[serde(default)]
    pub keep: Keep,
    /// The model and version that made the stem, recorded so a missing cache
    /// is made again with the same one (ml-features §5.6).
    pub model: String,
}

/// The model id and version this build separates with.
pub fn current_model() -> String {
    format!("{}-{}", separate::MODEL, separate::model_version())
}

/// A file's identity for a cache key: path, size, modification time.
fn identity(source: &str) -> String {
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
    format!("{source}\u{0}{size}\u{0}{modified}")
}

/// Where the voice stem of `source` by `model` lives.
pub fn stem_path(source: &str, model: &str) -> PathBuf {
    let key = format!("{}\u{0}{model}\u{0}stem", identity(source));
    cache_root()
        .join("voice")
        .join(format!("{:016x}-voice.wav", fnv1a(key.as_bytes())))
}

/// Where the mix a clip plays lives, whether or not it exists.
pub fn cache_path(source: &str, isolate: &Isolate) -> PathBuf {
    let key = format!(
        "{}\u{0}{}\u{0}{:?}\u{0}{:.2}",
        identity(source),
        isolate.model,
        isolate.keep,
        quantize(isolate.strength)
    );
    cache_root()
        .join("voice")
        .join(format!("{:016x}-isolated.wav", fnv1a(key.as_bytes())))
}

/// Make the stem and the mix for `source` (whose material lasts
/// `duration`) unless they are cached. Returns the mix. Blocking: the stem
/// runs the model over the whole file. `progress` gets `0..=1`.
pub fn render(
    source: &str,
    duration: Micros,
    isolate: &Isolate,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<PathBuf, String> {
    let target = cache_path(source, isolate);
    if target.is_file() {
        return Ok(target);
    }
    if isolate.model != current_model() {
        tracing::warn!(
            model = %isolate.model,
            "a clip was isolated with a model this build does not have; using {}",
            current_model()
        );
    }
    let stem = stem_path(source, &isolate.model);
    if !stem.is_file() {
        separate::prepare(&|_, _| {}, cancel).map_err(|e| e.to_string())?;
        atomically(&stem, |part| {
            render_stem(source, duration, part, cancel, &|f| progress(f * 0.95))
        })?;
    }
    atomically(&target, |part| {
        render_mix(source, &stem, duration, isolate, part, cancel)
    })?;
    progress(1.0);
    Ok(target)
}

/// Write through a partial file renamed into place, so a reader (the
/// preview, another render) never sees half a WAV.
fn atomically(
    target: &Path,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create the cache: {e}"))?;
    }
    let part = target.with_extension(format!("{}.part", std::process::id()));
    match write(&part) {
        Ok(()) => std::fs::rename(&part, target)
            .map_err(|e| format!("cannot finish the isolated voice: {e}")),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

/// The model over the whole file, in overlapping segments.
fn render_stem(
    source: &str,
    duration: Micros,
    out: &Path,
    cancel: &AtomicBool,
    progress: &dyn Fn(f32),
) -> Result<(), String> {
    let mut reader = AudioClipReader::open(source, RATE, 2).map_err(|e| e.to_string())?;
    let total = frames_for(duration, RATE);
    if total == 0 {
        return Err("the clip has no sound to separate".into());
    }
    let mut writer = WavWriter::create(out, RATE, 2)?;
    let mut separator = Separator::new();
    let mut buffer = vec![0.0f32; SEGMENT * 2];
    let mut start = 0usize;
    let mut written = 0usize;
    let emit = |writer: &mut WavWriter, [l, r]: [Vec<f32>; 2], written: &mut usize| {
        let n = l.len().min(total - *written);
        let interleaved: Vec<f32> = (0..n).flat_map(|i| [l[i], r[i]]).collect();
        *written += n;
        writer.write(&interleaved)
    };
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let frames = SEGMENT.min(total - start);
        let chunk = &mut buffer[..frames * 2];
        reader
            .read(start as i64, frames, chunk)
            .map_err(|e| e.to_string())?;
        let left: Vec<f32> = chunk.iter().step_by(2).copied().collect();
        let right: Vec<f32> = chunk.iter().skip(1).step_by(2).copied().collect();
        let (l, r, _) =
            separate::separate_segment(&left, &right, Some(cancel)).map_err(|e| e.to_string())?;
        emit(&mut writer, separator.push([l, r]), &mut written)?;
        progress(((start + frames) as f32 / total as f32).min(1.0));
        if start + frames >= total {
            break;
        }
        start += HOP;
    }
    emit(&mut writer, separator.finish(), &mut written)?;
    writer.finish()
}

/// The stem and the original, mixed by the setting.
fn render_mix(
    source: &str,
    stem: &Path,
    duration: Micros,
    isolate: &Isolate,
    out: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    const CHUNK: usize = 1 << 16;
    let mut original = AudioClipReader::open(source, RATE, 2).map_err(|e| e.to_string())?;
    let mut voice = AudioClipReader::open(stem, RATE, 2).map_err(|e| e.to_string())?;
    let total = frames_for(duration, RATE);
    let mut writer = WavWriter::create(out, RATE, 2)?;
    let rest = 1.0 - quantize(isolate.strength);
    let (mut m, mut v) = (vec![0.0f32; CHUNK * 2], vec![0.0f32; CHUNK * 2]);
    let mut at = 0usize;
    while at < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let n = CHUNK.min(total - at);
        original
            .read(at as i64, n, &mut m[..n * 2])
            .map_err(|e| e.to_string())?;
        voice
            .read(at as i64, n, &mut v[..n * 2])
            .map_err(|e| e.to_string())?;
        let mixed: Vec<f32> = m[..n * 2]
            .iter()
            .zip(&v[..n * 2])
            .map(|(&mix, &vocal)| {
                let other = mix - vocal;
                match isolate.keep {
                    Keep::Voice => vocal + rest * other,
                    Keep::Background => other + rest * vocal,
                }
            })
            .collect();
        writer.write(&mixed)?;
        at += n;
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(strength: f32, keep: Keep) -> Isolate {
        Isolate {
            strength,
            keep,
            model: "m-1".into(),
        }
    }

    #[test]
    fn the_cache_is_keyed_by_strength_keep_and_model_but_the_stem_only_by_model() {
        let a = cache_path("/x.wav", &setting(1.0, Keep::Voice));
        assert_eq!(a, cache_path("/x.wav", &setting(0.99, Keep::Voice)));
        assert_ne!(a, cache_path("/x.wav", &setting(0.5, Keep::Voice)));
        assert_ne!(a, cache_path("/x.wav", &setting(1.0, Keep::Background)));
        let mut other = setting(1.0, Keep::Voice);
        other.model = "m-2".into();
        assert_ne!(a, cache_path("/x.wav", &other));
        assert_eq!(stem_path("/x.wav", "m-1"), stem_path("/x.wav", "m-1"));
        assert_ne!(stem_path("/x.wav", "m-1"), stem_path("/x.wav", "m-2"));
        assert_ne!(stem_path("/x.wav", "m-1"), a);
    }

    #[test]
    fn the_setting_reads_old_and_new_spellings() {
        let s: Isolate = serde_json::from_str(r#"{"strength":0.8,"model":"m"}"#).unwrap();
        assert_eq!(s.keep, Keep::Voice);
        let s: Isolate =
            serde_json::from_str(r#"{"strength":1,"keep":"background","model":"m"}"#).unwrap();
        assert_eq!(s.keep, Keep::Background);
    }
}
