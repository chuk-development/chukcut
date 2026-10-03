//! Playing a voice sample or a stock preview before it is used.
//!
//! One sample at a time on its own output stream, apart from the timeline's
//! audio engine: starting a new one stops the last. Remote samples are
//! fetched into the cache first, then decoded through the same reader the
//! mixer uses.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::modules::audio::{
    AudioClipReader, AudioOutput, ClipReader as _, DeviceClock, MIX_CHANNELS,
};
use crate::modules::workspace::paths;

static PLAYING: Mutex<Option<Arc<AtomicBool>>> = Mutex::new(None);

/// Stop whatever sample is playing.
pub fn stop() {
    if let Some(flag) = PLAYING.lock().take() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Whether a sample is playing.
pub fn is_playing() -> bool {
    PLAYING
        .lock()
        .as_ref()
        .is_some_and(|f| !f.load(Ordering::Relaxed))
}

/// Play `source`, a URL or a file path, and return at once. Errors (no
/// device, a file that does not decode) end up in the log: an audition that
/// does not sound is its own report.
pub fn play(source: &str) {
    stop();
    let flag = Arc::new(AtomicBool::new(false));
    *PLAYING.lock() = Some(Arc::clone(&flag));
    let source = source.to_string();
    let _ = std::thread::Builder::new()
        .name("audition".into())
        .spawn(move || {
            if let Err(error) = run(&source, &flag) {
                tracing::warn!(%error, "the audition did not play");
            }
            flag.store(true, Ordering::Relaxed);
        });
}

fn run(source: &str, stop: &AtomicBool) -> Result<(), String> {
    let path = if source.starts_with("http://") || source.starts_with("https://") {
        super::stock::cached_preview(
            &paths::cache_root().join("library").join("previews"),
            source,
        )?
    } else {
        PathBuf::from(source)
    };
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }
    let info = crate::modules::media::probe(&path).map_err(|e| e.to_string())?;
    let clock = Arc::new(DeviceClock::new());
    let (output, mut producer) = AudioOutput::open(clock).map_err(|e| e.to_string())?;
    let rate = output.sample_rate();
    let mut reader =
        AudioClipReader::open(&path, rate, MIX_CHANNELS as u16).map_err(|e| e.to_string())?;
    let total = (info.duration.max(0) as i128 * i128::from(rate) / 1_000_000) as i64;
    let block = 1024usize;
    let mut buffer = vec![0f32; block * MIX_CHANNELS];
    let mut frame = 0i64;
    while frame < total && !stop.load(Ordering::Relaxed) {
        let frames = block.min((total - frame) as usize);
        let samples = &mut buffer[..frames * MIX_CHANNELS];
        reader
            .read(frame, frames, samples)
            .map_err(|e| e.to_string())?;
        let mut written = 0;
        while written < samples.len() && !stop.load(Ordering::Relaxed) {
            let n = producer.push(&samples[written..]);
            written += n;
            if n == 0 {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        frame += frames as i64;
    }
    while !producer.is_empty() && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(output);
    Ok(())
}
