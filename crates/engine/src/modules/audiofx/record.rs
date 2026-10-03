//! Voiceover recording from the default input device.
//!
//! The shape follows the output side (`audio::device`): the cpal callback is
//! real time, so it only converts samples to `f32` and pushes them into a
//! lock-free ring; a writer thread drains the ring into a WAV file and keeps
//! the level meter. The writer is [`TakeWriter`], which knows nothing about
//! cpal, so the count-in, the file and the meter are tested without a
//! microphone.
//!
//! ## Count-in
//!
//! The stream opens at once — the meter moves during the count-in, so the
//! user can see the microphone is live — but the first `count_in` of input is
//! not written. The take starts when the count reaches zero, which is the
//! moment the app starts the playhead from the punch-in point, so the take
//! lines up with the picture it was spoken to.
//!
//! ## Where the file goes
//!
//! Into the project's media folder: `<project dir>/<project name> Media/`
//! next to a saved project, or `recordings/` under the app's data directory
//! for one that has never been saved. Never the cache: a recording is the
//! user's own material and must survive "clear cache".

use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, StreamConfig};
use serde::Serialize;

use crate::modules::audio::ring::{ring, RingConsumer, RingProducer};
use crate::modules::project::document::{Micros, MICROS_PER_SECOND};
use crate::modules::workspace::paths::data_root;

/// The folder a take for the project at `project_path` is saved in.
pub fn recordings_dir(project_path: Option<&Path>) -> PathBuf {
    match project_path {
        Some(path) => {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Project".into());
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join(format!("{stem} Media"))
        }
        None => data_root().join("recordings"),
    }
}

/// A new, unused file name for a take in `dir`.
pub fn take_path(dir: &Path) -> PathBuf {
    let stamp = crate::modules::cloud::provenance::now_rfc3339()
        .replace('T', " ")
        .replace(':', "-")
        .trim_end_matches('Z')
        .to_string();
    let mut path = dir.join(format!("Voiceover {stamp}.wav"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Voiceover {stamp} ({n}).wav"));
        n += 1;
    }
    path
}

/// A finished take.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Take {
    pub path: String,
    pub duration: Micros,
    pub sample_rate: u32,
    pub channels: u16,
}

/// Writes a take: drops the count-in, streams `f32` WAV, measures the level.
pub struct TakeWriter {
    file: BufWriter<std::fs::File>,
    path: PathBuf,
    rate: u32,
    channels: u16,
    /// Input frames still to be dropped before the take starts.
    count_in: u64,
    written: u64,
    bytes: Vec<u8>,
    /// The start of a frame whose other channels have not arrived yet: the
    /// ring hands out samples, not frames.
    carry: Vec<f32>,
}

impl TakeWriter {
    pub fn create(
        path: &Path,
        rate: u32,
        channels: u16,
        count_in: Duration,
    ) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let file = std::fs::File::create(path)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        let mut writer = Self {
            file: BufWriter::new(file),
            path: path.to_path_buf(),
            rate: rate.max(1),
            channels: channels.max(1),
            count_in: (count_in.as_secs_f64() * rate as f64).round() as u64,
            written: 0,
            bytes: Vec::new(),
            carry: Vec::new(),
        };
        writer.header(0)?;
        Ok(writer)
    }

    fn header(&mut self, frames: u64) -> Result<(), String> {
        let data = frames * self.channels as u64 * 4;
        if data > u32::MAX as u64 - 50 {
            return Err("the take is longer than a WAV file can hold".into());
        }
        let block = self.channels as u32 * 4;
        let mut h = Vec::with_capacity(58);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&(50 + data as u32).to_le_bytes());
        h.extend_from_slice(b"WAVEfmt ");
        h.extend_from_slice(&18u32.to_le_bytes());
        h.extend_from_slice(&3u16.to_le_bytes());
        h.extend_from_slice(&self.channels.to_le_bytes());
        h.extend_from_slice(&self.rate.to_le_bytes());
        h.extend_from_slice(&(self.rate * block).to_le_bytes());
        h.extend_from_slice(&(block as u16).to_le_bytes());
        h.extend_from_slice(&32u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(b"fact");
        h.extend_from_slice(&4u32.to_le_bytes());
        h.extend_from_slice(&(frames as u32).to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&(data as u32).to_le_bytes());
        let io = |e: std::io::Error| e.to_string();
        self.file.seek(SeekFrom::Start(0)).map_err(io)?;
        self.file.write_all(&h).map_err(io)?;
        self.file.seek(SeekFrom::End(0)).map_err(io)?;
        Ok(())
    }

    /// Whether the count-in is still running.
    pub fn counting_in(&self) -> bool {
        self.count_in > 0
    }

    /// Frames written to the take so far.
    pub fn frames(&self) -> u64 {
        self.written
    }

    /// Take interleaved samples. Returns their peak, for the meter.
    pub fn push(&mut self, samples: &[f32]) -> Result<f32, String> {
        let channels = self.channels as usize;
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let mut joined = std::mem::take(&mut self.carry);
        joined.extend_from_slice(samples);
        let whole = joined.len() / channels * channels;
        self.carry = joined.split_off(whole);
        let interleaved = &joined[..];
        let frames = (interleaved.len() / channels) as u64;
        let skip = frames.min(self.count_in);
        self.count_in -= skip;
        let rest = &interleaved[skip as usize * channels..frames as usize * channels];
        if rest.is_empty() {
            return Ok(peak);
        }
        self.bytes.clear();
        for s in rest {
            let s = if s.is_finite() { *s } else { 0.0 };
            self.bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.file
            .write_all(&self.bytes)
            .map_err(|e| format!("cannot write the take: {e}"))?;
        self.written += rest.len() as u64 / channels as u64;
        Ok(peak)
    }

    /// Close the file. A take with nothing in it is deleted and refused.
    pub fn finish(mut self) -> Result<Take, String> {
        let frames = self.written;
        self.header(frames)?;
        self.file
            .flush()
            .map_err(|e| format!("cannot finish the take: {e}"))?;
        drop(self.file);
        if frames == 0 {
            let _ = std::fs::remove_file(&self.path);
            return Err("nothing was recorded".into());
        }
        Ok(Take {
            path: self.path.to_string_lossy().into_owned(),
            duration: (frames as i128 * MICROS_PER_SECOND as i128 / self.rate as i128) as Micros,
            sample_rate: self.rate,
            channels: self.channels,
        })
    }
}

/// What the record button shows.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub counting_in: bool,
    /// Time recorded so far, after the count-in.
    pub elapsed: Micros,
    /// Recent peak, `0..=1`, decaying: the input meter.
    pub level: f32,
    pub sample_rate: u32,
    pub channels: u16,
}

struct Shared {
    stop: AtomicBool,
    counting_in: AtomicBool,
    frames: AtomicU64,
    /// Peak as `f32` bits.
    level: AtomicU32,
    lost: AtomicBool,
}

/// A recording in progress. Dropping it without [`Recorder::stop`] keeps
/// whatever was written as a valid file.
pub struct Recorder {
    stream: Option<cpal::Stream>,
    shared: Arc<Shared>,
    writer: Option<JoinHandle<Result<Take, String>>>,
    rate: u32,
    channels: u16,
    opened: std::time::Instant,
}

impl Recorder {
    /// Open the default input and start recording into `path` after
    /// `count_in`.
    pub fn start(path: &Path, count_in: Duration) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("there is no microphone or other input device")?;
        let supported = device
            .default_input_config()
            .map_err(|e| format!("the input device cannot be used: {e}"))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.config();
        let rate = config.sample_rate.max(1);
        let channels = config.channels.max(1);

        let writer = TakeWriter::create(path, rate, channels, count_in)?;
        // Two seconds of headroom: the writer thread is allowed to stall on a
        // slow disk for that long before input is lost.
        let (producer, consumer) = ring(rate as usize * channels as usize * 2);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            counting_in: AtomicBool::new(!count_in.is_zero()),
            frames: AtomicU64::new(0),
            level: AtomicU32::new(0),
            lost: AtomicBool::new(false),
        });
        let stream = match format {
            SampleFormat::F32 => input::<f32>(&device, config, producer, &shared),
            SampleFormat::I16 => input::<i16>(&device, config, producer, &shared),
            SampleFormat::I32 => input::<i32>(&device, config, producer, &shared),
            SampleFormat::U16 => input::<u16>(&device, config, producer, &shared),
            SampleFormat::F64 => input::<f64>(&device, config, producer, &shared),
            other => Err(format!("the input device only offers {other:?} samples")),
        }?;
        stream
            .play()
            .map_err(|e| format!("the input device would not start: {e}"))?;

        let thread_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("chukcut-voiceover".into())
            .spawn(move || drain(writer, consumer, thread_shared))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stream: Some(stream),
            shared,
            writer: Some(handle),
            rate,
            channels,
            opened: std::time::Instant::now(),
        })
    }

    pub fn status(&self) -> RecorderStatus {
        RecorderStatus {
            counting_in: self.shared.counting_in.load(Ordering::Relaxed),
            elapsed: (self.shared.frames.load(Ordering::Relaxed) as i128
                * MICROS_PER_SECOND as i128
                / self.rate as i128) as Micros,
            level: f32::from_bits(self.shared.level.load(Ordering::Relaxed)),
            sample_rate: self.rate,
            channels: self.channels,
        }
    }

    /// When the input opened: the count-in runs from here.
    pub fn opened(&self) -> std::time::Instant {
        self.opened
    }

    /// Whether the device went away mid-take.
    pub fn is_lost(&self) -> bool {
        self.shared.lost.load(Ordering::Relaxed)
    }

    /// Stop and close the file.
    pub fn stop(mut self) -> Result<Take, String> {
        self.finish()
    }

    fn finish(&mut self) -> Result<Take, String> {
        // Closing the stream first means nothing more arrives; the writer then
        // drains what is left in the ring.
        self.stream.take();
        self.shared.stop.store(true, Ordering::Release);
        match self.writer.take() {
            Some(handle) => handle
                .join()
                .map_err(|_| "the recording thread failed".to_string())?,
            None => Err("the recording was already stopped".into()),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.writer.is_some() {
            let _ = self.finish();
        }
    }
}

fn input<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut producer: RingProducer,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    // The one allocation; the callback converts into it in passes.
    let mut scratch = vec![0.0f32; 8_192];
    let lost = Arc::clone(shared);
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                for chunk in data.chunks(scratch.len()) {
                    for (out, sample) in scratch.iter_mut().zip(chunk) {
                        *out = sample.to_sample::<f32>();
                    }
                    // A full ring drops input rather than blocking the device.
                    producer.push(&scratch[..chunk.len()]);
                }
            },
            move |error: cpal::Error| {
                tracing::warn!(%error, "input device error");
                if !matches!(error.kind(), cpal::ErrorKind::Xrun) {
                    lost.lost.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
        .map_err(|e| format!("the input device cannot be opened: {e}"))
}

/// Input samples a device can honestly have delivered `elapsed` after it
/// opened, with half a second of slack for its buffering.
///
/// Real hardware is paced by its clock. ALSA's `null` plugin (and a few
/// virtual devices) deliver input as fast as it is read: on the test display
/// that made a six-second take 35 minutes long and skipped the count-in. So
/// input beyond wall time is dropped; on a real microphone this never bites.
fn wall_limit(elapsed: Duration, rate: u32, channels: u16) -> u64 {
    ((elapsed.as_secs_f64() + 0.5) * rate as f64) as u64 * channels as u64
}

fn drain(
    mut writer: TakeWriter,
    mut consumer: RingConsumer,
    shared: Arc<Shared>,
) -> Result<Take, String> {
    let mut buffer = vec![0.0f32; 16_384];
    let mut level = 0.0f32;
    let opened = std::time::Instant::now();
    let (rate, channels) = (writer.rate, writer.channels);
    let mut taken: u64 = 0;
    loop {
        let stopping = shared.stop.load(Ordering::Acquire);
        let n = consumer.pop(&mut buffer);
        if n > 0 {
            let limit = wall_limit(opened.elapsed(), rate, channels);
            let keep = (n as u64).min(limit.saturating_sub(taken)) as usize;
            taken += keep as u64;
            let peak = writer.push(&buffer[..keep])?;
            // A meter that falls by about 20 dB a second and jumps to peaks.
            level = peak.max(level * 0.9);
            shared.level.store(level.to_bits(), Ordering::Relaxed);
            shared
                .counting_in
                .store(writer.counting_in(), Ordering::Relaxed);
            shared.frames.store(writer.frames(), Ordering::Relaxed);
            if keep < n {
                // Ahead of the clock: wait for it rather than spin.
                std::thread::sleep(Duration::from_millis(5));
            }
        } else if stopping {
            break;
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("chukcut-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn the_count_in_is_not_in_the_take_and_the_file_decodes_to_what_went_in() {
        let path = scratch("take.wav");
        let mut writer = TakeWriter::create(&path, 48_000, 2, Duration::from_millis(500)).unwrap();
        // Half a second of "count-in noise" at 0.9, then a second of a ramp.
        writer.push(&vec![0.9f32; 48_000]).unwrap();
        assert!(!writer.counting_in());
        let ramp: Vec<f32> = (0..96_000)
            .map(|i| (i / 2) as f32 / 48_000.0 * 0.5)
            .collect();
        writer.push(&ramp).unwrap();
        let take = writer.finish().unwrap();
        assert_eq!(take.duration, MICROS_PER_SECOND);

        let mut reader =
            crate::modules::audio::decode::AudioClipReader::open(&path, 48_000, 2).unwrap();
        let mut out = vec![0.0f32; 96_000];
        use crate::modules::audio::decode::ClipReader;
        reader.read(0, 48_000, &mut out).unwrap();
        assert!(out[0].abs() < 1e-6, "the count-in was dropped");
        assert!((out[2 * 24_000] - 0.25).abs() < 1e-4);
    }

    #[test]
    fn a_count_in_that_ends_mid_chunk_keeps_the_rest_of_the_chunk() {
        let path = scratch("split.wav");
        let mut writer = TakeWriter::create(&path, 1_000, 1, Duration::from_millis(250)).unwrap();
        writer.push(&[1.0; 100]).unwrap();
        writer.push(&[2.0; 300]).unwrap();
        assert_eq!(writer.frames(), 150);
    }

    #[test]
    fn samples_split_mid_frame_stay_in_their_channels() {
        let path = scratch("frames.wav");
        let mut writer = TakeWriter::create(&path, 1_000, 2, Duration::ZERO).unwrap();
        writer.push(&[0.1, -0.1, 0.1]).unwrap();
        writer.push(&[-0.1, 0.1, -0.1]).unwrap();
        assert_eq!(writer.frames(), 3);
        writer.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let data: Vec<f32> = bytes[58..]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert_eq!(data, vec![0.1, -0.1, 0.1, -0.1, 0.1, -0.1]);
    }

    #[test]
    fn input_faster_than_the_clock_is_capped_at_wall_time() {
        let one_second = wall_limit(Duration::from_secs(1), 48_000, 2);
        assert_eq!(one_second, 144_000, "a second plus half a second of slack");
    }

    #[test]
    fn an_empty_take_is_refused_and_removed() {
        let path = scratch("empty.wav");
        let writer = TakeWriter::create(&path, 48_000, 1, Duration::ZERO).unwrap();
        assert!(writer.finish().is_err());
        assert!(!path.exists());
    }

    #[test]
    fn takes_go_next_to_a_saved_project() {
        let dir = recordings_dir(Some(Path::new("/home/me/films/Trip.chukcut")));
        assert_eq!(dir, Path::new("/home/me/films/Trip Media"));
        assert!(recordings_dir(None).ends_with("recordings"));
    }
}
