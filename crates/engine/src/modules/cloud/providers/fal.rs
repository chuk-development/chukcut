//! fal.ai: one key, one queue API, many models.
//!
//! chukcut uses it to process the user's clips: the curated actions in
//! `fal_actions.toml`. A run is: price the endpoint (fal's pricing API) so the
//! user sees the cost before anything is spent, upload the clip to fal's
//! storage, submit to the queue, poll, download the result next to an
//! `asset.json`. Three hosts are involved — the queue, the platform API
//! (pricing) and the REST API (storage) — and an account pointed anywhere but
//! the default sends all three to its own base URL, which is how the tests
//! run it against a mock.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::super::http::{self, SendJson as _};
use super::super::jobs::{self, JobEvent, JobStatus, PollPolicy, QueueApi, Submitted};
use super::super::provenance::{self, Commercial, Cost, Licence, Origin, OriginKind};
use super::super::registry::{descriptor, Account};
use super::super::TestReport;

/// One curated action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FalAction {
    pub id: String,
    pub label: String,
    pub description: String,
    pub endpoint: String,
    /// The result file's extension.
    pub extension: String,
    /// `beside` or `replace`.
    pub result: String,
    /// The model's input, without `video_url`.
    #[serde(default)]
    pub input: toml::Table,
}

#[derive(Deserialize)]
struct ActionFile {
    action: Vec<FalAction>,
}

/// The actions, parsed once from the table compiled into the binary.
pub fn actions() -> &'static [FalAction] {
    static ACTIONS: OnceLock<Vec<FalAction>> = OnceLock::new();
    ACTIONS.get_or_init(|| {
        toml::from_str::<ActionFile>(include_str!("fal_actions.toml"))
            .map(|f| f.action)
            .unwrap_or_else(|error| {
                tracing::error!(%error, "fal_actions.toml does not parse");
                Vec::new()
            })
    })
}

pub fn action(id: &str) -> Option<&'static FalAction> {
    actions().iter().find(|a| a.id == id)
}

/// A model's price as fal states it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub unit_price: f64,
    pub unit: String,
    pub currency: String,
}

/// What the clip being processed is, for the estimate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaFacts {
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub bytes: u64,
}

/// The cost shown before a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    pub price: Price,
    /// How many units this clip is.
    pub quantity: f64,
    pub amount: f64,
    /// "$0.42 (12.0 s × $0.035 per second)" for the confirmation.
    pub summary: String,
    /// What leaves the machine: "Uploads 84 MB of your clip to fal.ai".
    pub upload: String,
}

impl Estimate {
    pub fn new(price: Price, media: &MediaFacts) -> Self {
        let unit = price.unit.to_ascii_lowercase();
        let frames = media.duration_seconds * media.fps.max(1.0);
        let megapixels = f64::from(media.width) * f64::from(media.height) / 1_000_000.0;
        let quantity = if unit.contains("second") {
            media.duration_seconds
        } else if unit.contains("minute") {
            media.duration_seconds / 60.0
        } else if unit.contains("frame") {
            frames
        } else if unit.contains("megapixel") {
            // Per megapixel of video means every frame's pixels.
            megapixels * frames
        } else {
            // "video", "request", "image", "unit": one per run.
            1.0
        };
        let amount = (price.unit_price * quantity * 10_000.0).round() / 10_000.0;
        let symbol = if price.currency.eq_ignore_ascii_case("USD") {
            "$"
        } else {
            ""
        };
        let summary = if (quantity - 1.0).abs() < f64::EPSILON {
            format!("{symbol}{amount:.2} per run")
        } else {
            format!(
                "{symbol}{amount:.2} ({quantity:.1} {} × {symbol}{} per {})",
                plural(&unit),
                trim_price(price.unit_price),
                unit
            )
        };
        let upload = format!("Uploads {} of your clip to fal.ai", megabytes(media.bytes));
        Self {
            price,
            quantity,
            amount,
            summary,
            upload,
        }
    }
}

fn plural(unit: &str) -> String {
    if unit.ends_with('s') {
        unit.to_string()
    } else {
        format!("{unit}s")
    }
}

fn trim_price(price: f64) -> String {
    let text = format!("{price:.4}");
    let text = text.trim_end_matches('0');
    text.trim_end_matches('.').to_string()
}

fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 10.0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

pub struct Fal {
    account: Account,
    key: String,
    /// How often the queue is polled; the tests shorten it.
    pub poll: PollPolicy,
}

impl Fal {
    pub fn new(account: Account, key: String) -> Self {
        Self {
            account,
            key,
            poll: PollPolicy::default(),
        }
    }

    fn is_default_host(&self) -> bool {
        self.account.base_url.trim_end_matches('/') == descriptor(self.account.kind).presets[0].1
    }

    fn queue_url(&self, path: &str) -> String {
        self.account.url(path)
    }

    fn api_url(&self, path: &str) -> String {
        if self.is_default_host() {
            http::join("https://api.fal.ai", path)
        } else {
            self.account.url(path)
        }
    }

    fn rest_url(&self, path: &str) -> String {
        if self.is_default_host() {
            http::join("https://rest.fal.ai", path)
        } else {
            self.account.url(path)
        }
    }

    fn auth(&self, request: ureq::Request) -> ureq::Request {
        request.set("Authorization", &format!("Key {}", self.key))
    }

    fn fail(&self, error: ureq::Error) -> String {
        http::describe(error, &self.key)
    }

    /// `GET api.fal.ai/v1/models/pricing?endpoint_id=…`.
    pub fn price(&self, endpoint: &str) -> Result<Price, String> {
        let url = self.api_url(&format!(
            "v1/models/pricing?endpoint_id={}",
            http::encode(endpoint)
        ));
        let body = self
            .auth(http::agent().get(&url))
            .call()
            .map_err(|e| self.fail(e))
            .and_then(http::read_json)?;
        let entry = body
            .get("prices")
            .and_then(Value::as_array)
            .and_then(|list| {
                list.iter()
                    .find(|p| p.get("endpoint_id").and_then(Value::as_str) == Some(endpoint))
                    .or_else(|| list.first())
            })
            .ok_or_else(|| format!("fal.ai has no price for {endpoint}"))?;
        Ok(Price {
            unit_price: entry
                .get("unit_price")
                .and_then(Value::as_f64)
                .ok_or("fal.ai's price has no amount")?,
            unit: entry
                .get("unit")
                .and_then(Value::as_str)
                .unwrap_or("request")
                .to_string(),
            currency: entry
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD")
                .to_string(),
        })
    }

    /// A pricing call for the first action: free, and it proves the key.
    pub fn test(&self) -> TestReport {
        let endpoint = actions()
            .first()
            .map(|a| a.endpoint.as_str())
            .unwrap_or("fal-ai/rife/video");
        match self.price(endpoint) {
            Ok(_) => TestReport::ok("Connected"),
            Err(message) => TestReport::failed(message),
        }
    }

    pub fn estimate(&self, action: &FalAction, media: &MediaFacts) -> Result<Estimate, String> {
        Ok(Estimate::new(self.price(&action.endpoint)?, media))
    }

    /// Put a file on fal's storage; returns the URL a model input can use.
    pub fn upload(&self, path: &Path, events: &dyn Fn(JobEvent)) -> Result<String, String> {
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "clip.mp4".into());
        let content_type = content_type(path);
        let initiate = self
            .auth(
                http::agent()
                    .post(&self.rest_url("storage/upload/initiate?storage_type=fal-cdn-v3")),
            )
            .send_json(json!({"content_type": content_type, "file_name": file_name}))
            .map_err(|e| self.fail(e))
            .and_then(http::read_json)?;
        let upload_url = initiate
            .get("upload_url")
            .and_then(Value::as_str)
            .ok_or("fal.ai gave no upload address")?;
        let file_url = initiate
            .get("file_url")
            .and_then(Value::as_str)
            .ok_or("fal.ai gave no file address")?
            .to_string();
        let file = std::fs::File::open(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let total = file.metadata().map(|m| m.len()).unwrap_or(0);
        events(JobEvent::Uploading { sent: 0, total });
        // The upload URL is pre-signed; it takes no key. A length header so
        // the body is not sent chunked, which signed storage URLs refuse.
        http::agent()
            .put(upload_url)
            .set("Content-Type", content_type)
            .set("Content-Length", &total.to_string())
            .send(file)
            .map_err(|e| self.fail(e))?;
        events(JobEvent::Uploading { sent: total, total });
        Ok(file_url)
    }

    /// Run `action` on the clip at `source`, writing the result under `dir`
    /// with its provenance. `estimate` is recorded as the cost.
    pub fn run(
        &self,
        action: &FalAction,
        source: &Path,
        dir: &Path,
        estimate: Option<&Estimate>,
        events: &dyn Fn(JobEvent),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, String> {
        let video_url = self.upload(source, events)?;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let mut input = serde_json::to_value(&action.input).map_err(|e| e.to_string())?;
        input["video_url"] = json!(video_url);
        let job = FalJob {
            fal: self,
            endpoint: &action.endpoint,
            input,
        };
        let (submitted, result) = jobs::run(&job, self.poll, events, cancel)?;
        let url = output_url(&result)
            .ok_or("fal.ai finished but sent no file back")?
            .to_string();

        let stem = source
            .file_stem()
            .map(|s| provenance::slug(&s.to_string_lossy()))
            .unwrap_or_else(|| "clip".into());
        let path = dir.join(format!(
            "{stem}-{}.{}",
            action.id.replace('_', "-"),
            action.extension
        ));
        http::download(
            &url,
            &path,
            &|bytes| events(JobEvent::Downloading { bytes }),
            cancel,
        )?;

        let origin = Origin {
            title: format!("{} ({stem})", action.label),
            model: action.endpoint.clone(),
            endpoint: action.endpoint.clone(),
            prompt: action.label.clone(),
            request_id: submitted.request_id,
            cost: estimate.map(|e| Cost {
                amount: e.amount,
                unit: e.price.currency.clone(),
                estimated: true,
            }),
            licence: Licence {
                id: "LicenseRef-fal-model".into(),
                name: format!("{} model licence", action.endpoint),
                url: format!("https://fal.ai/models/{}", action.endpoint),
                commercial: Commercial::Unknown,
                note: "Each fal.ai model has its own licence; check the model's page".into(),
                ..Licence::default()
            },
            ..Origin::new(OriginKind::Generated, "fal")
        };
        provenance::write_sidecar(&path, &origin)?;
        events(JobEvent::Done);
        Ok(path)
    }
}

/// The file in a model's output: `video.url`, `image.url` or `images[0].url`.
pub fn output_url(result: &Value) -> Option<&str> {
    result
        .pointer("/video/url")
        .or_else(|| result.pointer("/image/url"))
        .or_else(|| result.pointer("/images/0/url"))
        .or_else(|| result.pointer("/audio/url"))
        .and_then(Value::as_str)
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("mkv") => "video/x-matroska",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => "video/mp4",
    }
}

struct FalJob<'a> {
    fal: &'a Fal,
    endpoint: &'a str,
    input: Value,
}

impl QueueApi for FalJob<'_> {
    fn submit(&self) -> Result<Submitted, String> {
        let body = self
            .fal
            .auth(http::agent().post(&self.fal.queue_url(self.endpoint)))
            .send_json(self.input.clone())
            .map_err(|e| self.fal.fail(e))
            .and_then(http::read_json)?;
        let field = |name: &str| {
            body.get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let request_id = field("request_id");
        if request_id.is_empty() {
            return Err("fal.ai did not accept the job".to_string());
        }
        // fal hands back absolute URLs; build them only if it did not.
        let base = format!(
            "{}/requests/{request_id}",
            self.fal.queue_url(self.endpoint)
        );
        let or = |value: String, fallback: String| if value.is_empty() { fallback } else { value };
        Ok(Submitted {
            status_url: or(field("status_url"), format!("{base}/status")),
            response_url: or(field("response_url"), base.clone()),
            cancel_url: or(field("cancel_url"), format!("{base}/cancel")),
            request_id,
        })
    }

    fn status(&self, job: &Submitted) -> Result<JobStatus, String> {
        let separator = if job.status_url.contains('?') {
            '&'
        } else {
            '?'
        };
        let body = self
            .fal
            .auth(http::agent().get(&format!("{}{separator}logs=1", job.status_url)))
            .call()
            .map_err(|e| self.fal.fail(e))
            .and_then(http::read_json)?;
        let status = body.get("status").and_then(Value::as_str).unwrap_or("");
        Ok(match status {
            "IN_QUEUE" => JobStatus::Queued(
                body.get("queue_position")
                    .and_then(Value::as_u64)
                    .map(|p| p as u32),
            ),
            "IN_PROGRESS" => JobStatus::Running(
                body.get("logs")
                    .and_then(Value::as_array)
                    .and_then(|logs| logs.last())
                    .and_then(|l| l.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ),
            "COMPLETED" => match body.get("error").and_then(Value::as_str) {
                Some(error) if !error.is_empty() => {
                    JobStatus::Failed(format!("fal.ai could not process the clip: {error}"))
                }
                _ => JobStatus::Completed,
            },
            other => JobStatus::Failed(format!("fal.ai reported an unknown state: {other}")),
        })
    }

    fn result(&self, job: &Submitted) -> Result<Value, String> {
        self.fal
            .auth(http::agent().get(&job.response_url))
            .call()
            .map_err(|e| self.fal.fail(e))
            .and_then(http::read_json)
    }

    fn cancel(&self, job: &Submitted) -> Result<(), String> {
        match self.fal.auth(http::agent().put(&job.cancel_url)).call() {
            Ok(_) => Ok(()),
            // Already finished: nothing left to stop.
            Err(ureq::Error::Status(400, _)) => Ok(()),
            Err(error) => Err(self.fal.fail(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::super::http::test_server;
    use super::super::super::registry::ProviderKind;
    use super::*;

    #[test]
    fn the_action_table_parses() {
        let ids: Vec<&str> = actions().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["remove_background", "upscale", "interpolate"]);
        assert_eq!(
            action("upscale").unwrap().input["upscale_factor"].as_float(),
            Some(2.0)
        );
    }

    #[test]
    fn estimates_follow_the_billing_unit() {
        let media = MediaFacts {
            duration_seconds: 12.0,
            width: 1920,
            height: 1080,
            fps: 30.0,
            bytes: 84_000_000,
        };
        let per_second = Estimate::new(
            Price {
                unit_price: 0.035,
                unit: "second".into(),
                currency: "USD".into(),
            },
            &media,
        );
        assert!((per_second.amount - 0.42).abs() < 1e-9);
        assert_eq!(
            per_second.summary,
            "$0.42 (12.0 seconds × $0.035 per second)"
        );
        assert_eq!(per_second.upload, "Uploads 84 MB of your clip to fal.ai");
        let per_video = Estimate::new(
            Price {
                unit_price: 0.1,
                unit: "video".into(),
                currency: "USD".into(),
            },
            &media,
        );
        assert_eq!(per_video.summary, "$0.10 per run");
    }

    #[test]
    fn a_job_is_priced_uploaded_queued_polled_and_downloaded() {
        // fal answers with absolute URLs, so the mock names its own.
        let server = test_server::serve_with(|url| {
            vec![
                (
                    200,
                    "application/json",
                    br#"{"prices":[{"endpoint_id":"fal-ai/rife/video","unit_price":0.01,"unit":"second","currency":"USD"}]}"#.to_vec(),
                ),
                (
                    200,
                    "application/json",
                    format!(r#"{{"upload_url":"{url}/put/clip","file_url":"https://cdn/clip.mp4"}}"#).into_bytes(),
                ),
                (200, "text/plain", Vec::new()),
                (
                    200,
                    "application/json",
                    format!(r#"{{"request_id":"req-1","status_url":"{url}/q/status","response_url":"{url}/q/response","cancel_url":"{url}/q/cancel","queue_position":3}}"#).into_bytes(),
                ),
                (200, "application/json", br#"{"status":"IN_QUEUE","queue_position":1}"#.to_vec()),
                (200, "application/json", br#"{"status":"IN_PROGRESS","logs":[{"message":"frame 10/40"}]}"#.to_vec()),
                (200, "application/json", br#"{"status":"COMPLETED"}"#.to_vec()),
                (
                    200,
                    "application/json",
                    format!(r#"{{"video":{{"url":"{url}/out/result.mp4"}}}}"#).into_bytes(),
                ),
                (200, "video/mp4", b"result-bytes".to_vec()),
            ]
        });
        let mut account = Account::of_kind(ProviderKind::Fal);
        account.base_url = server.url.clone();
        let mut fal = Fal::new(account, "fal-key".into());
        fal.poll = PollPolicy {
            first: Duration::from_millis(1),
            max: Duration::from_millis(2),
            timeout: Duration::from_secs(10),
        };

        let scratch =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch/cloud/fal");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        let source = scratch.join("My Clip.mp4");
        std::fs::write(&source, b"source-video").unwrap();

        let rife = action("interpolate").unwrap();
        let estimate = fal
            .estimate(
                rife,
                &MediaFacts {
                    duration_seconds: 5.0,
                    fps: 30.0,
                    bytes: 12,
                    ..MediaFacts::default()
                },
            )
            .unwrap();
        assert!((estimate.amount - 0.05).abs() < 1e-9);

        let events = std::cell::RefCell::new(Vec::new());
        let path = fal
            .run(
                rife,
                &source,
                &scratch.join("out"),
                Some(&estimate),
                &|e| events.borrow_mut().push(e),
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"result-bytes");
        assert!(
            path.ends_with("out/my-clip-interpolate.mp4"),
            "{}",
            path.display()
        );
        let origin = provenance::read_sidecar(&path).unwrap();
        assert_eq!(origin.provider, "fal");
        assert_eq!(origin.endpoint, "fal-ai/rife/video");
        assert_eq!(origin.request_id, "req-1");
        assert_eq!(origin.cost.unwrap().amount, 0.05);
        assert_eq!(origin.licence.commercial, Commercial::Unknown);

        let events = events.into_inner();
        assert!(events.contains(&JobEvent::Queued { position: Some(1) }));
        assert!(events.contains(&JobEvent::Running {
            log: Some("frame 10/40".into())
        }));
        assert_eq!(events.last(), Some(&JobEvent::Done));

        let requests = server.requests.lock().unwrap();
        assert!(requests[0]
            .request_line
            .starts_with("GET /v1/models/pricing?endpoint_id=fal-ai%2Frife%2Fvideo"));
        assert_eq!(requests[0].header("authorization"), Some("Key fal-key"));
        assert!(requests[1]
            .request_line
            .starts_with("POST /storage/upload/initiate?storage_type=fal-cdn-v3"));
        assert_eq!(requests[2].request_line, "PUT /put/clip HTTP/1.1");
        assert_eq!(requests[2].body, b"source-video");
        assert!(
            requests[2].header("authorization").is_none(),
            "signed URL, no key"
        );
        assert_eq!(requests[3].request_line, "POST /fal-ai/rife/video HTTP/1.1");
        let input: Value = serde_json::from_slice(&requests[3].body).unwrap();
        assert_eq!(input["video_url"], "https://cdn/clip.mp4");
        assert_eq!(input["num_frames"], 1);
        assert_eq!(requests[4].request_line, "GET /q/status?logs=1 HTTP/1.1");
        assert_eq!(requests[7].request_line, "GET /q/response HTTP/1.1");
        assert_eq!(requests[8].request_line, "GET /out/result.mp4 HTTP/1.1");
        assert!(requests[8].header("authorization").is_none());
    }

    #[test]
    fn a_failed_model_run_reports_fals_reason() {
        let server = test_server::serve_with(|url| {
            vec![
                (
                    200,
                    "application/json",
                    format!(r#"{{"request_id":"r","status_url":"{url}/s","response_url":"{url}/r","cancel_url":"{url}/c"}}"#).into_bytes(),
                ),
                (
                    200,
                    "application/json",
                    br#"{"status":"COMPLETED","error":"video too long"}"#.to_vec(),
                ),
            ]
        });
        let mut account = Account::of_kind(ProviderKind::Fal);
        account.base_url = server.url.clone();
        let mut fal = Fal::new(account, "k".into());
        fal.poll.first = Duration::from_millis(1);
        let job = FalJob {
            fal: &fal,
            endpoint: "fal-ai/rife/video",
            input: json!({}),
        };
        let error = jobs::run(&job, fal.poll, &|_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(error.contains("video too long"), "{error}");
    }
}
