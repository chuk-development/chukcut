//! Pulling the frame at an arbitrary instant out of a video file.
//!
//! ## Why this is not one FFmpeg call
//!
//! There is no "give me the frame at t" in libav. `av_seek_frame` lands on a
//! *keyframe*, and keyframes are seconds apart in any normally encoded file, so
//! seeking alone answers a different question than the one the timeline asks —
//! it returns the frame up to several seconds before the playhead. The only
//! correct answer is: seek to the keyframe at or before `t`, then decode
//! forward, discarding frames, until the presentation timestamp reaches `t`.
//!
//! ## Why the decoder outlives the request
//!
//! Because that forward decode is the expensive part, and its cost depends on
//! what the decoder already has. Opening the file, finding stream info,
//! initialising the codec and seeking costs tens of milliseconds; decoding the
//! *next* frame when you already hold the previous one costs a fraction of a
//! millisecond. A decoder that is reopened per request pays the former every
//! time, which is why playback built that way runs at a few frames per second.
//!
//! So this type owns its demuxer and codec context across calls, and keeps
//! three pieces of state that make sequential reads cheap and correct:
//!
//! - `position`, the timestamp of the last frame handed out. A request that
//!   sits just ahead of it decodes forward instead of seeking, which is the
//!   difference between playback and a slideshow.
//! - `pending`, the one frame we decoded past the target. Without it, a
//!   sequential read would drop every frame it overshot on, and playback would
//!   show every other frame.
//! - `last`, the frame we handed out. It is still the visible one until the
//!   playhead reaches `pending`, and it is the only copy we have — so without
//!   it a playhead nudged forward by half a frame would be answered with the
//!   overshoot, one frame early.
//!
//! ## Why a seek can need retrying
//!
//! `av_seek_frame` is only as accurate as the container's index, and MPEG-TS
//! has none — it is searched by bisecting the byte stream and lands *after*
//! the requested instant more often than not. Once the demuxer is past the
//! frame, decoding forward can only get further away. So an overshoot is not
//! believed: the seek is retried from progressively earlier, and in the last
//! resort from the start of the file, which always works.

use std::path::{Path, PathBuf};

use ffmpeg_next as ffmpeg;
use ffmpeg::software::scaling;
use ffmpeg::util::frame;

use super::probe::normalize_rotation;
use super::{ensure_initialized, micros_to_ts, ts_to_micros, MediaError, Result};
use crate::modules::project::Micros;

/// One frame, ready for the renderer or a JPEG encoder.
pub struct DecodedFrame {
    /// Tightly packed RGBA8, `width * height * 4` bytes, rotation applied.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Presentation timestamp of the frame actually returned, relative to the
    /// start of the file. It is the last frame at or before the requested time,
    /// so it is normally a little *behind* what was asked for — the caller
    /// needs this to know which frame it is looking at.
    pub pts: Micros,
}

/// How far ahead of the current position a request may be before we seek.
///
/// Below this, decoding forward is cheaper than a seek plus a keyframe decode;
/// above it, we would be decoding and throwing away more frames than the seek
/// costs.
///
/// Half a second, not two. The larger window was chosen as "roughly one GOP",
/// which is right for the cost of a *single* jump but wrong for what playback
/// actually does when it falls behind: the pacer drops late frames and moves
/// the cursor forward, and with a two-second window the decoder then honoured
/// that jump by decoding every dropped frame anyway. Dropping thirty frames
/// cost thirty decodes, so falling behind made the renderer fall further
/// behind. Measured: playback frames taking 500–1200 ms against a 33 ms budget.
///
/// At half a second a normal sequential step still decodes forward, and a
/// catch-up jump seeks instead — which is the whole point of dropping frames.
const FORWARD_DECODE_WINDOW: Micros = 500_000;

/// How far before the target a retried seek starts, quadrupling each attempt.
///
/// One second is well over a GOP on anything normally encoded, so the first
/// retry almost always succeeds and the sequence terminates at the start of
/// the file after a handful of steps.
const SEEK_BACKOFF: Micros = 1_000_000;

/// Whether a seek failed to put the requested frame within reach.
///
/// `av_seek_frame` is only as accurate as the container's index, and some
/// containers have none. MPEG-TS is searched by bisecting the byte stream and
/// routinely lands *after* the instant that was asked for; MP4 rounds the
/// request into the stream's time base and can pick the keyframe after a frame
/// that sits exactly on the boundary. Either way the demuxer is now past the
/// frame, decoding forward can only get further away, and the answer would be
/// a frame from the future — or, at the tail of a file, nothing at all.
fn overshoots(found: &Option<(Micros, frame::Video)>, target: Micros) -> bool {
    match found {
        Some((pts, _)) => *pts > target,
        None => true,
    }
}

pub struct VideoDecoder {
    path: PathBuf,
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::decoder::Video,
    /// Built on the first frame rather than at open, because a codec context is
    /// allowed to report an unknown pixel format until it has decoded
    /// something, and swscale cannot be configured for "unknown". It is rebuilt
    /// if a stream changes format mid-file, which concatenated recordings do.
    scaler: Option<scaling::Context>,
    /// Display height the caller asked for, kept so the scaler can be rebuilt.
    target_display_height: Option<u32>,
    stream_index: usize,
    time_base: ffmpeg::Rational,
    /// First timestamp of the stream, in its own time base. Frame timestamps
    /// include it and seek targets do not, so every conversion in this file
    /// goes through it to keep "zero" meaning "start of the file".
    start_ts: i64,

    /// Coded size, i.e. what swscale writes.
    src_width: u32,
    src_height: u32,
    /// Size swscale is asked for, before rotation.
    scaled_width: u32,
    scaled_height: u32,
    rotation: i32,

    /// Timestamp of the last frame handed out, or `None` before the first
    /// decode and immediately after a seek.
    position: Option<Micros>,
    /// A frame decoded past the last request, kept so a sequential read does
    /// not lose it.
    pending: Option<(Micros, frame::Video)>,
    /// The frame last handed out, kept because it is still the visible one
    /// until the playhead crosses `pending`'s timestamp. Without it, a request
    /// that moves forward by less than a frame has nothing in hand but the
    /// overshoot, and answering with that shows the picture one frame early.
    last: Option<(Micros, frame::Video)>,
    /// Whether the demuxer has run out and the decoder has been told so.
    draining: bool,
}

/// # Safety
///
/// Two separate reasons this type is not `Send` by inference, and both have to
/// hold for this impl to be sound.
///
/// **The raw pointer.** `scaling::Context` holds a `*mut SwsContext`. FFmpeg's
/// swscale and codec contexts are not safe to *use* concurrently, but they
/// carry no thread affinity, so moving one is fine as long as use stays
/// exclusive. Every method here takes `&mut self`, so the borrow checker
/// guarantees that wherever the decoder ends up.
///
/// **The non-atomic refcounts, which are the subtler half.** `ffmpeg-next`
/// 6.1 keeps `Rc` inside both halves of this struct: `format::context::Input`
/// owns an `Rc<Destructor>`, and the codec context built from that input's
/// stream parameters holds an `Rc<dyn Any>` clone of it as a keep-alive. So a
/// single `VideoDecoder` owns two handles to one non-atomic refcount.
///
/// Moving that between threads is sound here because **both handles move
/// together and none is left behind**: `open_inner` is the only constructor,
/// nothing outside this struct retains a clone, and the struct is moved whole
/// or not at all. What would be unsound is a clone staying on the original
/// thread while another is used elsewhere, because the refcount updates would
/// race.
///
/// That constraint is what makes `provider::MediaSourceProvider`'s mutex
/// load-bearing rather than merely convenient. It is not there to prevent two
/// threads decoding at once — `&mut self` already does that — it is there so
/// that access from different threads is *ordered*, which is what keeps the
/// non-atomic refcount operations from overlapping. Any future code that hands
/// a decoder to a second thread must preserve both properties: move it whole,
/// and serialise access.
unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    /// Open `path` and decode at the file's native resolution.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(path.as_ref(), None)
    }

    /// Open `path` and have swscale downscale to `target_height` display pixels
    /// on the way out.
    ///
    /// Scaling during colour conversion rather than afterwards is close to free
    /// — swscale is doing a pass over the frame either way — and it keeps a
    /// thumbnail strip from allocating a full-resolution RGBA buffer per frame.
    /// The height is a *display* height, so a rotated portrait video scaled to
    /// 80 comes out 80 tall after rotation, not 80 wide.
    pub fn open_scaled(path: impl AsRef<Path>, target_height: u32) -> Result<Self> {
        Self::open_inner(path.as_ref(), Some(target_height.max(1)))
    }

    fn open_inner(path: &Path, target_display_height: Option<u32>) -> Result<Self> {
        ensure_initialized();

        let input = ffmpeg::format::input(&path).map_err(|source| MediaError::Open {
            path: path.to_path_buf(),
            source,
        })?;

        let stream = input
            .streams()
            .best(ffmpeg::media::Type::Video)
            .ok_or_else(|| MediaError::NoVideoStream(path.to_path_buf()))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let start_ts = normalize_start(stream.start_time());
        let rotation = container_rotation(&stream);
        let parameters = stream.parameters();
        let codec_name = parameters.id().name().to_string();

        let context = ffmpeg::codec::context::Context::from_parameters(parameters).map_err(
            |source| MediaError::Decode {
                path: path.to_path_buf(),
                source,
            },
        )?;
        let decoder = context
            .decoder()
            .video()
            .map_err(|_| MediaError::NoDecoder {
                path: path.to_path_buf(),
                codec: codec_name,
            })?;

        let src_width = decoder.width().max(1);
        let src_height = decoder.height().max(1);
        let (scaled_width, scaled_height) =
            scaled_dimensions(src_width, src_height, rotation, target_display_height);

        Ok(Self {
            path: path.to_path_buf(),
            input,
            decoder,
            scaler: None,
            target_display_height,
            stream_index,
            time_base,
            start_ts,
            src_width,
            src_height,
            scaled_width,
            scaled_height,
            rotation,
            position: None,
            pending: None,
            last: None,
            draining: false,
        })
    }

    /// Coded width, before rotation.
    pub fn width(&self) -> u32 {
        self.src_width
    }

    /// Coded height, before rotation.
    pub fn height(&self) -> u32 {
        self.src_height
    }

    /// Display rotation in degrees clockwise, already applied to every frame
    /// this decoder returns.
    pub fn rotation(&self) -> i32 {
        self.rotation
    }

    /// Size of the frames this decoder hands out, with rotation applied.
    pub fn output_size(&self) -> (u32, u32) {
        rotated_size(self.scaled_width, self.scaled_height, self.rotation)
    }

    /// The frame visible at `micros`, i.e. the last frame whose presentation
    /// timestamp is at or before it.
    ///
    /// Seeks only when the request is behind the current position or far enough
    /// ahead of it to be worth a keyframe decode; a request that walks forward
    /// a frame at a time never touches the demuxer's seek path.
    pub fn seek_and_decode(&mut self, micros: Micros) -> Result<DecodedFrame> {
        let target = micros.max(0);

        let mut seeked = false;
        if self.needs_seek(target) {
            self.seek(target)?;
            seeked = true;
        }

        let mut found = self.try_decode_until(target)?;

        // Nothing came back at all: the decoder had already run to the end of
        // the file, which is where a playhead parked past the last frame leaves
        // it. Rewinding is the only way to get that frame back, and the last
        // frame is the right thing to show.
        if found.is_none() && !seeked {
            self.seek(target)?;
            found = self.try_decode_until(target)?;
        }

        // Either the seek landed *past* the frame we wanted, or it found
        // nothing at all. Both mean the same thing: the frame is behind where
        // the demuxer now is, and no amount of decoding forward will reach it.
        // Retry from progressively earlier, ending at the start of the file,
        // which always works. See `overshoots` below for why this happens.
        let mut from = target;
        let mut back = SEEK_BACKOFF;
        while from > 0 && overshoots(&found, target) {
            from = target.saturating_sub(back).max(0);
            back = back.saturating_mul(4);
            self.seek(from)?;
            found = self.try_decode_until(target)?;
        }

        let (pts, frame) = found.ok_or_else(|| MediaError::NoFrameAt {
            path: self.path.clone(),
            at: target,
        })?;

        self.position = Some(pts);
        let converted = self.convert(&frame, pts);
        // Hold on to what we handed out. It stays the visible frame until the
        // playhead reaches the next one, and it is the only copy of it we have
        // — the demuxer has already moved past.
        self.last = Some((pts, frame));
        converted
    }

    /// [`Self::decode_until`], with "there was nothing there" as a value rather
    /// than an error.
    fn try_decode_until(
        &mut self,
        target: Micros,
    ) -> Result<Option<(Micros, frame::Video)>> {
        match self.decode_until(target) {
            Ok(found) => Ok(Some(found)),
            Err(MediaError::NoFrameAt { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    fn needs_seek(&self, target: Micros) -> bool {
        match self.position {
            // Never decoded anything: the decoder is parked at the start, so
            // only a target near the start avoids a seek.
            None => target > FORWARD_DECODE_WINDOW,
            Some(position) => target < position || target - position > FORWARD_DECODE_WINDOW,
        }
    }

    fn seek(&mut self, target: Micros) -> Result<()> {
        // `Input::seek` passes -1 as the stream index, which makes the
        // timestamp `AV_TIME_BASE` units — microseconds. The range caps the
        // search at the target so we land on the keyframe at or *before* it;
        // an unbounded range is free to overshoot, and then no amount of
        // forward decoding can reach the requested frame.
        let seek_ts = target + ts_to_micros(self.start_ts, self.time_base);
        self.input
            .seek(seek_ts, ..seek_ts)
            .map_err(|source| MediaError::Decode {
                path: self.path.clone(),
                source,
            })?;

        self.decoder.flush();
        self.position = None;
        self.pending = None;
        self.last = None;
        self.draining = false;
        Ok(())
    }

    /// Decode forward until the frame covering `target` is in hand.
    fn decode_until(&mut self, target: Micros) -> Result<(Micros, frame::Video)> {
        // Start from the frame already on screen when the request has not moved
        // past it. A playhead nudged forward by half a frame is still inside
        // the same frame, and at that point the only other thing in hand is
        // `pending` — the frame we decoded *past* — so without this the answer
        // would be one frame early. It is also the cheap answer to a repeated
        // request for the same instant, which is what a paused preview does.
        let mut chosen: Option<(Micros, frame::Video)> = match self.last.take() {
            Some((pts, frame)) if pts <= target => Some((pts, frame)),
            _ => None,
        };

        loop {
            let Some((pts, decoded)) = self.next_frame()? else {
                // End of stream: the last frame we saw is the visible one.
                break;
            };

            if pts > target {
                match chosen {
                    // We overshot. Keep the overshoot for the next request
                    // rather than dropping it, and use the frame before it.
                    Some(_) => {
                        self.pending = Some((pts, decoded));
                        break;
                    }
                    // Nothing earlier exists — `target` is before the first
                    // frame of the file. That frame is the best answer.
                    None => {
                        chosen = Some((pts, decoded));
                        break;
                    }
                }
            }

            let reached = pts == target;
            chosen = Some((pts, decoded));
            if reached {
                break;
            }
        }

        chosen.ok_or(MediaError::NoFrameAt {
            path: self.path.clone(),
            at: target,
        })
    }

    /// The next frame in presentation order, or `None` at end of stream.
    fn next_frame(&mut self) -> Result<Option<(Micros, frame::Video)>> {
        if let Some(pending) = self.pending.take() {
            return Ok(Some(pending));
        }

        loop {
            let mut decoded = frame::Video::empty();
            match self.decoder.receive_frame(&mut decoded) {
                Ok(()) => {
                    let pts = self.frame_micros(&decoded);
                    return Ok(Some((pts, decoded)));
                }
                // The decoder wants more input before it can produce anything.
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {}
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(source) => {
                    return Err(MediaError::Decode {
                        path: self.path.clone(),
                        source,
                    })
                }
            }

            if self.draining {
                // Already flushed and still nothing: genuinely finished.
                return Ok(None);
            }

            match self.read_packet()? {
                Some(packet) => {
                    self.decoder
                        .send_packet(&packet)
                        .map_err(|source| MediaError::Decode {
                            path: self.path.clone(),
                            source,
                        })?;
                }
                None => {
                    // Draining matters for B-frame codecs: frames buffered for
                    // reordering only come out after the decoder is told the
                    // stream ended, and those are exactly the frames at the
                    // tail of the file the user scrubs to.
                    self.decoder
                        .send_eof()
                        .map_err(|source| MediaError::Decode {
                            path: self.path.clone(),
                            source,
                        })?;
                    self.draining = true;
                }
            }
        }
    }

    /// The next packet belonging to our stream, skipping the others.
    fn read_packet(&mut self) -> Result<Option<ffmpeg::Packet>> {
        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {
                    if packet.stream() == self.stream_index {
                        return Ok(Some(packet));
                    }
                }
                Err(ffmpeg::Error::Eof) => return Ok(None),
                Err(source) => {
                    return Err(MediaError::Decode {
                        path: self.path.clone(),
                        source,
                    })
                }
            }
        }
    }

    /// Timestamp of a decoded frame, in microseconds from the start of the file.
    ///
    /// `best_effort_timestamp` is preferred over the raw pts because it is what
    /// FFmpeg itself uses for display order: some containers leave pts unset on
    /// frames whose order can be recovered from dts and the codec delay.
    fn frame_micros(&self, decoded: &frame::Video) -> Micros {
        let ts = decoded
            .timestamp()
            .or_else(|| decoded.pts())
            .unwrap_or(self.start_ts);
        ts_to_micros(ts - self.start_ts, self.time_base)
    }

    /// Configure swscale for the frame we actually got.
    fn ensure_scaler(&mut self, decoded: &frame::Video) -> Result<()> {
        let matches = self.scaler.as_ref().is_some_and(|scaler| {
            let input = scaler.input();
            input.format == decoded.format()
                && input.width == decoded.width()
                && input.height == decoded.height()
        });
        if matches {
            return Ok(());
        }

        self.src_width = decoded.width().max(1);
        self.src_height = decoded.height().max(1);
        let (scaled_width, scaled_height) = scaled_dimensions(
            self.src_width,
            self.src_height,
            self.rotation,
            self.target_display_height,
        );
        self.scaled_width = scaled_width;
        self.scaled_height = scaled_height;

        let scaler = scaling::Context::get(
            decoded.format(),
            self.src_width,
            self.src_height,
            ffmpeg::format::Pixel::RGBA,
            scaled_width,
            scaled_height,
            // Bilinear is the right trade here: the preview is transient, and
            // the exporter composites these frames on the GPU where the
            // downstream resample dominates anything a better kernel would buy.
            scaling::Flags::BILINEAR,
        )
        .map_err(|source| MediaError::Decode {
            path: self.path.clone(),
            source,
        })?;
        self.scaler = Some(scaler);
        Ok(())
    }

    fn convert(&mut self, decoded: &frame::Video, pts: Micros) -> Result<DecodedFrame> {
        self.ensure_scaler(decoded)?;

        let path = self.path.clone();
        let width = self.scaled_width;
        let height = self.scaled_height;
        let scaler = self
            .scaler
            .as_mut()
            .expect("ensure_scaler leaves a scaler in place");

        let mut rgba = frame::Video::empty();
        scaler
            .run(decoded, &mut rgba)
            .map_err(|source| MediaError::Decode { path, source })?;

        // swscale pads each row to a stride of its choosing; callers want a
        // tight buffer, so copy row by row rather than handing out the padding.
        let stride = rgba.stride(0);
        let plane = rgba.data(0);
        let row_bytes = width as usize * 4;
        let mut tight = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let start = row * stride;
            tight.extend_from_slice(&plane[start..start + row_bytes]);
        }

        let (data, width, height) = rotate_rgba(tight, width, height, self.rotation);
        Ok(DecodedFrame {
            data,
            width,
            height,
            pts,
        })
    }
}

/// A stream whose first timestamp is unknown starts at zero.
fn normalize_start(start_time: i64) -> i64 {
    // AV_NOPTS_VALUE is i64::MIN; a negative start is meaningless for our
    // purposes either way.
    if start_time == i64::MIN || start_time < 0 {
        0
    } else {
        start_time
    }
}

fn container_rotation(stream: &ffmpeg::Stream) -> i32 {
    for side_data in stream.side_data() {
        if side_data.kind() != ffmpeg::packet::side_data::Type::DisplayMatrix {
            continue;
        }
        let bytes = side_data.data();
        if bytes.len() < 9 * std::mem::size_of::<i32>() {
            continue;
        }
        // SAFETY: a display matrix is nine `int32_t` and the buffer is at least
        // that long.
        let degrees = unsafe { ffmpeg::ffi::av_display_rotation_get(bytes.as_ptr() as *const i32) };
        return normalize_rotation(-degrees);
    }
    0
}

/// Size after applying a quarter-turn rotation.
pub(crate) fn rotated_size(width: u32, height: u32, rotation: i32) -> (u32, u32) {
    if rotation % 180 == 90 {
        (height, width)
    } else {
        (width, height)
    }
}

/// What to ask swscale for, given a desired *display* height.
///
/// The target is expressed post-rotation because that is the only size a caller
/// can reason about, so for a quarter-turned video the constraint lands on the
/// coded width instead. Dimensions are forced even: several swscale paths
/// assume even chroma dimensions and produce a green edge column otherwise.
pub(crate) fn scaled_dimensions(
    src_width: u32,
    src_height: u32,
    rotation: i32,
    target_display_height: Option<u32>,
) -> (u32, u32) {
    let Some(target) = target_display_height else {
        return (src_width, src_height);
    };

    let (_, display_height) = rotated_size(src_width, src_height, rotation);
    if display_height == 0 || target >= display_height {
        // Never upscale: a 90-pixel-tall source asked for a 200-pixel thumbnail
        // gains nothing but memory.
        return (src_width, src_height);
    }

    let factor = target as f64 / display_height as f64;
    let width = even_at_least_two((src_width as f64 * factor).round() as u32);
    let height = even_at_least_two((src_height as f64 * factor).round() as u32);
    (width, height)
}

fn even_at_least_two(value: u32) -> u32 {
    let value = value.max(2);
    value - (value % 2)
}

/// Rotate a tightly packed RGBA8 buffer by a quarter turn.
///
/// Rotation is applied here, in the decoder, rather than being passed along as
/// metadata, because every consumer downstream — preview, thumbnails, the
/// compositor — needs frames the right way up, and exactly one of them would
/// eventually forget to ask.
pub(crate) fn rotate_rgba(
    data: Vec<u8>,
    width: u32,
    height: u32,
    rotation: i32,
) -> (Vec<u8>, u32, u32) {
    if rotation == 0 || width == 0 || height == 0 {
        return (data, width, height);
    }

    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; data.len()];

    // Destination pixel (x, y) is read from a source pixel chosen so that the
    // image turns clockwise by `rotation` degrees.
    match rotation {
        90 => {
            let (dw, dh) = (h, w);
            for y in 0..dh {
                for x in 0..dw {
                    let src = ((h - 1 - x) * w + y) * 4;
                    let dst = (y * dw + x) * 4;
                    out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                }
            }
            (out, height, width)
        }
        180 => {
            for y in 0..h {
                for x in 0..w {
                    let src = ((h - 1 - y) * w + (w - 1 - x)) * 4;
                    let dst = (y * w + x) * 4;
                    out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                }
            }
            (out, width, height)
        }
        270 => {
            let (dw, dh) = (h, w);
            for y in 0..dh {
                for x in 0..dw {
                    let src = (x * w + (w - 1 - y)) * 4;
                    let dst = (y * dw + x) * 4;
                    out[dst..dst + 4].copy_from_slice(&data[src..src + 4]);
                }
            }
            (out, height, width)
        }
        _ => (data, width, height),
    }
}

/// Snap a microsecond instant to the nearest exact tick of `time_base`.
///
/// Anything that caches frames by time needs this: two playhead positions a few
/// microseconds apart resolve to the same source frame, and unless they also
/// resolve to the same key, the cache misses on every scrub and decodes a frame
/// it already has.
pub fn quantize_to_frame(micros: Micros, time_base: ffmpeg::Rational) -> Micros {
    ts_to_micros(micros_to_ts(micros, time_base), time_base)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x1 image: left pixel red, right pixel green.
    fn two_by_one() -> Vec<u8> {
        vec![255, 0, 0, 255, 0, 255, 0, 255]
    }

    fn pixel(data: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    #[test]
    fn rotation_zero_is_a_no_op() {
        let (data, w, h) = rotate_rgba(two_by_one(), 2, 1, 0);
        assert_eq!((w, h), (2, 1));
        assert_eq!(data, two_by_one());
    }

    #[test]
    fn rotating_ninety_degrees_swaps_axes_clockwise() {
        // Left-to-right becomes top-to-bottom.
        let (data, w, h) = rotate_rgba(two_by_one(), 2, 1, 90);
        assert_eq!((w, h), (1, 2));
        assert_eq!(pixel(&data, w, 0, 0), RED);
        assert_eq!(pixel(&data, w, 0, 1), GREEN);
    }

    #[test]
    fn rotating_two_seventy_degrees_swaps_axes_counter_clockwise() {
        let (data, w, h) = rotate_rgba(two_by_one(), 2, 1, 270);
        assert_eq!((w, h), (1, 2));
        assert_eq!(pixel(&data, w, 0, 0), GREEN);
        assert_eq!(pixel(&data, w, 0, 1), RED);
    }

    #[test]
    fn rotating_one_eighty_degrees_mirrors_both_axes() {
        let (data, w, h) = rotate_rgba(two_by_one(), 2, 1, 180);
        assert_eq!((w, h), (2, 1));
        assert_eq!(pixel(&data, w, 0, 0), GREEN);
        assert_eq!(pixel(&data, w, 1, 0), RED);
    }

    #[test]
    fn four_quarter_turns_return_the_original() {
        let (a, w, h) = rotate_rgba(two_by_one(), 2, 1, 90);
        let (b, w, h) = rotate_rgba(a, w, h, 90);
        let (c, w, h) = rotate_rgba(b, w, h, 90);
        let (d, w, h) = rotate_rgba(c, w, h, 90);
        assert_eq!((w, h), (2, 1));
        assert_eq!(d, two_by_one());
    }

    #[test]
    fn rotated_size_only_swaps_on_quarter_turns() {
        assert_eq!(rotated_size(1920, 1080, 0), (1920, 1080));
        assert_eq!(rotated_size(1920, 1080, 90), (1080, 1920));
        assert_eq!(rotated_size(1920, 1080, 180), (1920, 1080));
        assert_eq!(rotated_size(1920, 1080, 270), (1080, 1920));
    }

    #[test]
    fn scaling_without_a_target_keeps_the_coded_size() {
        assert_eq!(scaled_dimensions(1920, 1080, 0, None), (1920, 1080));
        assert_eq!(scaled_dimensions(1920, 1080, 90, None), (1920, 1080));
    }

    #[test]
    fn scaling_constrains_the_display_height() {
        // Upright: the target applies to the coded height directly.
        assert_eq!(scaled_dimensions(1920, 1080, 0, Some(108)), (192, 108));
        // Quarter-turned: a phone video is coded 1920x1080 and displays
        // 1080x1920, so the target constrains the coded *width*. The result
        // displays 60x108 — 108 tall, as asked.
        let coded = scaled_dimensions(1920, 1080, 90, Some(108));
        assert_eq!(coded, (108, 60));
        assert_eq!(rotated_size(coded.0, coded.1, 90), (60, 108));
    }

    #[test]
    fn scaling_never_upscales() {
        assert_eq!(scaled_dimensions(160, 90, 0, Some(400)), (160, 90));
        assert_eq!(scaled_dimensions(160, 90, 0, Some(90)), (160, 90));
    }

    #[test]
    fn scaled_dimensions_are_even() {
        let (w, h) = scaled_dimensions(1001, 667, 0, Some(101));
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        assert!(w >= 2 && h >= 2);
    }

    #[test]
    fn unknown_stream_start_reads_as_zero() {
        assert_eq!(normalize_start(i64::MIN), 0);
        assert_eq!(normalize_start(-7), 0);
        assert_eq!(normalize_start(1024), 1024);
    }
}
