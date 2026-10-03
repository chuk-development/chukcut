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
use chukcut_ml_worker::{vittrack, yunet};
use ort::value::Tensor;

type Out = Arc<Mutex<BufWriter<std::io::Stdout>>>;

fn send(out: &Out, id: u64, body: Reply) {
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    // A closed stdout means the editor is gone; the read loop notices too.
    let _ = write_frame(&mut *out, &Message { id, body }, &[]);
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
        cancelled,
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
        send(&out, id, reply);
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
    last: vittrack::Rect,
}

struct Worker {
    root: PathBuf,
    runtime: Option<Runtime>,
    tracks: HashMap<u64, Track>,
    cancelled: Arc<Mutex<HashSet<u64>>>,
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
                    },
                );
                Ok(Outcome::Ok)
            }
            RequestBody::TrackUpdate {
                session,
                width,
                height,
            } => {
                let (w, h) = frame(payload, width, height)?;
                let started = Instant::now();
                let (bbox, score) = self.track(session, payload, w, h)?;
                Ok(Outcome::Track {
                    bbox: bbox.map(vittrack::Rect::to_f32),
                    score,
                    millis: millis(started),
                })
            }
            RequestBody::TrackEnd { session } => {
                self.tracks.remove(&session);
                Ok(Outcome::Ok)
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

    fn track(
        &mut self,
        session: u64,
        rgba: &[u8],
        w: usize,
        h: usize,
    ) -> Result<(Option<vittrack::Rect>, f32), Failure> {
        let track = self
            .tracks
            .get(&session)
            .ok_or_else(|| bad(format!("no track {session}; start it first")))?;
        let (spec, last, template) = (track.spec, track.last, track.template.clone());
        let search = vittrack::crop_tensor(rgba, w, h, last, 4, vittrack::SEARCH_SIZE);
        let loaded = self.runtime()?.session(spec)?;
        let t = vittrack::TEMPLATE_SIZE as i64;
        let s = vittrack::SEARCH_SIZE as i64;
        let template = Tensor::from_array(([1i64, 3, t, t], template)).map_err(inference)?;
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
        let (conf, size, offset) = (get("output1")?, get("output2")?, get("output3")?);
        drop(outputs);
        let (next, score) = vittrack::update(last, &conf, &size, &offset);
        if let (Some(next), Some(track)) = (next, self.tracks.get_mut(&session)) {
            track.last = next;
        }
        Ok((next, score))
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
                        },
                    );
                    worker.track(session, &rgba, w, h).map(|_| ())
                }
            }
        };
        // One untimed run: the first run on a GPU provider allocates and
        // picks kernels, which is not what a frame costs afterwards.
        run(self)?;
        let mut times = Vec::with_capacity(iterations as usize);
        for i in 0..iterations {
            if self.is_cancelled(id) {
                self.tracks.remove(&session);
                return Err((ErrorKind::Cancelled, "cancelled".into()));
            }
            let started = Instant::now();
            run(self)?;
            times.push(millis(started));
            progress((i + 1) as f32 / iterations as f32, "running");
        }
        self.tracks.remove(&session);
        Ok(Outcome::Benchmark {
            provider,
            iterations,
            mean_millis: times.iter().sum::<f32>() / times.len() as f32,
            min_millis: times.iter().copied().fold(f32::MAX, f32::min),
        })
    }
}
