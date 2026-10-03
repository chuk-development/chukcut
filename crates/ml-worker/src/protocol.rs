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
//! `progress` messages and then exactly one `done` or `error`. Requests run
//! one at a time in the order they arrive; a `cancel` is the exception — the
//! worker reads it as soon as it arrives and the request it names stops at
//! its next check and ends with an `error` of kind `cancelled`.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Bumped when a change would make an old engine and a new worker (or the
/// reverse) misread each other. `hello` reports it; the engine refuses a
/// worker with another number instead of failing on a strange message later.
pub const PROTOCOL_VERSION: u32 = 1;

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
    },
    /// Forget a track. Answered with [`Outcome::Ok`].
    TrackEnd { session: u64 },
    /// Run `model` `iterations` times on a synthetic input of `width` ×
    /// `height` and report the time per run. Reports progress and can be
    /// cancelled; it is how a speed claim in the docs is measured.
    Benchmark {
        model: String,
        width: u32,
        height: u32,
        iterations: u32,
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
    },
    Benchmark {
        provider: String,
        iterations: u32,
        /// Mean over the timed runs, after one untimed warm-up run.
        mean_millis: f32,
        min_millis: f32,
    },
    Ok,
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
