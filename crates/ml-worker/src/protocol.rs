//! The wire protocol between the engine and `chukcut-ml-worker`.
//!
//! One message is one frame on a byte stream (the worker's stdin for
//! requests, its stdout for replies):
//!
//! ```text
//! u32 LE  header length  (bytes of JSON)
//! ...     header         (a `Request` or a `Message`, as JSON)
//! u32 LE  payload length (0 when there is no payload)
//! ...     payload        (raw bytes: an RGBA8 frame, tightly packed)
//! ```
//!
//! JSON for the header keeps the protocol readable in a log and lets either
//! side add a field without breaking the other. Pixels travel as a raw payload
//! beside it, because base64 inside JSON would cost a third more bytes and a
//! copy on both sides for every frame.
//!
//! Every request carries an `id`. The worker answers each id with zero or more
//! `progress` messages and then exactly one `done` or `error`. A `done` may
//! carry a payload too: a matte comes back as one byte of alpha per pixel. Requests run
//! one at a time in the order they arrive; a `cancel` is the exception — the
//! worker reads it as soon as it arrives and the request it names stops at
//! its next check and ends with an `error` of kind `cancelled`.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Bumped when a change would make an old engine and a new worker (or the
/// reverse) misread each other. `hello` reports it; the engine refuses a
/// worker with another number instead of failing on a strange message later.
///
/// 2: whole-frame re-detection for tracks, and mattes (the first reply with a
/// payload).
/// 3: per-frame mattes (BiRefNet), click segmentation (`segment`), the
/// `needs_gpu` error and the loaded CUDA libraries in a probe.
/// 4: frame interpolation (`interpolate`), the first request with two frames
/// in its payload and several out.
/// 5: inpainting (`inpaint`, a picture and its mask in one payload) and
/// super-resolution (`upscale`).
/// 6: audio source separation (`separate`, samples in and out) and dense
/// face landmarks (`face_landmarks`).
pub const PROTOCOL_VERSION: u32 = 6;

/// A header longer than this is a broken stream, not a message.
const MAX_HEADER: usize = 1 << 20;
/// A frame larger than this is a broken stream: 8K RGBA is 133 MB.
const MAX_PAYLOAD: usize = 256 << 20;

/// What the engine asks for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub body: RequestBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestBody {
    /// Who are you? Answered with [`Outcome::Hello`]; touches no runtime.
    Hello,
    /// Load ONNX Runtime (if it is not loaded yet) and report which execution
    /// providers work on this machine. Answered with [`Outcome::Probe`].
    Probe,
    /// Create the inference session for `model` now rather than on first use,
    /// so the first frame of a job is not the slow one. Answered with
    /// [`Outcome::Loaded`].
    Load { model: String },
    /// Faces in one RGBA8 frame (the payload). Answered with
    /// [`Outcome::Faces`]. Coordinates are in the frame's pixels.
    DetectFaces {
        model: String,
        width: u32,
        height: u32,
        /// Faces scoring below this are dropped (YuNet's default is 0.6).
        score_threshold: f32,
    },
    /// Start a single-object track: `bbox` (x, y, w, h in pixels) is the
    /// object in this frame (the payload). Answered with [`Outcome::Ok`].
    TrackStart {
        model: String,
        session: u64,
        width: u32,
        height: u32,
        bbox: [f32; 4],
    },
    /// The object's box in the next frame (the payload). Answered with
    /// [`Outcome::Track`].
    TrackUpdate {
        session: u64,
        width: u32,
        height: u32,
        /// When the search around the last box finds nothing, search the
        /// whole frame (`vittrack::scan_boxes`). Costs one model run per
        /// window, about a hundred at 640×360, so the engine asks for it on
        /// some lost frames, not all.
        #[serde(default)]
        redetect: bool,
    },
    /// Forget a track. Answered with [`Outcome::Ok`].
    TrackEnd { session: u64 },
    /// The alpha matte of the next frame (the payload) in matting session
    /// `session`. Frames of one session must be consecutive frames of one
    /// shot: the model carries state from each to the next. A new session id
    /// (or a new frame size) starts from a clean state. Answered with
    /// [`Outcome::Matte`] and the matte as the payload, `width × height`
    /// bytes.
    ///
    /// A model without state (`Task::MatteImage`) ignores the session. A
    /// model too slow for the CPU (`ModelSpec::cpu_ok` false) answers
    /// [`ErrorKind::NeedsGpu`] there unless `allow_cpu` is set.
    Matte {
        model: String,
        session: u64,
        width: u32,
        height: u32,
        #[serde(default)]
        allow_cpu: bool,
    },
    /// Forget a matting session's state. Answered with [`Outcome::Ok`].
    MatteEnd { session: u64 },
    /// The mask of the object under `points` (and inside `bbox`, x, y, w, h)
    /// in this frame (the payload), by promptable segmentation model `model`
    /// (an encoder whose companion is the decoder). Answered with
    /// [`Outcome::Segment`] and the mask as the payload, `width × height`
    /// bytes. The worker keeps the last frame's embedding, so more clicks on
    /// the same frame cost only the decoder.
    Segment {
        model: String,
        width: u32,
        height: u32,
        points: Vec<SegmentPoint>,
        #[serde(default)]
        bbox: Option<[f32; 4]>,
    },
    /// The frames between two consecutive frames, at each of `phases`
    /// (`0 < t < 1`, the share of the way from the first to the second), by
    /// interpolation model `model`. The payload is both RGBA8 frames,
    /// `width × height` each, the earlier first. Answered with
    /// [`Outcome::Interpolated`] and the frames as the payload, one RGBA8
    /// frame per phase in the order asked.
    Interpolate {
        model: String,
        width: u32,
        height: u32,
        phases: Vec<f32>,
    },
    /// Paint over the masked part of a picture with inpainting model
    /// `model`. The payload is the picture, RGBA8 `width × height`, then
    /// its mask, `width × height` bytes, nonzero where the picture is to be
    /// filled. Answered with [`Outcome::Inpainted`] and the filled picture
    /// as the payload, RGBA8 of the same size: the network's whole answer,
    /// unmasked pixels included, so the caller decides how to blend it in.
    /// The picture is usually a crop around the mask, not a whole frame.
    Inpaint {
        model: String,
        width: u32,
        height: u32,
    },
    /// The picture (the payload, RGBA8 `width × height`) made larger by
    /// super-resolution model `model`, then resized to `out_width ×
    /// out_height` (area averaging below the model's own scale). Answered
    /// with [`Outcome::Upscaled`] and the picture, RGBA8, as the payload.
    Upscale {
        model: String,
        width: u32,
        height: u32,
        out_width: u32,
        out_height: u32,
    },
    /// Run `model` `iterations` times on a synthetic input of `width` ×
    /// `height` and report the time per run. Reports progress and can be
    /// cancelled; it is how a speed claim in the docs is measured.
    Benchmark {
        model: String,
        width: u32,
        height: u32,
        iterations: u32,
    },
    /// The voice in one stretch of stereo sound, by source-separation model
    /// `model`. The payload is `frames` samples per channel at the model's
    /// rate (`registry::SEPARATION_RATE`), planar `f32` little-endian: the
    /// left channel, then the right. At most one model segment
    /// (`registry::SEPARATION_SEGMENT` frames); a shorter stretch is padded
    /// with silence. Answered with [`Outcome::Separated`] and the voice as
    /// the payload, in the same layout and length.
    Separate { model: String, frames: u32 },
    /// The faces in this frame (the payload, RGBA8) with dense landmarks:
    /// face mesh model `model` on every face YuNet finds, or in the regions
    /// `hints` (the `roi` of last frame's faces) when there are any, the way
    /// a tracker follows instead of searching. A hint whose face is gone is
    /// dropped; when none is left, YuNet searches again. At most
    /// `max_faces`, largest first. Answered with [`Outcome::FaceLandmarks`].
    FaceLandmarks {
        model: String,
        width: u32,
        height: u32,
        #[serde(default)]
        hints: Vec<[f32; 4]>,
        max_faces: u32,
    },
    /// The people in one frame (the payload, RGBA8), by person detector
    /// `model`, at least `score_threshold` sure, largest first. Answered
    /// with [`Outcome::People`].
    DetectPeople {
        model: String,
        width: u32,
        height: u32,
        score_threshold: f32,
    },
    /// The people in this frame (the payload, RGBA8) with their body
    /// keypoints: pose model `model` on every person its detector finds, or
    /// in the boxes `hints` (last frame's people's `region`) when there are
    /// any, the way a tracker follows instead of searching. A hint whose
    /// person is gone is dropped; when none is left the detector searches
    /// again. With `search`, the detector also runs when hints are left,
    /// for people they do not cover (someone who walked in). People keep
    /// the hints' order; found ones follow, largest first. At most
    /// `max_people`. Answered with [`Outcome::BodyLandmarks`].
    BodyLandmarks {
        model: String,
        width: u32,
        height: u32,
        #[serde(default)]
        hints: Vec<[f32; 4]>,
        max_people: u32,
        #[serde(default)]
        search: bool,
    },
    /// Stop request `target` at its next check.
    Cancel { target: u64 },
    /// Finish the current request and exit.
    Shutdown,
}

/// What the worker says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// The request this answers; 0 for a message no request asked for.
    pub id: u64,
    #[serde(flatten)]
    pub body: Reply,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Progress { fraction: f32, stage: String },
    Done { outcome: Outcome },
    Error { kind: ErrorKind, message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Hello {
        protocol: u32,
        version: String,
        pid: u32,
    },
    Probe(Probe),
    Loaded {
        model: String,
        /// The execution provider the session runs on, e.g. `CUDA` or `CPU`.
        provider: String,
        /// Session creation, including the first (warm-up) run.
        millis: f32,
    },
    Faces {
        faces: Vec<Face>,
        /// Inference plus pre- and post-processing, not the transfer.
        millis: f32,
        provider: String,
    },
    Track {
        /// The new box, or `None` when the score fell below the tracker's
        /// threshold and the object is taken as lost in this frame.
        bbox: Option<[f32; 4]>,
        score: f32,
        millis: f32,
        /// The box came from a whole-frame scan, not from the search around
        /// the last one.
        #[serde(default)]
        redetected: bool,
    },
    /// The payload is the matte: `width × height` bytes, 0 transparent to
    /// 255 opaque, rows top to bottom.
    Matte {
        width: u32,
        height: u32,
        millis: f32,
        provider: String,
    },
    /// The payload is the mask, as for [`Outcome::Matte`].
    Segment {
        width: u32,
        height: u32,
        /// The model's own estimate of the mask's quality (SAM's predicted
        /// IoU), about 0..1.
        score: f32,
        millis: f32,
        provider: String,
    },
    /// The payload is `count` RGBA8 frames of `width × height`, back to
    /// back, in the order of the request's phases.
    Interpolated {
        width: u32,
        height: u32,
        count: u32,
        millis: f32,
        provider: String,
    },
    /// The payload is the filled picture, RGBA8 `width × height`.
    Inpainted {
        width: u32,
        height: u32,
        millis: f32,
        provider: String,
    },
    /// The payload is the larger picture, RGBA8 `width × height`.
    Upscaled {
        width: u32,
        height: u32,
        millis: f32,
        provider: String,
    },
    Benchmark {
        provider: String,
        iterations: u32,
        /// Mean over the timed runs, after one untimed warm-up run.
        mean_millis: f32,
        min_millis: f32,
    },
    /// The payload is the voice, as [`RequestBody::Separate`]'s payload.
    Separated {
        frames: u32,
        millis: f32,
        provider: String,
    },
    FaceLandmarks {
        faces: Vec<FaceMesh>,
        millis: f32,
        provider: String,
    },
    People {
        people: Vec<Person>,
        millis: f32,
        provider: String,
    },
    BodyLandmarks {
        people: Vec<BodyPose>,
        millis: f32,
        provider: String,
    },
    Ok,
}

/// One person a detector found, in the frame's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Person {
    /// x, y, width, height.
    pub bbox: [f32; 4],
    pub score: f32,
}

/// One person's body keypoints, in the frame's pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyPose {
    /// The box the seen keypoints span: x, y, width, height.
    pub bbox: [f32; 4],
    /// The box the next frame's keypoints are looked for in; what a
    /// request's `hints` take.
    pub region: [f32; 4],
    /// The mean keypoint confidence, about 0..1.
    pub score: f32,
    /// COCO's 17 keypoints in order (nose, eyes, ears, shoulders, elbows,
    /// wrists, hips, knees, ankles; left before right): x, y in pixels, then
    /// the model's confidence, about 0..1 (below 0.3: not seen).
    pub points: Vec<[f32; 3]>,
}

/// One face's dense landmarks, in the frame's pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceMesh {
    /// The box the landmarks span, x, y, width, height.
    pub bbox: [f32; 4],
    /// The square region the next frame's landmarks are looked for in:
    /// centre x, centre y, side, rotation in radians (the eye line's angle).
    /// What a request's `hints` take.
    pub roi: [f32; 4],
    /// The face model's presence score, 0..1.
    pub score: f32,
    /// MediaPipe's canonical face mesh: 468 points, then five per iris
    /// (478). x and y in pixels; z in the same scale, towards the camera
    /// negative.
    pub points: Vec<[f32; 3]>,
}

/// The machine, as the worker sees it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    /// The ONNX Runtime library that was loaded.
    pub runtime: String,
    /// Its version string, e.g. `1.28.3`.
    pub runtime_version: String,
    /// Providers that registered, in the order the worker prefers them.
    pub providers: Vec<String>,
    /// Providers that were tried and failed, with why — "libcudnn.so.9 not
    /// found" is the common one.
    pub unavailable: Vec<(String, String)>,
    /// The CUDA runtime, cuBLAS and cuDNN files the worker has loaded, by
    /// path: whether they came from chukcut's packs or from the system.
    #[serde(default)]
    pub libraries: Vec<String>,
}

/// One click of a segmentation prompt, in the frame's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SegmentPoint {
    pub x: f32,
    pub y: f32,
    /// On the object, or a part to leave out.
    pub keep: bool,
}

/// One detected face, in the frame's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Face {
    /// x, y, width, height.
    pub bbox: [f32; 4],
    /// Right eye, left eye, nose tip, right and left mouth corner (x, y).
    pub landmarks: [[f32; 2]; 5],
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No ONNX Runtime library could be loaded. The engine falls back to its
    /// model-free path.
    RuntimeMissing,
    /// The model file is not in the cache; the engine downloads it.
    ModelMissing,
    /// The request does not make sense (unknown model, frame size mismatch).
    BadRequest,
    Cancelled,
    /// The runtime failed while running the model.
    Inference,
    /// The model only runs on a GPU provider, and none works here.
    NeedsGpu,
}

/// Write one frame. Flushes, so a message never waits in a buffer while the
/// other side waits for it.
pub fn write_frame<T: Serialize>(
    out: &mut impl Write,
    header: &T,
    payload: &[u8],
) -> io::Result<()> {
    let header = serde_json::to_vec(header).map_err(io::Error::other)?;
    out.write_all(&(header.len() as u32).to_le_bytes())?;
    out.write_all(&header)?;
    out.write_all(&(payload.len() as u32).to_le_bytes())?;
    out.write_all(payload)?;
    out.flush()
}

/// Read one frame. `Ok(None)` at a clean end of stream (the other side
/// closed between frames); an end in the middle of a frame is an error.
pub fn read_frame<T: for<'de> Deserialize<'de>>(
    input: &mut impl Read,
) -> io::Result<Option<(T, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let header_len = u32::from_le_bytes(len) as usize;
    if header_len > MAX_HEADER {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a {header_len}-byte header is not a message"),
        ));
    }
    let mut header = vec![0u8; header_len];
    input.read_exact(&mut header)?;
    input.read_exact(&mut len)?;
    let payload_len = u32::from_le_bytes(len) as usize;
    if payload_len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a {payload_len}-byte payload is not a frame"),
        ));
    }
    let mut payload = vec![0u8; payload_len];
    input.read_exact(&mut payload)?;
    let header = serde_json::from_slice(&header)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(Some((header, payload)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_and_its_frame_survive_the_round_trip() {
        let request = Request {
            id: 7,
            body: RequestBody::DetectFaces {
                model: "yunet".into(),
                width: 2,
                height: 1,
                score_threshold: 0.6,
            },
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &request, &[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        write_frame(
            &mut wire,
            &Request {
                id: 8,
                body: RequestBody::Shutdown,
            },
            &[],
        )
        .unwrap();
        let mut reader = wire.as_slice();
        let (back, payload): (Request, _) = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(back, request);
        assert_eq!(payload, [1, 2, 3, 4, 5, 6, 7, 8]);
        let (next, payload): (Request, _) = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(next.body, RequestBody::Shutdown);
        assert!(payload.is_empty());
        assert!(read_frame::<Request>(&mut reader).unwrap().is_none());
    }

    #[test]
    fn the_header_is_plain_tagged_json() {
        let message = Message {
            id: 3,
            body: Reply::Done {
                outcome: Outcome::Track {
                    bbox: None,
                    score: 0.1,
                    millis: 2.0,
                    redetected: false,
                },
            },
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("\"type\":\"done\""), "{json}");
        assert!(json.contains("\"kind\":\"track\""), "{json}");
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(back, message);
    }

    #[test]
    fn a_stream_cut_inside_a_frame_is_an_error_not_an_end() {
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Request {
                id: 1,
                body: RequestBody::Probe,
            },
            &[9; 16],
        )
        .unwrap();
        wire.truncate(wire.len() - 4);
        assert!(read_frame::<Request>(&mut wire.as_slice()).is_err());
    }

    #[test]
    fn a_garbage_length_is_refused_before_allocating() {
        let wire = u32::MAX.to_le_bytes();
        let error = read_frame::<Request>(&mut wire.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
