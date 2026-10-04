//! `chukcut-ml-worker`: runs ONNX models for the editor, in its own process.
//!
//! Why a process of its own (`docs/research/ml-features.md` §5.1): a native
//! crash or an out-of-memory in the runtime kills this process, not the
//! editor with the user's unsaved work; the runtime's GPU libraries never load
//! into the editor; and a machine without them simply has no worker.
//!
//! It speaks the framed protocol in [`chukcut_ml_worker::protocol`] on stdin
//! and stdout and logs to stderr. One request runs at a time. A reader thread
//! takes requests off stdin as they arrive, so a `cancel` reaches the running
//! request at once instead of waiting behind it.
//!
//! ```text
//! chukcut-ml-worker [--root <dir>]
//! ```
//!
//! `--root` is the ML directory in the cache (`~/.cache/chukcut/ml`), which
//! holds `models/` and `runtime/`. The engine downloads into it; the worker
//! only reads.

use std::collections::{HashMap, HashSet};
use std::io::BufWriter;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chukcut_ml_worker::protocol::{
    read_frame, write_frame, ErrorKind, Message, Outcome, Reply, Request, RequestBody,
    PROTOCOL_VERSION,
};
use chukcut_ml_worker::registry::{self, ModelSpec, Task};
use chukcut_ml_worker::runtime::{Failure, Runtime};
use chukcut_ml_worker::{birefnet, rife, rvm, sam, vittrack, yunet};
use ort::value::Tensor;

type Out = Arc<Mutex<BufWriter<std::io::Stdout>>>;

/// VitTrack's three output maps: confidence, size, offset.
type VitMaps = (Vec<f32>, Vec<f32>, Vec<f32>);

fn send(out: &Out, id: u64, body: Reply) {
    send_with(out, id, body, &[]);
}

fn send_with(out: &Out, id: u64, body: Reply, payload: &[u8]) {
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    // A closed stdout means the editor is gone; the read loop notices too.
    let _ = write_frame(&mut *out, &Message { id, body }, payload);
}

fn default_root() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("chukcut")
        .join("ml")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = default_root();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from).unwrap_or(root),
            "--version" => {
                println!("chukcut-ml-worker {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other => eprintln!("chukcut-ml-worker: ignoring argument {other}"),
        }
    }
    let out: Out = Arc::new(Mutex::new(BufWriter::new(std::io::stdout())));
    let cancelled: Arc<Mutex<HashSet<u64>>> = Arc::default();
    let (tx, rx) = mpsc::channel::<(Request, Vec<u8>)>();

    {
        let (out, cancelled) = (out.clone(), cancelled.clone());
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                match read_frame::<Request>(&mut stdin) {
                    Ok(Some((request, payload))) => {
                        if let RequestBody::Cancel { target } = request.body {
                            cancelled
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .insert(target);
                            send(
                                &out,
                                request.id,
                                Reply::Done {
                                    outcome: Outcome::Ok,
                                },
                            );
                            continue;
                        }
                        if tx.send((request, payload)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        eprintln!("chukcut-ml-worker: unreadable request, stopping: {e}");
                        break;
                    }
                }
            }
            // Dropping `tx` ends the main loop: the editor closed our stdin.
        });
    }

    let mut worker = Worker {
        root,
        runtime: None,
        tracks: HashMap::new(),
        mattes: HashMap::new(),
        embedding: None,
        cancelled,
        reply_payload: Vec::new(),
    };
    while let Ok((request, payload)) = rx.recv() {
        let id = request.id;
        if worker.take_cancel(id) {
            send(&out, id, cancelled_reply());
            continue;
        }
        let shutdown = request.body == RequestBody::Shutdown;
        let progress = |fraction: f32, stage: &str| {
            send(
                &out,
                id,
                Reply::Progress {
                    fraction,
                    stage: stage.to_string(),
                },
            )
        };
        let reply = match worker.handle(id, request.body, &payload, &progress) {
            Ok(outcome) => Reply::Done { outcome },
            Err((kind, message)) => Reply::Error { kind, message },
        };
        // Only a `done` carries the payload a request left behind.
        let reply_payload = std::mem::take(&mut worker.reply_payload);
        match reply {
            Reply::Done { .. } => send_with(&out, id, reply, &reply_payload),
            _ => send(&out, id, reply),
        }
        worker.take_cancel(id);
        if shutdown {
            break;
        }
    }
}

fn cancelled_reply() -> Reply {
    Reply::Error {
        kind: ErrorKind::Cancelled,
        message: "cancelled".into(),
    }
}

struct Track {
    spec: &'static ModelSpec,
    template: Vec<f32>,
    /// Where the object was last seen; the next search is around it.
    last: vittrack::Rect,
    /// The last box held with confidence (`vittrack::CONFIDENT`): the size
    /// scans look for and new boxes are checked against.
    held: vittrack::Rect,
    /// The object's colours in the first frame, which a re-detection must
    /// match.
    colours: Vec<f32>,
}

/// A matting session: the recurrent state after the last frame, and the
/// frame size it belongs to.
struct MatteSession {
    size: (usize, usize),
    states: [rvm::State; 4],
}

/// The last frame's SAM embedding, so more clicks on it skip the encoder.
struct Embedding {
    model: &'static str,
    /// A hash of the frame's bytes and size.
    frame: u64,
    data: Vec<f32>,
}

/// FNV-1a over a frame: cheap next to the encoder (2 MB in ~2 ms), and a
/// collision only costs a wrong mask on a frame the user is looking at.
fn frame_hash(rgba: &[u8], w: usize, h: usize) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325 ^ ((w as u64) << 32 | h as u64);
    for chunk in rgba.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        hash ^= u64::from_le_bytes(word);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct Worker {
    root: PathBuf,
    runtime: Option<Runtime>,
    tracks: HashMap<u64, Track>,
    mattes: HashMap<u64, MatteSession>,
    embedding: Option<Embedding>,
    cancelled: Arc<Mutex<HashSet<u64>>>,
    /// What the request being answered sends back beside its header (a
    /// matte); taken and cleared when the reply goes out.
    reply_payload: Vec<u8>,
}

fn bad(message: impl Into<String>) -> Failure {
    (ErrorKind::BadRequest, message.into())
}

fn inference(e: impl std::fmt::Display) -> Failure {
    (ErrorKind::Inference, e.to_string())
}

fn millis(since: Instant) -> f32 {
    since.elapsed().as_secs_f32() * 1000.0
}

/// `payload` checked to be an RGBA8 frame of `width × height`.
fn frame(payload: &[u8], width: u32, height: u32) -> Result<(usize, usize), Failure> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || payload.len() != w * h * 4 {
        return Err(bad(format!(
            "a {width}x{height} RGBA frame is {} bytes, got {}",
            w * h * 4,
            payload.len()
        )));
    }
    Ok((w, h))
}

fn model(id: &str, task: Task) -> Result<&'static ModelSpec, Failure> {
    let spec = registry::model(id).ok_or_else(|| bad(format!("unknown model {id}")))?;
    if spec.task != task {
        return Err(bad(format!("{id} is not a {task:?} model")));
    }
    Ok(spec)
}

impl Worker {
    fn take_cancel(&self, id: u64) -> bool {
        self.cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&id)
    }

    fn is_cancelled(&self, id: u64) -> bool {
        self.cancelled
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
    }

    fn runtime(&mut self) -> Result<&mut Runtime, Failure> {
        if self.runtime.is_none() {
            self.runtime = Some(Runtime::load(&self.root)?);
        }
        Ok(self.runtime.as_mut().expect("set above"))
    }

    fn handle(
        &mut self,
        id: u64,
        body: RequestBody,
        payload: &[u8],
        progress: &dyn Fn(f32, &str),
    ) -> Result<Outcome, Failure> {
        match body {
            RequestBody::Hello => Ok(Outcome::Hello {
                protocol: PROTOCOL_VERSION,
                version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
            }),
            RequestBody::Probe => Ok(Outcome::Probe(self.runtime()?.probe.clone())),
            RequestBody::Load { model: name } => {
                let spec =
                    registry::model(&name).ok_or_else(|| bad(format!("unknown model {name}")))?;
                let started = Instant::now();
                progress(0.0, "loading the runtime");
                let runtime = self.runtime()?;
                progress(0.3, "creating the session");
                if let Some(companion) = spec.companion.and_then(registry::model) {
                    runtime.session(companion)?;
                }
                let loaded = runtime.session(spec)?;
                let provider = loaded.provider.to_string();
                Ok(Outcome::Loaded {
                    model: name,
                    provider,
                    millis: millis(started),
                })
            }
            RequestBody::DetectFaces {
                model: name,
                width,
                height,
                score_threshold,
            } => {
                let (w, h) = frame(payload, width, height)?;
                let spec = model(&name, Task::DetectFaces)?;
                let started = Instant::now();
                let (faces, provider) = self.detect(spec, payload, w, h, score_threshold)?;
                Ok(Outcome::Faces {
                    faces,
                    millis: millis(started),
                    provider,
                })
            }
            RequestBody::TrackStart {
                model: name,
                session,
                width,
                height,
                bbox,
            } => {
                let (w, h) = frame(payload, width, height)?;
                let spec = model(&name, Task::TrackBox)?;
                if !(bbox[2] >= 1.0 && bbox[3] >= 1.0) {
                    return Err(bad("the box to track is empty"));
                }
                let last = vittrack::Rect::from_f32(bbox);
                let template =
                    vittrack::crop_tensor(payload, w, h, last, 2, vittrack::TEMPLATE_SIZE);
                // Make sure the session exists now, so a missing model is
                // reported at the start rather than on the second frame.
                self.runtime()?.session(spec)?;
                self.tracks.insert(
                    session,
                    Track {
                        spec,
                        template,
                        last,
                        held: last,
                        colours: vittrack::colour_signature(payload, w, h, last),
                    },
                );
                Ok(Outcome::Ok)
            }
            RequestBody::TrackUpdate {
                session,
                width,
                height,
                redetect,
            } => {
                let (w, h) = frame(payload, width, height)?;
                let started = Instant::now();
                let (bbox, score, redetected) = self.track(session, payload, w, h, redetect)?;
                Ok(Outcome::Track {
                    bbox: bbox.map(vittrack::Rect::to_f32),
                    score,
                    millis: millis(started),
                    redetected,
                })
            }
            RequestBody::TrackEnd { session } => {
                self.tracks.remove(&session);
                Ok(Outcome::Ok)
            }
            RequestBody::Matte {
                model: name,
                session,
                width,
                height,
                allow_cpu,
            } => {
                let (w, h) = frame(payload, width, height)?;
                let spec = registry::model(&name)
                    .filter(|m| matches!(m.task, Task::Matte | Task::MatteImage))
                    .ok_or_else(|| bad(format!("{name} is not a matting model")))?;
                let started = Instant::now();
                let (alpha, provider) = if spec.task == Task::MatteImage {
                    self.matte_image(spec, payload, w, h, allow_cpu)?
                } else {
                    self.matte(spec, session, payload, w, h)?
                };
                self.reply_payload = alpha;
                Ok(Outcome::Matte {
                    width,
                    height,
                    millis: millis(started),
                    provider,
                })
            }
            RequestBody::MatteEnd { session } => {
                self.mattes.remove(&session);
                Ok(Outcome::Ok)
            }
            RequestBody::Segment {
                model: name,
                width,
                height,
                points,
                bbox,
            } => {
                let (w, h) = frame(payload, width, height)?;
                let spec = model(&name, Task::SegmentEncoder)?;
                if !points.iter().any(|p| p.keep) && bbox.is_none() {
                    return Err(bad("a selection needs a point on the object or a box"));
                }
                let clicks: Vec<sam::Click> = points
                    .iter()
                    .map(|p| sam::Click {
                        x: p.x,
                        y: p.y,
                        keep: p.keep,
                    })
                    .collect();
                let started = Instant::now();
                let (mask, score, provider) = self.segment(spec, payload, w, h, &clicks, bbox)?;
                self.reply_payload = mask;
                Ok(Outcome::Segment {
                    width,
                    height,
                    score,
                    millis: millis(started),
                    provider,
                })
            }
            RequestBody::Interpolate {
                model: name,
                width,
                height,
                phases,
            } => {
                let (w, h) = (width as usize, height as usize);
                if w == 0 || h == 0 || payload.len() != w * h * 8 {
                    return Err(bad(format!(
                        "two {width}x{height} RGBA frames are {} bytes, got {}",
                        w * h * 8,
                        payload.len()
                    )));
                }
                let spec = model(&name, Task::Interpolate)?;
                if phases.is_empty() || !phases.iter().all(|&t| rife::valid_phase(t)) {
                    return Err(bad("every phase must lie strictly between 0 and 1"));
                }
                let started = Instant::now();
                let (first, second) = payload.split_at(w * h * 4);
                let mut frames = Vec::with_capacity(phases.len() * w * h * 4);
                let mut provider = String::new();
                for (i, &phase) in phases.iter().enumerate() {
                    if self.is_cancelled(id) {
                        return Err((ErrorKind::Cancelled, "cancelled".into()));
                    }
                    let (frame, used) = self.interpolate(spec, first, second, w, h, phase)?;
                    frames.extend_from_slice(&frame);
                    provider = used;
                    progress((i + 1) as f32 / phases.len() as f32, "interpolating");
                }
                self.reply_payload = frames;
                Ok(Outcome::Interpolated {
                    width,
                    height,
                    count: phases.len() as u32,
                    millis: millis(started),
                    provider,
                })
            }
            RequestBody::Benchmark {
                model: name,
                width,
                height,
                iterations,
            } => self.benchmark(id, &name, width, height, iterations.max(1), progress),
            RequestBody::Cancel { .. } => Ok(Outcome::Ok),
            RequestBody::Shutdown => Ok(Outcome::Ok),
        }
    }

    fn detect(
        &mut self,
        spec: &'static ModelSpec,
        rgba: &[u8],
        w: usize,
        h: usize,
        threshold: f32,
    ) -> Result<(Vec<chukcut_ml_worker::protocol::Face>, String), Failure> {
        let loaded = self.runtime()?.session(spec)?;
        let (shape, data) = yunet::input(rgba, w, h);
        let (pw, ph) = (shape[3] as usize, shape[2] as usize);
        let tensor = Tensor::from_array((shape, data)).map_err(inference)?;
        let outputs = loaded
            .session
            .run(ort::inputs!["input" => tensor])
            .map_err(inference)?;
        let mut flat: HashMap<&str, &[f32]> = HashMap::new();
        for name in [
            "cls_8", "cls_16", "cls_32", "obj_8", "obj_16", "obj_32", "bbox_8", "bbox_16",
            "bbox_32", "kps_8", "kps_16", "kps_32",
        ] {
            let value = outputs
                .get(name)
                .ok_or_else(|| inference(format!("the model has no output {name}")))?;
            let (_, data) = value.try_extract_tensor::<f32>().map_err(inference)?;
            flat.insert(name, data);
        }
        let strides: [yunet::StrideOutput; 3] = std::array::from_fn(|i| {
            let s = yunet::STRIDES[i];
            yunet::StrideOutput {
                cls: flat[format!("cls_{s}").as_str()],
                obj: flat[format!("obj_{s}").as_str()],
                bbox: flat[format!("bbox_{s}").as_str()],
                kps: flat[format!("kps_{s}").as_str()],
            }
        });
        let faces = yunet::decode(&strides, pw, ph, w, h, threshold);
        Ok((faces, loaded.provider.to_string()))
    }

    /// The matte of the next frame of `session`, as alpha bytes, and the
    /// provider it ran on. The session's state moves on by one frame.
    fn matte(
        &mut self,
        spec: &'static ModelSpec,
        session: u64,
        rgba: &[u8],
        w: usize,
        h: usize,
    ) -> Result<(Vec<u8>, String), Failure> {
        let states = match self.mattes.get(&session) {
            Some(m) if m.size == (w, h) => m.states.clone(),
            _ => std::array::from_fn(|_| rvm::State::zero()),
        };
        let loaded = self.runtime()?.session(spec)?;
        let provider = loaded.provider.to_string();
        let src = Tensor::from_array(([1i64, 3, h as i64, w as i64], rvm::input(rgba, w, h)))
            .map_err(inference)?;
        let ratio =
            Tensor::from_array(([1i64], vec![rvm::downsample_ratio(w, h)])).map_err(inference)?;
        let [r1, r2, r3, r4] = states.map(|s| Tensor::from_array((s.shape, s.data)));
        let outputs = loaded
            .session
            .run(ort::inputs![
                "src" => src,
                "r1i" => r1.map_err(inference)?,
                "r2i" => r2.map_err(inference)?,
                "r3i" => r3.map_err(inference)?,
                "r4i" => r4.map_err(inference)?,
                "downsample_ratio" => ratio,
            ])
            .map_err(inference)?;
        let extract = |name: &str| -> Result<rvm::State, Failure> {
            let value = outputs
                .get(name)
                .ok_or_else(|| inference(format!("the model has no output {name}")))?;
            let (shape, data) = value.try_extract_tensor::<f32>().map_err(inference)?;
            Ok(rvm::State {
                shape: shape.to_vec(),
                data: data.to_vec(),
            })
        };
        let pha = extract("pha")?;
        if pha.data.len() != w * h {
            return Err(inference(format!(
                "the matte is {} values for a {w}x{h} frame",
                pha.data.len()
            )));
        }
        let mut next = Vec::with_capacity(4);
        for name in rvm::STATE_OUTPUTS {
            next.push(extract(name)?);
        }
        drop(outputs);
        let states: [rvm::State; 4] = next.try_into().expect("four states");
        self.mattes.insert(
            session,
            MatteSession {
                size: (w, h),
                states,
            },
        );
        Ok((rvm::alpha_bytes(&pha.data), provider))
    }

    /// A per-frame matte (BiRefNet) and the provider it ran on.
    fn matte_image(
        &mut self,
        spec: &'static ModelSpec,
        rgba: &[u8],
        w: usize,
        h: usize,
        allow_cpu: bool,
    ) -> Result<(Vec<u8>, String), Failure> {
        let loaded = self.runtime()?.session(spec)?;
        let provider = loaded.provider.to_string();
        if provider == "CPU" && !spec.cpu_ok && !allow_cpu {
            return Err((
                ErrorKind::NeedsGpu,
                format!(
                    "{} needs a GPU: on the CPU one frame takes 12–25 s and 6–11 GB of memory",
                    spec.name
                ),
            ));
        }
        let size = birefnet::SIZE as i64;
        let input = Tensor::from_array(([1i64, 3, size, size], birefnet::input(rgba, w, h)))
            .map_err(inference)?;
        let outputs = loaded
            .session
            .run(ort::inputs!["input_image" => input])
            .map_err(inference)?;
        let value = outputs
            .get("output_image")
            .ok_or_else(|| inference("the model has no output output_image"))?;
        let (_, logits) = value.try_extract_tensor::<f32>().map_err(inference)?;
        if logits.len() != birefnet::SIZE * birefnet::SIZE {
            return Err(inference(format!(
                "the matte is {} values, not {}²",
                logits.len(),
                birefnet::SIZE
            )));
        }
        Ok((birefnet::matte(logits, w, h), provider))
    }

    /// The frame at `phase` between `first` and `second` (RGBA8, `w × h`
    /// each), as RGBA8, and the provider it ran on.
    fn interpolate(
        &mut self,
        spec: &'static ModelSpec,
        first: &[u8],
        second: &[u8],
        w: usize,
        h: usize,
        phase: f32,
    ) -> Result<(Vec<u8>, String), Failure> {
        let loaded = self.runtime()?.session(spec)?;
        let provider = loaded.provider.to_string();
        let input = Tensor::from_array((
            [1i64, 6, h as i64, w as i64],
            rife::input(first, second, w, h),
        ))
        .map_err(inference)?;
        // A scalar: a 0-dimensional tensor, not a one-element vector.
        let timestep = Tensor::from_array(((), vec![phase])).map_err(inference)?;
        let outputs = loaded
            .session
            .run(ort::inputs!["input" => input, "timestep" => timestep])
            .map_err(inference)?;
        let value = outputs
            .get("output")
            .ok_or_else(|| inference("the model has no output output"))?;
        let (_, data) = value.try_extract_tensor::<f32>().map_err(inference)?;
        if data.len() != 3 * w * h {
            return Err(inference(format!(
                "the frame is {} values for a {w}x{h} picture",
                data.len()
            )));
        }
        Ok((rife::frame_bytes(data, w, h), provider))
    }

    /// The SAM mask of `clicks` (and `bbox`) in this frame: the mask bytes,
    /// the model's quality estimate and the provider. The encoder runs only
    /// when the frame differs from the last one.
    fn segment(
        &mut self,
        spec: &'static ModelSpec,
        rgba: &[u8],
        w: usize,
        h: usize,
        clicks: &[sam::Click],
        bbox: Option<[f32; 4]>,
    ) -> Result<(Vec<u8>, f32, String), Failure> {
        let decoder_id = spec
            .companion
            .ok_or_else(|| bad(format!("{} has no decoder", spec.id)))?;
        let decoder = model(decoder_id, Task::SegmentDecoder)?;
        let hash = frame_hash(rgba, w, h);
        let cached = self
            .embedding
            .as_ref()
            .is_some_and(|e| e.model == spec.id && e.frame == hash);
        if !cached {
            let loaded = self.runtime()?.session(spec)?;
            let (input, rh, rw) = sam::encoder_input(rgba, w, h);
            let input =
                Tensor::from_array(([rh as i64, rw as i64, 3], input)).map_err(inference)?;
            let outputs = loaded
                .session
                .run(ort::inputs!["input_image" => input])
                .map_err(inference)?;
            let value = outputs
                .get("image_embeddings")
                .ok_or_else(|| inference("the encoder has no output image_embeddings"))?;
            let data = value
                .try_extract_tensor::<f32>()
                .map_err(inference)?
                .1
                .to_vec();
            drop(outputs);
            self.embedding = Some(Embedding {
                model: spec.id,
                frame: hash,
                data,
            });
        }
        let embedding = self
            .embedding
            .as_ref()
            .map(|e| e.data.clone())
            .expect("set above");
        let loaded = self.runtime()?.session(decoder)?;
        let provider = loaded.provider.to_string();
        let (coords, labels, n) = sam::prompt(clicks, bbox, w, h);
        let n = n as i64;
        let outputs = loaded
            .session
            .run(ort::inputs![
                "image_embeddings" => Tensor::from_array(([1i64, 256, 64, 64], embedding)).map_err(inference)?,
                "point_coords" => Tensor::from_array(([1i64, n, 2], coords)).map_err(inference)?,
                "point_labels" => Tensor::from_array(([1i64, n], labels)).map_err(inference)?,
                "mask_input" => Tensor::from_array(([1i64, 1, 256, 256], vec![0.0f32; 256 * 256])).map_err(inference)?,
                "has_mask_input" => Tensor::from_array(([1i64], vec![0.0f32])).map_err(inference)?,
                "orig_im_size" => Tensor::from_array(([2i64], vec![h as f32, w as f32])).map_err(inference)?,
            ])
            .map_err(inference)?;
        let masks = outputs
            .get("masks")
            .ok_or_else(|| inference("the decoder has no output masks"))?;
        let (_, logits) = masks.try_extract_tensor::<f32>().map_err(inference)?;
        if logits.len() != w * h {
            return Err(inference(format!(
                "the mask is {} values for a {w}x{h} frame",
                logits.len()
            )));
        }
        let mask = sam::mask_bytes(logits);
        let score = outputs
            .get("iou_predictions")
            .and_then(|v| v.try_extract_tensor::<f32>().ok())
            .and_then(|(_, d)| d.first().copied())
            .unwrap_or(0.0);
        Ok((mask, score, provider))
    }

    /// One VitTrack run: `template` against `search`, the three output maps.
    fn vit_run(
        &mut self,
        spec: &'static ModelSpec,
        template: &[f32],
        search: Vec<f32>,
    ) -> Result<VitMaps, Failure> {
        let loaded = self.runtime()?.session(spec)?;
        let t = vittrack::TEMPLATE_SIZE as i64;
        let s = vittrack::SEARCH_SIZE as i64;
        let template =
            Tensor::from_array(([1i64, 3, t, t], template.to_vec())).map_err(inference)?;
        let search = Tensor::from_array(([1i64, 3, s, s], search)).map_err(inference)?;
        let outputs = loaded
            .session
            .run(ort::inputs!["template" => template, "search" => search])
            .map_err(inference)?;
        let get = |name: &str| -> Result<Vec<f32>, Failure> {
            let value = outputs
                .get(name)
                .ok_or_else(|| inference(format!("the model has no output {name}")))?;
            Ok(value
                .try_extract_tensor::<f32>()
                .map_err(inference)?
                .1
                .to_vec())
        };
        Ok((get("output1")?, get("output2")?, get("output3")?))
    }

    /// The tracked box in this frame: the search around the last box, then,
    /// when that finds nothing and `redetect` is set, a scan of the whole
    /// frame. Answers the box (or `None`), its score and whether the scan
    /// found it.
    fn track(
        &mut self,
        session: u64,
        rgba: &[u8],
        w: usize,
        h: usize,
        redetect: bool,
    ) -> Result<(Option<vittrack::Rect>, f32, bool), Failure> {
        let track = self
            .tracks
            .get(&session)
            .ok_or_else(|| bad(format!("no track {session}; start it first")))?;
        let (spec, last, held) = (track.spec, track.last, track.held);
        let (template, colours) = (track.template.clone(), track.colours.clone());
        let search = vittrack::crop_tensor(rgba, w, h, last, 4, vittrack::SEARCH_SIZE);
        let (conf, size, offset) = self.vit_run(spec, &template, search)?;
        let (mut next, mut score) = vittrack::update(last, held, &conf, &size, &offset);
        let mut redetected = false;
        if next.is_none() && redetect {
            let mut best: Option<(vittrack::Rect, f32)> = None;
            for window in vittrack::scan_boxes(w, h, held) {
                let search = vittrack::crop_tensor(rgba, w, h, window, 4, vittrack::SEARCH_SIZE);
                let (conf, size, offset) = self.vit_run(spec, &template, search)?;
                let (Some(hit), raw) = vittrack::scan_hit(window, &conf, &size, &offset) else {
                    continue;
                };
                if raw < vittrack::REDETECT_THRESHOLD
                    || !vittrack::plausible(held, hit)
                    || best.is_some_and(|(_, s)| raw <= s)
                {
                    continue;
                }
                let colour =
                    vittrack::colour_match(&colours, &vittrack::colour_signature(rgba, w, h, hit));
                if colour >= vittrack::MIN_COLOUR_MATCH {
                    best = Some((hit, raw));
                }
            }
            if let Some((hit, raw)) = best {
                next = Some(hit);
                score = raw;
                redetected = true;
            }
        }
        if let (Some(next), Some(track)) = (next, self.tracks.get_mut(&session)) {
            track.last = next;
            if redetected || score >= vittrack::CONFIDENT {
                track.held = next;
            }
        }
        Ok((next, score, redetected))
    }

    fn benchmark(
        &mut self,
        id: u64,
        name: &str,
        width: u32,
        height: u32,
        iterations: u32,
        progress: &dyn Fn(f32, &str),
    ) -> Result<Outcome, Failure> {
        let spec = registry::model(name).ok_or_else(|| bad(format!("unknown model {name}")))?;
        let (w, h) = (width.max(1) as usize, height.max(1) as usize);
        // A mid-grey frame with a soft gradient: real work for the network,
        // nothing that depends on a fixture.
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = (96 + (i % w) * 64 / w) as u8;
                [v, v, v, 255]
            })
            .collect();
        let provider = self.runtime()?.session(spec)?.provider.to_string();
        let session = u64::MAX;
        let run = |worker: &mut Worker| -> Result<(), Failure> {
            match spec.task {
                Task::DetectFaces => worker.detect(spec, &rgba, w, h, 0.6).map(|_| ()),
                Task::TrackBox => {
                    let bbox = vittrack::Rect {
                        x: (w / 3) as i32,
                        y: (h / 3) as i32,
                        w: (w / 6).max(8) as i32,
                        h: (h / 6).max(8) as i32,
                    };
                    let template =
                        vittrack::crop_tensor(&rgba, w, h, bbox, 2, vittrack::TEMPLATE_SIZE);
                    worker.tracks.insert(
                        session,
                        Track {
                            spec,
                            template,
                            last: bbox,
                            held: bbox,
                            colours: vittrack::colour_signature(&rgba, w, h, bbox),
                        },
                    );
                    worker.track(session, &rgba, w, h, false).map(|_| ())
                }
                Task::Matte => worker.matte(spec, session, &rgba, w, h).map(|_| ()),
                Task::MatteImage => worker.matte_image(spec, &rgba, w, h, true).map(|_| ()),
                Task::SegmentEncoder => {
                    // A new frame per run, so the encoder is timed and not
                    // the embedding cache.
                    worker.embedding = None;
                    let click = sam::Click {
                        x: (w / 2) as f32,
                        y: (h / 2) as f32,
                        keep: true,
                    };
                    worker
                        .segment(spec, &rgba, w, h, &[click], None)
                        .map(|_| ())
                }
                Task::Interpolate => {
                    // The second frame is the first moved a sixteenth of
                    // the width: real flow for the network to find.
                    let shift = (w / 16).max(1) * 4;
                    let mut second = rgba[shift..].to_vec();
                    second.extend_from_slice(&rgba[..shift]);
                    worker
                        .interpolate(spec, &rgba, &second, w, h, 0.5)
                        .map(|_| ())
                }
                Task::SegmentDecoder => Err(bad(format!(
                    "{} runs with its encoder; benchmark that",
                    spec.id
                ))),
            }
        };
        // One untimed run: the first run on a GPU provider allocates and
        // picks kernels, which is not what a frame costs afterwards.
        run(self)?;
        let mut times = Vec::with_capacity(iterations as usize);
        for i in 0..iterations {
            if self.is_cancelled(id) {
                self.tracks.remove(&session);
                self.mattes.remove(&session);
                return Err((ErrorKind::Cancelled, "cancelled".into()));
            }
            let started = Instant::now();
            run(self)?;
            times.push(millis(started));
            progress((i + 1) as f32 / iterations as f32, "running");
        }
        self.tracks.remove(&session);
        self.mattes.remove(&session);
        Ok(Outcome::Benchmark {
            provider,
            iterations,
            mean_millis: times.iter().sum::<f32>() / times.len() as f32,
            min_millis: times.iter().copied().fold(f32::MAX, f32::min),
        })
    }
}
