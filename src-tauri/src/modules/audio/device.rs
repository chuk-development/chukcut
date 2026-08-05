//! The output device.
//!
//! One stream on the default output, and the callback that feeds it. Almost
//! everything here is about the cases that are not the happy one, because the
//! happy one is four lines:
//!
//! - **There is no device.** A machine with no sound card, a container, a CI
//!   runner, a user who unplugged their interface. The editor must still open
//!   and still play — silently, on wall time. Audio is a feature, not a
//!   dependency.
//! - **The device leaves mid-playback.** A Bluetooth headset walking out of
//!   range takes the stream with it. The error arrives on cpal's error
//!   callback, the clock stops being the master, and the engine tries again
//!   later.
//! - **The device is not 48 kHz.** Plenty of hardware is 44.1, and some is 96.
//!   Nothing downstream may assume otherwise: the rate the device chose is the
//!   rate the mixer and the decoders run at, so the conversion happens once,
//!   inside swresample, rather than being bolted on afterwards.
//! - **The device is not stereo `f32`.** The mix bus is stereo `f32` because
//!   that is what arithmetic is comfortable in; the device gets whatever it
//!   asked for, converted in the callback, which is a multiply and a cast.
//!
//! ## What the callback may do
//!
//! Copy, convert, and add to two atomics. It may not allocate, lock anything
//! another thread holds for long, or touch the file system. Every buffer it
//! uses is allocated when the stream is built. This is not a style preference:
//! the callback has a hard deadline of one buffer period, and missing it is
//! not a slow frame, it is a click.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

use super::clock::DeviceClock;
use super::ring::{ring, RingConsumer, RingProducer};
use super::{AudioError, Result};

/// The rate we ask for when the device will give it to us. Everything the
/// editor decodes is 48 kHz or converts to it cheaply, and it is what the
/// exporter writes.
const PREFERRED_RATE: u32 = 48_000;

/// Frames per callback we ask for. Small enough that a seek is not audible as
/// a delay, large enough not to spend the machine on callback overhead. The
/// device is free to ignore it.
const PREFERRED_BUFFER: u32 = 512;

/// How much audio the ring holds, as a multiple of the callback buffer.
///
/// Deep enough that a mixer thread descheduled for a few milliseconds — which
/// it will be, it shares a machine with a video decoder — does not starve the
/// callback; shallow enough that the audio queued ahead of a seek is a small
/// number of milliseconds, because that queue is the one thing a flush cannot
/// take back once the device has it.
const RING_BLOCKS: usize = 8;

/// The largest block the callback converts in one pass. Bounds the scratch
/// buffer allocated at build time; a device asking for more is handled in
/// several passes rather than by allocating on the real-time thread.
const MAX_CALLBACK_BLOCK: usize = 8_192;

/// What the device settled on.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: String,
    /// Frames per callback, if the device would say.
    pub buffer_frames: Option<u32>,
}

/// An open output stream.
///
/// Dropping it closes the device. The stream is kept running even when nothing
/// is playing, because starting one costs tens of milliseconds and often a
/// click, and a running stream that is fed nothing outputs silence for free.
pub struct AudioOutput {
    stream: cpal::Stream,
    info: DeviceInfo,
    /// Set from cpal's error callback when the device goes away.
    lost: Arc<AtomicBool>,
}

impl std::fmt::Debug for AudioOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioOutput")
            .field("info", &self.info)
            .field("lost", &self.is_lost())
            .finish()
    }
}

impl AudioOutput {
    /// Open the default output device and start its stream.
    ///
    /// Returns the stream and the producing end of the ring that feeds it. The
    /// caller owns the producer and is the only thread allowed to touch it.
    pub fn open(clock: Arc<DeviceClock>) -> Result<(Self, RingProducer)> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(AudioError::NoDevice)?;
        let name = device
            .description()
            .map(|description| description.name().to_string())
            .unwrap_or_else(|_| "default output".into());

        let chosen = choose_config(&device)?;
        let sample_format = chosen.sample_format();
        let mut config: StreamConfig = chosen.config();
        config.buffer_size = preferred_buffer_size(chosen.buffer_size());

        let channels = config.channels.max(1) as usize;
        let rate = config.sample_rate.max(1);
        let block = match config.buffer_size {
            cpal::BufferSize::Fixed(frames) => frames as usize,
            cpal::BufferSize::Default => PREFERRED_BUFFER as usize,
        };
        let (producer, consumer) = ring((block * channels * RING_BLOCKS).max(channels * 1_024));

        let lost = Arc::new(AtomicBool::new(false));
        let stream = build_stream(
            &device,
            config,
            sample_format,
            consumer,
            Arc::clone(&clock),
            Arc::clone(&lost),
        )?;

        // 0.18 no longer starts ALSA streams implicitly.
        stream.play().map_err(AudioError::Device)?;
        clock.attach(rate);

        let buffer_frames = stream.buffer_size().ok().or(match config.buffer_size {
            cpal::BufferSize::Fixed(frames) => Some(frames),
            cpal::BufferSize::Default => None,
        });

        let info = DeviceInfo {
            name,
            sample_rate: rate,
            channels: config.channels,
            sample_format: format!("{sample_format:?}"),
            buffer_frames,
        };
        tracing::info!(
            device = %info.name,
            rate = info.sample_rate,
            channels = info.channels,
            format = %info.sample_format,
            "audio output open"
        );

        Ok((Self { stream, info, lost }, producer))
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn sample_rate(&self) -> u32 {
        self.info.sample_rate
    }

    pub fn channels(&self) -> usize {
        self.info.channels.max(1) as usize
    }

    /// Whether the device has reported an error it cannot come back from.
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }

    /// Stop the stream without closing the device.
    pub fn pause(&self) {
        if let Err(error) = self.stream.pause() {
            tracing::debug!(%error, "audio stream would not pause");
        }
    }

    pub fn play(&self) {
        if let Err(error) = self.stream.play() {
            tracing::debug!(%error, "audio stream would not start");
        }
    }
}

/// Pick a configuration, preferring the one the rest of the module is happiest
/// with and accepting whatever the device actually has.
///
/// The order of preference is float samples first (no conversion), then
/// stereo (no channel mapping), then 48 kHz (no resampling). A device that
/// offers none of those still works; it just does more arithmetic.
fn choose_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let default = device.default_output_config().map_err(AudioError::Device)?;

    let supported = match device.supported_output_configs() {
        Ok(configs) => configs.collect::<Vec<_>>(),
        Err(error) => {
            tracing::debug!(%error, "device would not enumerate its configurations");
            return Ok(default);
        }
    };

    let best = supported
        .into_iter()
        .filter_map(|range| {
            let rate = PREFERRED_RATE.clamp(range.min_sample_rate(), range.max_sample_rate());
            let score = (range.sample_format() == SampleFormat::F32) as u32 * 4
                + (range.channels() == 2) as u32 * 2
                + range.contains_rate(PREFERRED_RATE) as u32;
            range
                .try_with_sample_rate(rate)
                .map(|config| (score, config))
        })
        .max_by_key(|(score, _)| *score);

    match best {
        Some((_, config)) => Ok(config),
        None => Ok(default),
    }
}

/// Ask for our buffer size when the device's range allows it.
fn preferred_buffer_size(supported: &cpal::SupportedBufferSize) -> cpal::BufferSize {
    match supported {
        cpal::SupportedBufferSize::Range { min, max } => {
            cpal::BufferSize::Fixed(PREFERRED_BUFFER.clamp(*min, *max))
        }
        // Some backends will not say, and asking for a size they cannot honour
        // fails the whole stream. Their default is better than no audio.
        cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
    }
}

fn build_stream(
    device: &cpal::Device,
    config: StreamConfig,
    format: SampleFormat,
    consumer: RingConsumer,
    clock: Arc<DeviceClock>,
    lost: Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    match format {
        SampleFormat::F32 => run::<f32>(device, config, consumer, clock, lost),
        SampleFormat::F64 => run::<f64>(device, config, consumer, clock, lost),
        SampleFormat::I16 => run::<i16>(device, config, consumer, clock, lost),
        SampleFormat::I32 => run::<i32>(device, config, consumer, clock, lost),
        SampleFormat::U16 => run::<u16>(device, config, consumer, clock, lost),
        SampleFormat::U8 => run::<u8>(device, config, consumer, clock, lost),
        other => Err(AudioError::Unsupported(format!(
            "the audio device only offers {other:?} samples"
        ))),
    }
}

fn run<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut consumer: RingConsumer,
    clock: Arc<DeviceClock>,
    lost: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let block = match config.buffer_size {
        cpal::BufferSize::Fixed(frames) => frames as usize * channels,
        cpal::BufferSize::Default => PREFERRED_BUFFER as usize * channels,
    };
    // The one allocation. Everything the callback does happens in here.
    let mut scratch = vec![0.0f32; block.clamp(channels, MAX_CALLBACK_BLOCK)];

    let error_lost = Arc::clone(&lost);
    let error_clock = Arc::clone(&clock);

    let stream = device
        .build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                // How far ahead of audibility this callback is running. This
                // is the number that keeps the picture from leading the sound.
                let timestamp = info.timestamp();
                let latency = timestamp
                    .playback
                    .saturating_duration_since(timestamp.callback)
                    .as_micros() as i64;

                let mut written = 0;
                while written < data.len() {
                    let take = (data.len() - written).min(scratch.len());
                    let filled = consumer.pop(&mut scratch[..take]);
                    for index in 0..take {
                        // Past `filled` the ring had nothing: an underrun is
                        // silence, never stale samples and never a wait.
                        let sample = if index < filled { scratch[index] } else { 0.0 };
                        data[written + index] = T::from_sample(sample);
                    }
                    written += take;
                }

                // Silence counts as played: the device really did consume that
                // time, and a clock that stopped during an underrun would
                // freeze the picture too.
                clock.advance((data.len() / channels) as u64, latency);
            },
            move |error: cpal::Error| {
                use cpal::ErrorKind::*;
                match error.kind() {
                    // Recoverable: the stream keeps running, the user hears a
                    // moment of trouble, nothing else changes.
                    Xrun => tracing::debug!(%error, "audio buffer underrun"),
                    // Not recoverable. The device is gone and the clock must
                    // stop claiming to be the master.
                    DeviceNotAvailable | DeviceChanged | StreamInvalidated => {
                        tracing::warn!(%error, "audio device lost");
                        error_lost.store(true, Ordering::Relaxed);
                        error_clock.detach();
                    }
                    _ => tracing::warn!(%error, "audio stream error"),
                }
            },
            None,
        )
        .map_err(AudioError::Device)?;

    Ok(stream)
}

/// Interleave a stereo mix into `channels` device channels.
///
/// The mix bus is stereo because that is what the document describes; a device
/// may be mono, or a 5.1 card with four channels nobody is using. This runs on
/// the mixer thread, not in the callback, so the callback stays a copy.
///
/// Mono is the average of the two rather than the left channel, because
/// dropping a channel silences anything panned hard the other way. Extra
/// channels are left silent: putting the mix into a surround field is a
/// decision the project should make, not one to guess at here.
pub fn map_channels(stereo: &[f32], channels: usize, out: &mut Vec<f32>) {
    let channels = channels.max(1);
    let frames = stereo.len() / super::mixer::MIX_CHANNELS;
    out.clear();
    out.resize(frames * channels, 0.0);
    if channels == super::mixer::MIX_CHANNELS {
        out.copy_from_slice(&stereo[..frames * channels]);
        return;
    }
    for frame in 0..frames {
        let left = stereo[frame * super::mixer::MIX_CHANNELS];
        let right = stereo[frame * super::mixer::MIX_CHANNELS + 1];
        match channels {
            1 => out[frame] = (left + right) * 0.5,
            _ => {
                out[frame * channels] = left;
                out[frame * channels + 1] = right;
            }
        }
    }
}

/// Whether the machine has an output device at all.
///
/// Used to skip the tests that need one, and to tell the user why they cannot
/// hear anything.
pub fn has_output_device() -> bool {
    cpal::default_host().default_output_device().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Device tests are opt-in. They open the machine's real sound card, which
    /// is not something a test run should do behind the user's back, and there
    /// is nothing to open on a CI runner at all.
    fn device_tests_enabled() -> bool {
        std::env::var("CHUKCUT_AUDIO_DEVICE_TESTS").is_ok() && has_output_device()
    }

    #[test]
    fn stereo_passes_through_unchanged() {
        let mut out = Vec::new();
        map_channels(&[0.25, -0.5, 1.0, 0.0], 2, &mut out);
        assert_eq!(out, vec![0.25, -0.5, 1.0, 0.0]);
    }

    #[test]
    fn mono_averages_rather_than_dropping_a_channel() {
        let mut out = Vec::new();
        // A sound panned hard left must not vanish on a mono device.
        map_channels(&[1.0, 0.0, 0.0, 1.0], 1, &mut out);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn a_surround_device_gets_the_mix_in_its_front_pair() {
        let mut out = Vec::new();
        map_channels(&[1.0, -1.0], 6, &mut out);
        assert_eq!(out, vec![1.0, -1.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn mapping_reuses_the_buffer_it_is_given() {
        let mut out = vec![9.0; 64];
        map_channels(&[0.5, 0.5], 2, &mut out);
        assert_eq!(out, vec![0.5, 0.5], "left-over samples would be heard");
    }

    #[test]
    fn a_buffer_size_the_device_will_not_describe_falls_back_to_its_default() {
        assert!(matches!(
            preferred_buffer_size(&cpal::SupportedBufferSize::Unknown),
            cpal::BufferSize::Default
        ));
        assert!(matches!(
            preferred_buffer_size(&cpal::SupportedBufferSize::Range { min: 64, max: 4096 }),
            cpal::BufferSize::Fixed(512)
        ));
        // A device that cannot go as low as we would like gets what it can.
        assert!(matches!(
            preferred_buffer_size(&cpal::SupportedBufferSize::Range {
                min: 1024,
                max: 4096
            }),
            cpal::BufferSize::Fixed(1024)
        ));
    }

    #[test]
    fn opening_the_real_device_produces_a_running_stream() {
        if !device_tests_enabled() {
            eprintln!("skipped: set CHUKCUT_AUDIO_DEVICE_TESTS=1 with an output device present");
            return;
        }
        let clock = Arc::new(DeviceClock::new());
        let (output, mut producer) = AudioOutput::open(Arc::clone(&clock)).expect("open device");
        assert!(output.sample_rate() >= 8_000);
        assert!(output.channels() >= 1);
        assert!(clock.is_active());

        // Feed it silence and check the clock moves with the device.
        let block = vec![0.0f32; output.channels() * 4_096];
        producer.push(&block);
        std::thread::sleep(std::time::Duration::from_millis(120));
        assert!(clock.frames() > 0, "the callback never ran");
    }
}
