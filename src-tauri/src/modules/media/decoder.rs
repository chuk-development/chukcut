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
//!
//! ## Hardware decode, and why it changes nothing above
//!
//! With [`Acceleration::Auto`] this decoder attaches a VAAPI device and frames
//! come back as GPU surfaces instead of planes in memory. Everything in this
//! file about seeking, overshoot, `pending` and `last` is unchanged by that,
//! because none of it is about where the pixels live — `av_seek_frame` is the
//! demuxer's, and the demuxer does not know a hardware decoder is downstream.
//! That is worth stating because hardware decoders have a reputation for
//! seeking differently; the reputation is about decoders that do their own
//! demuxing, and this one does not. `tests/decode.rs` runs the entire seek
//! suite against both paths for exactly this reason.
//!
//! What *does* change is what comes out, and there are two ways to take it:
//!
//! - [`Self::seek_and_decode`] downloads the surface with
//!   `av_hwframe_transfer_data` and converts it to RGBA, so its result is
//!   byte-for-byte the same shape as the software path's. Correct, and it
//!   still pays a full-frame copy out of GPU memory.
//! - [`Self::seek_and_map`] hands back the surface's DMA-BUF descriptors
//!   instead, so the frame can become a texture without ever being in system
//!   memory. See [`super::dmabuf`].
//!
//! Hardware is not the default for every entry point. `open_scaled` asks
//! swscale to downscale during colour conversion, which is most of the point of
//! a thumbnail strip, and a hardware decode that produces a full-resolution
//! surface and then downloads it entirely to make a 80-pixel-tall picture is
//! slower than software, not faster. So a caller that wants hardware asks for
//! it.

use std::path::{Path, PathBuf};

use ffmpeg::software::scaling;
use ffmpeg::util::frame;
use ffmpeg_next as ffmpeg;

use super::dmabuf::DmabufFrame;
use super::hwdecode::{self, HwCodec, VaapiDevice};
use super::probe::normalize_rotation;
use super::{ensure_initialized, micros_to_ts, ts_to_micros, MediaError, Result};
use crate::modules::project::Micros;

/// Which decoder a caller wants behind the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Acceleration {
    /// libavcodec on the CPU. Always available, always correct, ~9 ms per
    /// 1080p frame on this machine.
    #[default]
    Software,
    /// VAAPI when this machine has probed as able to decode the file's codec,
    /// software otherwise. The failure is silent by design: a file that will
    /// not hardware-decode must still open.
    Auto,
    /// VAAPI or nothing. For tests and for a caller that would rather know.
    Vaapi,
}

impl Acceleration {
    fn wants_hardware(self) -> bool {
        matches!(self, Acceleration::Auto | Acceleration::Vaapi)
    }
}

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

    /// What the caller asked for. [`Self::acceleration`] reports what was
    /// actually obtained, which can be less.
    requested: Acceleration,
    /// The VAAPI device this decoder's codec context is attached to, held so it
    /// outlives the context. `None` on the software path.
    hardware: Option<VaapiDevice>,
    /// Whether frames are in fact coming back as GPU surfaces. Set from the
    /// first decoded frame rather than from the request, because libavcodec is
    /// entitled to fall back to software behind our back — see
    /// `hwdecode::select_vaapi` — and reporting the request would be a lie.
    hardware_frames: bool,
    /// Reusable destination for `av_hwframe_transfer_data`, so downloading a
    /// 1080p surface does not allocate three megabytes per frame.
    download: Option<frame::Video>,

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
///
/// **The VAAPI device, which adds nothing to the argument.** `VaapiDevice` is
/// already `Send + Sync` in its own right — libavutil refcounts an
/// `AVBufferRef` atomically — and it is documented there. It is named here only
/// so the next reader does not have to go and check. The *VADisplay* behind it
/// is shared across decoders and is driven under the same serialisation as
/// everything else above.
unsafe impl Send for VideoDecoder {}

impl VideoDecoder {
    /// Open `path` and decode at the file's native resolution, in software.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_inner(path.as_ref(), None, Acceleration::Software)
    }

    /// Open `path` at native resolution with a chosen decoder.
    ///
    /// [`Acceleration::Auto`] is the one to reach for in production: it uses
    /// the GPU where this machine has proved it can, and opens the file either
    /// way.
    pub fn open_with(path: impl AsRef<Path>, acceleration: Acceleration) -> Result<Self> {
        Self::open_inner(path.as_ref(), None, acceleration)
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
        Self::open_inner(path.as_ref(), Some(target_height.max(1)), Acceleration::Software)
    }

    /// [`Self::open_scaled`] with a chosen decoder.
    ///
    /// Worth knowing before choosing hardware here: the surface comes back at
    /// the file's full resolution whatever `target_height` says, so a scaled
    /// hardware decode downloads every pixel and then throws most of them
    /// away. It wins at export resolution and loses at thumbnail resolution.
    pub fn open_scaled_with(
        path: impl AsRef<Path>,
        target_height: u32,
        acceleration: Acceleration,
    ) -> Result<Self> {
        Self::open_inner(path.as_ref(), Some(target_height.max(1)), acceleration)
    }

    fn open_inner(
        path: &Path,
        target_display_height: Option<u32>,
        acceleration: Acceleration,
    ) -> Result<Self> {
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
        let codec_id = parameters.id();
        let codec_name = codec_id.name().to_string();

        let mut context = ffmpeg::codec::context::Context::from_parameters(parameters).map_err(
            |source| MediaError::Decode {
                path: path.to_path_buf(),
                source,
            },
        )?;

        // Attach the GPU *before* opening. `get_format` is consulted on the
        // first packet, but `hw_device_ctx` has to be in place by then and
        // libavcodec reads it during `avcodec_open2` to decide what to offer.
        let hardware = attach_hardware(&mut context, codec_id, acceleration, path)?;

        // Which decoder, not just which codec. On the hardware path this is
        // deliberately *not* `avcodec_find_decoder` — see
        // `hwdecode::hardware_decoder` for the AV1 case that makes the
        // difference between hardware and a very convincing impression of it.
        let decoder = match hardware
            .as_ref()
            .and_then(|_| hwdecode::hardware_decoder(codec_id))
        {
            Some(chosen) => context
                .decoder()
                .open_as(chosen)
                .and_then(|opened| opened.video()),
            None => context.decoder().video(),
        }
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
            requested: acceleration,
            hardware,
            hardware_frames: false,
            download: None,
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

    /// What this decoder is actually doing, which can be less than was asked
    /// for.
    ///
    /// Before the first frame it reports the request; after it, the truth. The
    /// two differ when libavcodec accepted the VAAPI device, failed to
    /// initialise the accelerator for this particular stream — a profile the
    /// chip does not have, a bit depth it will not do — and quietly carried on
    /// in software. Nothing else in the codebase can observe that, so this is
    /// the only place it can be reported honestly.
    pub fn acceleration(&self) -> Acceleration {
        match (self.position, self.hardware_frames) {
            (None, _) => self.requested,
            (Some(_), true) => Acceleration::Vaapi,
            (Some(_), false) => Acceleration::Software,
        }
    }

    /// Whether frames are coming back as GPU surfaces, i.e. whether
    /// [`Self::seek_and_map`] can do anything.
    pub fn is_hardware(&self) -> bool {
        self.hardware_frames
    }

    /// The render node this decoder is attached to, for a log line or a
    /// diagnostics panel. `None` on the software path.
    ///
    /// This is also what keeps the device field alive and honest: the device
    /// exists to outlive the codec context that references it, and a field
    /// nothing can observe is a field somebody eventually deletes.
    pub fn device_node(&self) -> Option<&str> {
        self.hardware.as_ref().map(VaapiDevice::node)
    }

    /// The frame visible at `micros`, i.e. the last frame whose presentation
    /// timestamp is at or before it.
    ///
    /// Seeks only when the request is behind the current position or far enough
    /// ahead of it to be worth a keyframe decode; a request that walks forward
    /// a frame at a time never touches the demuxer's seek path.
    pub fn seek_and_decode(&mut self, micros: Micros) -> Result<DecodedFrame> {
        let pts = self.locate(micros)?;
        // Take the frame back out rather than borrowing it, because converting
        // needs `&mut self` for the scaler and the download buffer. It goes
        // straight back: it is still the visible frame until the playhead
        // reaches `pending`, and it is the only copy we have.
        let (_, frame) = self.last.take().expect("locate leaves a frame in hand");
        let converted = self.convert(&frame, pts);
        self.last = Some((pts, frame));
        converted
    }

    /// The frame visible at `micros`, as DMA-BUF handles onto the GPU surface
    /// it was decoded into.
    ///
    /// Same frame [`Self::seek_and_decode`] would return, and the same seek
    /// policy reaches it; the difference is that the pixels are never copied
    /// out of GPU memory. Errors when this decoder is not producing hardware
    /// frames, because silently returning a downloaded-and-re-uploaded frame
    /// would hide the exact thing a caller reaching for this wants to know.
    ///
    /// The returned value pins the decoder's surface until it is dropped, so a
    /// caller holding one per cached texture must expect the surface pool to be
    /// that much smaller — see `hwdecode::EXTRA_HW_FRAMES`.
    pub fn seek_and_map(&mut self, micros: Micros) -> Result<MappedFrame> {
        let pts = self.locate(micros)?;
        let (_, frame) = self.last.take().expect("locate leaves a frame in hand");
        let mapped = if hwdecode::is_hardware_frame(&frame) {
            let (color_space, color_range) = self.frame_colour(&frame);
            DmabufFrame::map(&frame).map(|dmabuf| MappedFrame {
                dmabuf,
                pts,
                rotation: self.rotation,
                color_space,
                color_range,
            })
        } else {
            Err(MediaError::NoHardware(format!(
                "{} is decoding in software, so there is no GPU surface to export",
                self.path.display()
            )))
        };
        self.last = Some((pts, frame));
        mapped
    }

    /// What matrix and range this frame's chroma is expressed in.
    ///
    /// Three sources, in descending order of authority: the frame itself, the
    /// codec context (which carries what the container declared even when a
    /// particular frame does not), and finally a guess from the picture height.
    ///
    /// The guess exists because a great many files declare nothing at all, and
    /// "unspecified" is not an answer a shader can use. SD is BT.601 and HD is
    /// BT.709 is the same convention FFmpeg's own `scale` filter falls back on,
    /// so guessing this way at least agrees with the tool everyone checks
    /// against. It is logged as a guess, because a wrong matrix is a tint
    /// rather than a fault and nothing else would ever surface it.
    fn frame_colour(&self, frame: &frame::Video) -> (ffmpeg::color::Space, ffmpeg::color::Range) {
        use ffmpeg::color::{Range, Space};

        let mut space = frame.color_space();
        if matches!(space, Space::Unspecified | Space::Reserved) {
            space = self.decoder.color_space();
        }
        if matches!(space, Space::Unspecified | Space::Reserved) {
            // The height of the *coded* picture, not the scaled output: the
            // convention is about the material, not about how it is displayed.
            space = if self.src_height > 576 {
                Space::BT709
            } else {
                Space::BT470BG
            };
            tracing::debug!(
                file = %self.path.display(),
                guessed = ?space,
                "no colour matrix declared; guessing from the picture height"
            );
        }

        let mut range = frame.color_range();
        if range == Range::Unspecified {
            range = self.decoder.color_range();
        }
        if range == Range::Unspecified {
            range = Range::MPEG;
        }

        (space, range)
    }

    /// Decode until the frame covering `micros` is in `self.last`, and return
    /// its timestamp.
    ///
    /// Split out of [`Self::seek_and_decode`] so the DMA-BUF path reaches the
    /// same frame by the same route. Every retry rule this file documents lives
    /// here, and there is deliberately only one copy of it.
    fn locate(&mut self, micros: Micros) -> Result<Micros> {
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

        // The first frame settles which path we are actually on. libavcodec is
        // allowed to have dropped back to software inside `get_format` without
        // telling anyone, and this is where that becomes observable.
        self.hardware_frames = hwdecode::is_hardware_frame(&frame);

        self.position = Some(pts);
        // Hold on to what we handed out. It stays the visible frame until the
        // playhead reaches the next one, and it is the only copy of it we have
        // — the demuxer has already moved past.
        self.last = Some((pts, frame));
        Ok(pts)
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

        let mut scaler = scaling::Context::get(
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
        self.apply_colour(&mut scaler, decoded);
        self.scaler = Some(scaler);
        Ok(())
    }

    /// Tell swscale which matrix and range this file is in.
    ///
    /// **`sws_getContext` does not read the frame's colour tags and never has.**
    /// It initialises with `SWS_CS_DEFAULT`, which is BT.601, whatever the file
    /// says — so every BT.709 clip decoded through this path came out with
    /// BT.601 coefficients: reds and greens off by up to ten code values, a
    /// consistent tint rather than an obvious fault. FFmpeg's own `scale` filter
    /// sets this from the frame, which is why our output and `ffmpeg`'s did not
    /// quite agree and why the difference was small enough to look like rounding.
    ///
    /// This was found from the other side. `render/`'s shader takes the matrix
    /// from [`MappedFrame::color_space`], so a hardware-decoded frame and a
    /// software-decoded one composited to pictures 9.6 code values apart on
    /// average — and forcing the *wrong* matrix on the hardware path brought
    /// them back together, which is a diagnosis rather than a coincidence.
    ///
    /// Failure is ignored on purpose: `sws_setColorspaceDetails` returns `-1`
    /// for conversions with no YUV side, which is every RGB source, and that is
    /// not a reason to refuse a frame.
    fn apply_colour(&self, scaler: &mut scaling::Context, decoded: &frame::Video) {
        let (space, range) = self.frame_colour(decoded);
        // `sws_getCoefficients` takes an `SWS_CS_*`, and those constants are
        // numerically the `AVColorSpace` values they name — `SWS_CS_ITU709` is
        // 1 and so is `AVCOL_SPC_BT709`. It clamps anything it does not know to
        // its own default, so an odd tag cannot make this unsafe.
        let id = ffmpeg::ffi::AVColorSpace::from(space) as i32;
        let source_is_full = i32::from(range == ffmpeg::color::Range::JPEG);

        // SAFETY: `scaler` is a live `SwsContext` this call only reconfigures;
        // `sws_getCoefficients` returns a pointer into libswscale's own static
        // table, valid for the process, and the call copies from it rather than
        // retaining it. The destination is RGBA, which is full range by
        // definition, hence `1`. The last three are libswscale's spelling of
        // "no brightness, contrast or saturation adjustment".
        unsafe {
            let table = ffmpeg::ffi::sws_getCoefficients(id);
            let code = ffmpeg::ffi::sws_setColorspaceDetails(
                scaler.as_mut_ptr(),
                table,
                source_is_full,
                table,
                1,
                0,
                1 << 16,
                1 << 16,
            );
            if code < 0 {
                tracing::debug!(
                    file = %self.path.display(),
                    ?space,
                    "swscale will not take colour details for this conversion"
                );
            }
        }
    }

    /// Turn whatever the decoder produced into tightly packed RGBA.
    ///
    /// A hardware surface is downloaded first. That copy is the price of this
    /// entry point and it is why [`Self::seek_and_map`] exists — but it is also
    /// what makes the hardware path's *output* identical in shape to the
    /// software path's, which is what lets one test suite cover both.
    fn convert(&mut self, decoded: &frame::Video, pts: Micros) -> Result<DecodedFrame> {
        if hwdecode::is_hardware_frame(decoded) {
            // Take the scratch buffer out of `self` for the duration: the
            // conversion below needs `&mut self` for the scaler, which it
            // cannot have while `self.download` is borrowed. It goes back
            // whichever way the conversion ends, so a failed frame does not
            // cost the next one an allocation.
            let mut scratch = match self.download.take() {
                // Reusing the buffer only works while it is the right shape. A
                // concatenated recording that changes resolution mid-file
                // would otherwise get `EINVAL` from libavutil and read as a
                // corrupt stream.
                Some(frame)
                    if frame.width() == decoded.width() && frame.height() == decoded.height() =>
                {
                    frame
                }
                _ => frame::Video::empty(),
            };
            let downloaded = hwdecode::transfer_to_software(decoded, &mut scratch);
            let converted = downloaded.and_then(|()| self.convert_software(&scratch, pts));
            self.download = Some(scratch);
            return converted;
        }
        self.convert_software(decoded, pts)
    }

    fn convert_software(&mut self, decoded: &frame::Video, pts: Micros) -> Result<DecodedFrame> {
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

/// A decoded frame that is still on the GPU.
///
/// The counterpart of [`DecodedFrame`], for the path that does not copy. Note
/// what is *not* here: rotation has not been applied, because rotating means
/// touching pixels and the entire point is not to. The caller gets the angle
/// and applies it where it is free — in the compositor's transform, which is
/// already doing a matrix multiply per vertex.
pub struct MappedFrame {
    pub dmabuf: DmabufFrame,
    /// Presentation timestamp of the frame actually returned, exactly as
    /// [`DecodedFrame::pts`].
    pub pts: Micros,
    /// Display rotation in degrees clockwise, **not** applied.
    pub rotation: i32,
    /// The matrix this surface's chroma is expressed in, read from the file.
    ///
    /// It travels with the frame because the compositor is what converts, and
    /// it is read rather than assumed because a 1080p clip is *usually* BT.709
    /// and an SD one *usually* BT.601 — and "usually" ships a saturation error
    /// that nobody catches until delivery. See
    /// [`VideoDecoder::frame_colour`] for where the answer comes from when the
    /// file declares nothing.
    pub color_space: ffmpeg::color::Space,
    /// Whether luma runs 16..235 or 0..255.
    pub color_range: ffmpeg::color::Range,
}

impl MappedFrame {
    /// Size of the picture as decoded, before rotation.
    pub fn coded_size(&self) -> (u32, u32) {
        self.dmabuf.size()
    }

    /// Size the picture should be shown at, i.e. with rotation applied.
    pub fn display_size(&self) -> (u32, u32) {
        let (w, h) = self.dmabuf.size();
        rotated_size(w, h, self.rotation)
    }
}

impl std::fmt::Debug for MappedFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappedFrame")
            .field("pts", &self.pts)
            .field("rotation", &self.rotation)
            .field("color_space", &self.color_space)
            .field("color_range", &self.color_range)
            .field("dmabuf", &self.dmabuf)
            .finish()
    }
}

/// Point a codec context at the GPU, if that is what the caller wants and this
/// machine can do it.
///
/// Returns the device to be held for the decoder's lifetime, or `None` for the
/// software path. Only [`Acceleration::Vaapi`] turns "no hardware" into an
/// error; `Auto` logs the reason once per file at debug and carries on, because
/// **a file that will not hardware-decode must still open** and the alternative
/// is an import that fails on a machine where nothing is wrong.
fn attach_hardware(
    context: &mut ffmpeg::codec::context::Context,
    codec_id: ffmpeg::codec::Id,
    acceleration: Acceleration,
    path: &Path,
) -> Result<Option<VaapiDevice>> {
    if !acceleration.wants_hardware() {
        return Ok(None);
    }

    let refuse = |reason: String| -> Result<Option<VaapiDevice>> {
        if acceleration == Acceleration::Vaapi {
            Err(MediaError::NoHardware(reason))
        } else {
            tracing::debug!(file = %path.display(), reason, "decoding in software");
            Ok(None)
        }
    };

    let Some(codec) = HwCodec::from_id(codec_id) else {
        return refuse(format!(
            "{} is not one of the codecs this build hardware-decodes",
            codec_id.name()
        ));
    };
    // The probe, not the FFmpeg build. `docs/STATUS.md` records why: a codec
    // being present says nothing about whether the driver can drive it, in
    // either direction.
    if !hwdecode::supports(codec) {
        return refuse(format!(
            "this machine does not hardware-decode {}",
            codec.label()
        ));
    }
    let Some(device) = crate::modules::gpu::vaapi_device() else {
        return refuse("no VAAPI device on this machine".into());
    };

    match device.attach_to_decoder(context) {
        Ok(()) => Ok(Some(device)),
        Err(error) => refuse(error.to_string()),
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
