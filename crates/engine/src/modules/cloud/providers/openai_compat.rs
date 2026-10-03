//! OpenAI's REST format, as spoken by OpenAI, Groq and local servers.
//!
//! Transcription is `POST {base}/audio/transcriptions`, multipart, asking for
//! `verbose_json` with word and segment timestamps. Not every server honours
//! that: some return segments only, some only `text`, and a few reject the
//! `timestamp_granularities[]` field outright. All three are handled — the
//! last by asking once more without it.

use serde::Deserialize;

use super::super::http::{self, Multipart, SendJson as _};
use super::super::registry::Account;
use super::super::{
    AudioOut, TestReport, Transcribe, TranscribeRequest, Translate, Tts, TtsRequest, Voice,
};
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
                    tier: None,
                }
            }
            Err(error) => TestReport::failed(http::describe(error, self.key())),
        }
    }

    /// Whether this account is OpenAI itself rather than a server that
    /// copies its API: OpenAI's voices are a fixed list and its terms are
    /// known.
    pub fn is_openai(&self) -> bool {
        self.account.base_url.contains("api.openai.com")
    }

    fn speech_model(&self, requested: &str) -> String {
        let requested = requested.trim();
        if !requested.is_empty() {
            return requested.to_string();
        }
        let default = self.account.default_model.trim();
        // The account's default is usually its transcription model.
        if !default.is_empty() && !default.contains("whisper") && !default.contains("transcribe") {
            return default.to_string();
        }
        if self.is_openai() {
            "gpt-4o-mini-tts".to_string()
        } else {
            // What Kokoro-FastAPI, LocalAI and most clones accept.
            "tts-1".to_string()
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

/// OpenAI's voices for `gpt-4o-mini-tts`, in its own recommended order.
pub const OPENAI_VOICES: &[&str] = &[
    "marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage",
    "shimmer", "verse",
];

impl Tts for OpenAiCompatible {
    /// OpenAI's fixed list; for anything else, Kokoro-FastAPI's
    /// `GET /audio/voices`, and an empty list (type a voice name) when the
    /// server has no such endpoint.
    fn voices(&self) -> Result<Vec<Voice>, String> {
        let voice = |id: &str| Voice {
            id: id.to_string(),
            name: id.to_string(),
            category: String::new(),
            description: String::new(),
            preview_url: None,
        };
        if self.is_openai() {
            return Ok(OPENAI_VOICES.iter().map(|v| voice(v)).collect());
        }
        let url = http::join(&self.account.base_url, "audio/voices");
        match self.authorize(http::agent().get(&url)).call() {
            Ok(response) => {
                let body = http::read_json(response).unwrap_or_default();
                Ok(body
                    .get("voices")
                    .and_then(|v| v.as_array())
                    .map(|list| {
                        list.iter()
                            .filter_map(|v| {
                                v.as_str()
                                    .or_else(|| v.get("id").and_then(|i| i.as_str()))
                                    .or_else(|| v.get("name").and_then(|i| i.as_str()))
                            })
                            .map(voice)
                            .collect()
                    })
                    .unwrap_or_default())
            }
            Err(ureq::Error::Status(404 | 405, _)) => Ok(Vec::new()),
            Err(error) => Err(http::describe(error, self.key())),
        }
    }

    /// `POST {base}/audio/speech`. No timing in the answer; captions for it
    /// need a transcription pass.
    fn speak(&self, request: &TtsRequest) -> Result<AudioOut, String> {
        if request.text.trim().is_empty() {
            return Err("there is no text to speak".to_string());
        }
        let voice = request.voice.trim();
        if voice.is_empty() {
            return Err("choose or type a voice first".to_string());
        }
        let model = self.speech_model(&request.model);
        let mut body = serde_json::json!({
            "model": model,
            "input": request.text,
            "voice": voice,
            "response_format": "mp3",
        });
        if (request.speed - 1.0).abs() > 0.001 {
            body["speed"] = serde_json::json!(request.speed.clamp(0.25, 4.0));
        }
        if !request.instructions.trim().is_empty() {
            body["instructions"] = serde_json::json!(request.instructions.trim());
        }
        let url = http::join(&self.account.base_url, "audio/speech");
        let response = self
            .authorize(http::agent().post(&url))
            .send_json(body)
            .map_err(|e| http::describe(e, self.key()))?;
        let request_id = response
            .header("x-request-id")
            .unwrap_or_default()
            .to_string();
        let bytes = http::read_bytes(response)?;
        if bytes.is_empty() {
            return Err("the server answered with no audio".to_string());
        }
        Ok(AudioOut {
            bytes,
            extension: "mp3".into(),
            words: Vec::new(),
            request_id,
            model,
            tier: None,
        })
    }
}

/// Caption translation through `POST {base}/chat/completions`, the fallback
/// when there is no DeepL key. Lines go out as a JSON array and must come
/// back as one of the same length, so caption timing is untouched.
pub struct ChatTranslator {
    pub provider: OpenAiCompatible,
    /// Empty picks a cheap default for OpenAI and the account's model
    /// elsewhere.
    pub model: String,
}

/// Lines per request: small enough that a model keeps count.
const CHAT_BATCH: usize = 40;

impl ChatTranslator {
    fn model(&self) -> String {
        if !self.model.trim().is_empty() {
            self.model.trim().to_string()
        } else if self.provider.is_openai() {
            "gpt-4o-mini".to_string()
        } else {
            self.provider.account.default_model.clone()
        }
    }

    fn batch(
        &self,
        lines: &[String],
        target: &str,
        source: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let from = source
            .map(|s| format!(" from {}", crate::modules::speech::language_name(s)))
            .unwrap_or_default();
        let to = crate::modules::speech::language_name(target);
        let system = format!(
            "You translate video captions{from} into {to}. The user sends a JSON array of caption lines. \
             Answer with only a JSON array of exactly {} strings: the translations, in the same order, \
             one per line, keeping line breaks, emoji and tone. No commentary.",
            lines.len()
        );
        let body = serde_json::json!({
            "model": self.model(),
            "temperature": 0.2,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": serde_json::to_string(lines).unwrap_or_default()},
            ],
        });
        let url = http::join(&self.provider.account.base_url, "chat/completions");
        let response = self
            .provider
            .authorize(http::agent().post(&url))
            .send_json(body)
            .map_err(|e| http::describe(e, self.provider.key()))?;
        let answer = http::read_json(response)?;
        let content = answer
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .ok_or("the model answered with no text")?;
        parse_line_array(content, lines.len())
    }
}

/// The JSON array in a model's answer, which may come wrapped in a code
/// fence or a sentence despite the instructions.
pub fn parse_line_array(content: &str, expected: usize) -> Result<Vec<String>, String> {
    let start = content
        .find('[')
        .ok_or("the model did not answer with a list")?;
    let end = content
        .rfind(']')
        .ok_or("the model did not answer with a list")?;
    let lines: Vec<String> = serde_json::from_str(&content[start..=end])
        .map_err(|e| format!("the model's list does not parse ({e})"))?;
    if lines.len() != expected {
        return Err(format!(
            "the model returned {} lines for {expected}; try again or use DeepL",
            lines.len()
        ));
    }
    Ok(lines)
}

impl Translate for ChatTranslator {
    fn translate(
        &self,
        lines: &[String],
        target: &str,
        source: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let mut out = Vec::with_capacity(lines.len());
        for chunk in lines.chunks(CHAT_BATCH) {
            out.extend(self.batch(chunk, target, source)?);
        }
        Ok(out)
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
    fn speech_is_posted_in_openais_shape() {
        let server = test_server::serve(vec![(200, "audio/mpeg", b"mp3".to_vec())]);
        let mut request = TtsRequest::new("Hello", "af_bella");
        request.instructions = "cheerful".into();
        let out = provider(&server.url, Some("k")).speak(&request).unwrap();
        assert_eq!(out.bytes, b"mp3");
        assert_eq!(
            out.model, "tts-1",
            "a whisper default is not a speech model"
        );
        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.request_line, "POST /v1/audio/speech HTTP/1.1");
        let json: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
        assert_eq!(json["input"], "Hello");
        assert_eq!(json["voice"], "af_bella");
        assert_eq!(json["instructions"], "cheerful");
        assert!(json.get("speed").is_none());
    }

    #[test]
    fn voices_come_from_the_server_or_are_typed() {
        let server = test_server::serve(vec![
            (
                200,
                "application/json",
                br#"{"voices":["af_bella","am_adam"]}"#.to_vec(),
            ),
            (404, "application/json", b"{}".to_vec()),
        ]);
        let p = provider(&server.url, None);
        assert_eq!(p.voices().unwrap().len(), 2);
        assert!(p.voices().unwrap().is_empty(), "no list: type a name");
    }

    #[test]
    fn chat_translation_keeps_the_line_count() {
        let server = test_server::serve(vec![(
            200,
            "application/json",
            br#"{"choices":[{"message":{"content":"```json\n[\"Hallo\", \"Welt\"]\n```"}}]}"#
                .to_vec(),
        )]);
        let translator = ChatTranslator {
            provider: provider(&server.url, Some("k")),
            model: "gpt-4o-mini".into(),
        };
        let out = translator
            .translate(&["Hello".into(), "World".into()], "de", Some("en"))
            .unwrap();
        assert_eq!(out, ["Hallo", "Welt"]);
        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.request_line, "POST /v1/chat/completions HTTP/1.1");
        assert!(parse_line_array(r#"["one"]"#, 2).is_err());
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
