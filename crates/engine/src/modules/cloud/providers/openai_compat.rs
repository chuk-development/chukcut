//! OpenAI's REST format, as spoken by OpenAI, Groq and local servers.
//!
//! Transcription is `POST {base}/audio/transcriptions`, multipart, asking for
//! `verbose_json` with word and segment timestamps. Not every server honours
//! that: some return segments only, some only `text`, and a few reject the
//! `timestamp_granularities[]` field outright. All three are handled — the
//! last by asking once more without it.

use serde::Deserialize;

use super::super::http::{self, Multipart};
use super::super::registry::Account;
use super::super::{TestReport, Transcribe, TranscribeRequest};
use crate::modules::captions::{Cue, TimedWord, Transcript};
use crate::modules::project::Micros;

pub struct OpenAiCompatible {
    pub account: Account,
    pub key: Option<String>,
}

impl OpenAiCompatible {
    fn key(&self) -> &str {
        self.key.as_deref().unwrap_or("")
    }

    fn authorize(&self, request: ureq::Request) -> ureq::Request {
        match self.key.as_deref() {
            // A local server needs no key, and an empty bearer header makes
            // some of them refuse the request.
            Some(key) if !key.is_empty() => request.set("Authorization", &format!("Bearer {key}")),
            _ => request,
        }
    }

    /// `GET {base}/models`: free, quick, and it fills the model list.
    pub fn test(&self) -> TestReport {
        let url = http::join(&self.account.base_url, "models");
        let response = self.authorize(http::agent().get(&url)).call();
        match response {
            Ok(response) => {
                let body: serde_json::Value = response
                    .into_string()
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                    .unwrap_or_default();
                let mut models: Vec<String> = body
                    .get("data")
                    .and_then(|d| d.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                models.sort();
                let speech: Vec<&String> = models
                    .iter()
                    .filter(|m| m.contains("whisper") || m.contains("transcribe"))
                    .collect();
                let message = if models.is_empty() {
                    "Connected".to_string()
                } else if speech.is_empty() {
                    format!("Connected, {} models", models.len())
                } else {
                    format!(
                        "Connected, {} models, {} for speech",
                        models.len(),
                        speech.len()
                    )
                };
                TestReport {
                    ok: true,
                    message,
                    models,
                }
            }
            Err(error) => TestReport {
                ok: false,
                message: http::describe(error, self.key()),
                models: Vec::new(),
            },
        }
    }

    // `ureq::Error` is large because it carries the response; it is returned
    // once per upload, which is nothing next to the upload.
    #[allow(clippy::result_large_err)]
    fn post(&self, request: &TranscribeRequest, granular: bool) -> Result<String, ureq::Error> {
        let model = request
            .model
            .clone()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| self.account.default_model.clone());
        let mut form = Multipart::new()
            .text("model", &model)
            .text("response_format", "verbose_json");
        if granular {
            form = form
                .text("timestamp_granularities[]", "word")
                .text("timestamp_granularities[]", "segment");
        }
        if let Some(language) = request.language.as_deref().filter(|l| !l.is_empty()) {
            form = form.text("language", language);
        }
        let (content_type, body) = form
            .file(
                "file",
                &request.file_name,
                &request.content_type,
                &request.audio,
            )
            .finish();
        let url = http::join(&self.account.base_url, "audio/transcriptions");
        self.authorize(http::agent().post(&url))
            .set("Content-Type", &content_type)
            .send_bytes(&body)?
            .into_string()
            .map_err(ureq::Error::from)
    }
}

impl Transcribe for OpenAiCompatible {
    fn transcribe(&self, request: &TranscribeRequest) -> Result<Transcript, String> {
        let body = match self.post(request, true) {
            Ok(body) => body,
            // A server that does not know the field says so with a 400 that
            // names it; ask once more the plain way.
            Err(ureq::Error::Status(400, response)) => {
                let text = response.into_string().unwrap_or_default();
                if text.contains("timestamp_granularities") {
                    tracing::info!("the server refused word timestamps; asking without them");
                    self.post(request, false)
                        .map_err(|e| http::describe(e, self.key()))?
                } else {
                    return Err(format!(
                        "the provider refused the request (HTTP 400): {}",
                        http::redact(&text.chars().take(200).collect::<String>(), self.key())
                    ));
                }
            }
            Err(error) => return Err(http::describe(error, self.key())),
        };
        parse_verbose(&body, request.duration)
    }
}

#[derive(Deserialize)]
struct Verbose {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    words: Option<Vec<VerboseWord>>,
    #[serde(default)]
    segments: Option<Vec<VerboseSegment>>,
}

#[derive(Deserialize)]
struct VerboseWord {
    word: String,
    start: f64,
    end: f64,
}

#[derive(Deserialize)]
struct VerboseSegment {
    start: f64,
    end: f64,
    text: String,
}

fn micros(seconds: f64) -> Micros {
    if seconds.is_finite() {
        (seconds * 1_000_000.0).round() as Micros
    } else {
        0
    }
}

/// Read a `verbose_json` answer — or a plain `{"text": …}` one, which becomes
/// a single segment over `duration`.
pub fn parse_verbose(body: &str, duration: Micros) -> Result<Transcript, String> {
    let parsed: Verbose = serde_json::from_str(body)
        .map_err(|e| format!("the provider's answer is not a transcription ({e})"))?;
    let words: Vec<TimedWord> = parsed
        .words
        .unwrap_or_default()
        .into_iter()
        .filter(|w| !w.word.trim().is_empty())
        .map(|w| TimedWord {
            text: w.word.trim().to_string(),
            start: micros(w.start),
            end: micros(w.end.max(w.start)),
        })
        .collect();
    let mut segments: Vec<Cue> = parsed
        .segments
        .unwrap_or_default()
        .into_iter()
        .filter(|s| !s.text.trim().is_empty())
        .map(|s| Cue::new(micros(s.start), micros(s.end.max(s.start)), s.text.trim()))
        .collect();
    if words.is_empty() && segments.is_empty() {
        if let Some(text) = parsed.text.filter(|t| !t.trim().is_empty()) {
            segments.push(Cue::new(0, duration.max(1), text.trim()));
        }
    }
    Ok(Transcript {
        language: parsed.language.map(|l| language_code(&l)),
        words,
        segments,
    })
}

/// OpenAI answers with the language's English name ("german"); Groq and
/// others with the code. The document wants codes.
fn language_code(language: &str) -> String {
    let lower = language.trim().to_lowercase();
    crate::modules::speech::LANGUAGES
        .iter()
        .find(|(code, name)| *code == lower || name.to_lowercase() == lower)
        .map(|(code, _)| code.to_string())
        .unwrap_or(lower)
}

#[cfg(test)]
mod tests {
    use super::super::super::http::test_server;
    use super::super::super::registry::ProviderKind;
    use super::*;

    fn provider(url: &str, key: Option<&str>) -> OpenAiCompatible {
        OpenAiCompatible {
            account: Account::new(
                ProviderKind::OpenaiCompatible,
                "test",
                &format!("{url}/v1"),
                "whisper-1",
            ),
            key: key.map(str::to_string),
        }
    }

    fn request() -> TranscribeRequest {
        TranscribeRequest {
            audio: b"RIFFfake".to_vec(),
            file_name: "audio.wav".into(),
            content_type: "audio/wav".into(),
            language: Some("de".into()),
            model: None,
            duration: 2_000_000,
        }
    }

    #[test]
    fn a_transcription_request_is_shaped_like_openais() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"language":"german","text":"Hallo Welt","words":[{"word":"Hallo","start":0.1,"end":0.5},{"word":" Welt","start":0.6,"end":1.0}],"segments":[{"start":0.1,"end":1.0,"text":" Hallo Welt"}]}"#.to_vec(),
        )]);
        let transcript = provider(&server.url, Some("sk-test-key"))
            .transcribe(&request())
            .unwrap();
        assert_eq!(transcript.language.as_deref(), Some("de"));
        assert_eq!(transcript.words.len(), 2);
        assert_eq!(transcript.words[1].text, "Welt");
        assert_eq!(transcript.words[1].start, 600_000);

        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.request_line, "POST /v1/audio/transcriptions HTTP/1.1");
        assert_eq!(sent.header("authorization"), Some("Bearer sk-test-key"));
        let agent = sent.header("user-agent").unwrap();
        assert!(
            agent.starts_with("chukcut/") && !agent.contains('@'),
            "{agent}"
        );
        let body = String::from_utf8_lossy(&sent.body);
        for part in [
            "name=\"model\"\r\n\r\nwhisper-1",
            "name=\"response_format\"\r\n\r\nverbose_json",
            "name=\"timestamp_granularities[]\"\r\n\r\nword",
            "name=\"timestamp_granularities[]\"\r\n\r\nsegment",
            "name=\"language\"\r\n\r\nde",
            "filename=\"audio.wav\"",
        ] {
            assert!(body.contains(part), "missing {part:?}");
        }
    }

    #[test]
    fn a_server_without_word_timestamps_is_asked_again_plainly() {
        let server = test_server::serve(vec![
            (
                400,
                "application/json",
                br#"{"error":{"message":"unknown field timestamp_granularities[]"}}"#.to_vec(),
            ),
            (
                200,
                "application/json",
                br#"{"segments":[{"start":0.0,"end":1.5,"text":"only segments"}]}"#.to_vec(),
            ),
        ]);
        let transcript = provider(&server.url, None).transcribe(&request()).unwrap();
        assert!(transcript.words.is_empty());
        assert_eq!(transcript.segments[0].text, "only segments");
        // And the words are estimated from it.
        assert_eq!(transcript.timed_words().len(), 2);

        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].header("authorization").is_none(),
            "no key, no header"
        );
        assert!(!String::from_utf8_lossy(&requests[1].body).contains("timestamp_granularities"));
    }

    #[test]
    fn plain_text_answers_become_one_segment() {
        let transcript = parse_verbose(r#"{"text":"just text"}"#, 3_000_000).unwrap();
        assert_eq!(
            transcript.segments,
            vec![Cue::new(0, 3_000_000, "just text")]
        );
        assert!(parse_verbose("<html>", 1).is_err());
    }

    #[test]
    fn the_connection_test_lists_models() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"data":[{"id":"whisper-large-v3-turbo"},{"id":"llama"}]}"#.to_vec(),
        )]);
        let report = provider(&server.url, Some("k")).test();
        assert!(report.ok);
        assert_eq!(report.models, ["llama", "whisper-large-v3-turbo"]);
        assert!(
            report.message.contains("1 for speech"),
            "{}",
            report.message
        );

        let refused = test_server::serve(vec![(
            401,
            "application/json",
            br#"{"error":{"message":"bad key"}}"#.to_vec(),
        )]);
        let report = provider(&refused.url, Some("k")).test();
        assert!(!report.ok);
        assert!(report.message.contains("refused the API key"));
    }
}
