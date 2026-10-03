//! ElevenLabs: voices, speech with word timing, sound effects and music.
//!
//! One `xi-api-key` header for everything. Speech goes through
//! `/with-timestamps` when the caller wants timing, because the per-character
//! alignment in that answer becomes word captions with no transcription pass.
//! The plan (`/v1/user/subscription`) is read once per provider value and
//! travels with every result: the free plan forbids commercial use, and the
//! export dialog warns about it (`docs/research/integrations.md` §1.1).

use std::sync::OnceLock;

use base64::Engine as _;
use serde_json::{json, Value};

use super::super::http::{self, SendJson as _};
use super::super::registry::Account;
use super::super::{AudioGen, AudioGenRequest, AudioOut, TestReport, Tts, TtsRequest, Voice};
use crate::modules::captions::TimedWord;

/// The format asked for: MP3 at 44.1 kHz, 128 kbit/s, which every plan may
/// request (192 kbit/s needs Creator and up).
const OUTPUT_FORMAT: &str = "mp3_44100_128";

pub struct ElevenLabs {
    account: Account,
    key: String,
    tier: OnceLock<Option<String>>,
}

impl ElevenLabs {
    pub fn new(account: Account, key: String) -> Self {
        Self {
            account,
            key,
            tier: OnceLock::new(),
        }
    }

    fn get(&self, path: &str) -> ureq::Request {
        http::agent()
            .get(&self.account.url(path))
            .set("xi-api-key", &self.key)
    }

    fn post(&self, path: &str) -> ureq::Request {
        http::agent()
            .post(&self.account.url(path))
            .set("xi-api-key", &self.key)
    }

    fn fail(&self, error: ureq::Error) -> String {
        http::describe(error, &self.key)
    }

    fn subscription(&self) -> Result<Value, String> {
        let response = self
            .get("v1/user/subscription")
            .call()
            .map_err(|e| self.fail(e))?;
        http::read_json(response)
    }

    /// The plan, asked once. A failure is "unknown", never an error: the
    /// speech itself may still work with a key scoped to TTS only.
    pub fn tier(&self) -> Option<String> {
        self.tier
            .get_or_init(|| {
                self.subscription()
                    .ok()
                    .and_then(|v| v.get("tier").and_then(|t| t.as_str()).map(str::to_string))
            })
            .clone()
    }

    /// `GET /v1/user/subscription`: the plan and how much is left.
    pub fn test(&self) -> TestReport {
        match self.subscription() {
            Ok(body) => {
                let tier = body
                    .get("tier")
                    .and_then(|t| t.as_str())
                    .map(str::to_string);
                let used = body.get("character_count").and_then(Value::as_f64);
                let limit = body.get("character_limit").and_then(Value::as_f64);
                let mut message =
                    format!("Connected, plan: {}", tier.as_deref().unwrap_or("unknown"));
                if let (Some(used), Some(limit)) = (used, limit) {
                    if limit > 0.0 {
                        let left = ((limit - used) / limit * 100.0).clamp(0.0, 100.0);
                        message.push_str(&format!(", {left:.0} % of credits left"));
                    }
                }
                if tier.as_deref() == Some("free") {
                    message.push_str(". Free plan: not for commercial use");
                }
                let _ = self.tier.set(tier.clone());
                TestReport {
                    ok: true,
                    message,
                    tier,
                    ..TestReport::default()
                }
            }
            Err(message) => TestReport::failed(message),
        }
    }

    fn model(&self, requested: &str) -> String {
        if !requested.trim().is_empty() {
            requested.trim().to_string()
        } else if !self.account.default_model.trim().is_empty() {
            self.account.default_model.trim().to_string()
        } else {
            "eleven_multilingual_v2".to_string()
        }
    }

    // `ureq::Error` carries the response; returned once per generation.
    #[allow(clippy::result_large_err)]
    fn audio(&self, path: &str, body: Value) -> Result<(Vec<u8>, String), String> {
        let response = self
            .post(&format!("{path}?output_format={OUTPUT_FORMAT}"))
            .set("Accept", "audio/mpeg")
            .send_json(body)
            .map_err(|e| self.fail(e))?;
        let request_id = response
            .header("request-id")
            .unwrap_or_default()
            .to_string();
        let bytes = http::read_bytes(response)?;
        if bytes.is_empty() {
            return Err("ElevenLabs answered with no audio".to_string());
        }
        Ok((bytes, request_id))
    }
}

impl Tts for ElevenLabs {
    /// `GET /v2/voices`, every page.
    fn voices(&self) -> Result<Vec<Voice>, String> {
        let mut voices = Vec::new();
        let mut token: Option<String> = None;
        // A guard against a server that keeps saying "has_more": ten pages
        // of a hundred is more voices than any account has.
        for _ in 0..10 {
            let mut path = "v2/voices?page_size=100".to_string();
            if let Some(token) = &token {
                path.push_str(&format!("&next_page_token={}", http::encode(token)));
            }
            let response = self.get(&path).call().map_err(|e| self.fail(e))?;
            let body = http::read_json(response)?;
            voices.extend(parse_voices(&body));
            token = body
                .get("next_page_token")
                .and_then(|t| t.as_str())
                .map(str::to_string);
            let more = body
                .get("has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !more || token.is_none() {
                break;
            }
        }
        Ok(voices)
    }

    fn speak(&self, request: &TtsRequest) -> Result<AudioOut, String> {
        if request.text.trim().is_empty() {
            return Err("there is no text to speak".to_string());
        }
        if request.voice.trim().is_empty() {
            return Err("choose a voice first".to_string());
        }
        let model = self.model(&request.model);
        let mut settings = json!({
            "stability": request.stability.clamp(0.0, 1.0),
            "similarity_boost": request.similarity.clamp(0.0, 1.0),
        });
        if (request.speed - 1.0).abs() > 0.001 {
            // ElevenLabs accepts 0.7 to 1.2.
            settings["speed"] = json!(request.speed.clamp(0.7, 1.2));
        }
        let body = json!({
            "text": request.text,
            "model_id": model,
            "voice_settings": settings,
        });
        let voice = http::encode(request.voice.trim());
        let tier = self.tier();
        if request.with_timing {
            let response = self
                .post(&format!(
                    "v1/text-to-speech/{voice}/with-timestamps?output_format={OUTPUT_FORMAT}"
                ))
                .send_json(body)
                .map_err(|e| self.fail(e))?;
            let request_id = response
                .header("request-id")
                .unwrap_or_default()
                .to_string();
            let answer = http::read_json(response)?;
            let audio = answer
                .get("audio_base64")
                .and_then(|a| a.as_str())
                .ok_or("ElevenLabs answered without audio")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(audio.trim())
                .map_err(|e| format!("ElevenLabs sent audio that does not decode ({e})"))?;
            let alignment = answer
                .get("alignment")
                .filter(|a| !a.is_null())
                .or_else(|| answer.get("normalized_alignment"));
            Ok(AudioOut {
                bytes,
                extension: "mp3".into(),
                words: alignment.map(words_from_alignment).unwrap_or_default(),
                request_id,
                model,
                tier,
            })
        } else {
            let (bytes, request_id) = self.audio(&format!("v1/text-to-speech/{voice}"), body)?;
            Ok(AudioOut {
                bytes,
                extension: "mp3".into(),
                words: Vec::new(),
                request_id,
                model,
                tier,
            })
        }
    }
}

impl AudioGen for ElevenLabs {
    /// `POST /v1/sound-generation`: 0.5 to 30 seconds.
    fn sound_effect(&self, request: &AudioGenRequest) -> Result<AudioOut, String> {
        if request.prompt.trim().is_empty() {
            return Err("describe the sound first".to_string());
        }
        let mut body = json!({ "text": request.prompt.trim() });
        if let Some(seconds) = request.duration_seconds {
            body["duration_seconds"] = json!(seconds.clamp(0.5, 30.0));
        }
        if let Some(influence) = request.prompt_influence {
            body["prompt_influence"] = json!(influence.clamp(0.0, 1.0));
        }
        if request.looping {
            // Loops need the v2 sound model.
            body["loop"] = json!(true);
            body["model_id"] = json!("eleven_text_to_sound_v2");
        }
        let tier = self.tier();
        let (bytes, request_id) = self.audio("v1/sound-generation", body)?;
        Ok(AudioOut {
            bytes,
            extension: "mp3".into(),
            words: Vec::new(),
            request_id,
            model: if request.looping {
                "eleven_text_to_sound_v2".into()
            } else {
                "sound-generation".into()
            },
            tier,
        })
    }

    /// `POST /v1/music`: 3 seconds to 10 minutes.
    fn music(&self, request: &AudioGenRequest) -> Result<AudioOut, String> {
        if request.prompt.trim().is_empty() {
            return Err("describe the music first".to_string());
        }
        let mut body = json!({
            "prompt": request.prompt.trim(),
            "model_id": "music_v1",
        });
        if let Some(seconds) = request.duration_seconds {
            body["music_length_ms"] = json!((seconds.clamp(3.0, 600.0) * 1000.0).round() as u64);
        }
        if request.instrumental {
            body["force_instrumental"] = json!(true);
        }
        let tier = self.tier();
        let (bytes, request_id) = self.audio("v1/music", body)?;
        Ok(AudioOut {
            bytes,
            extension: "mp3".into(),
            words: Vec::new(),
            request_id,
            model: "music_v1".into(),
            tier,
        })
    }
}

fn parse_voices(body: &Value) -> Vec<Voice> {
    body.get("voices")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| {
                    let id = v.get("voice_id")?.as_str()?.to_string();
                    let name = v
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_string();
                    let mut labels: Vec<String> = v
                        .get("labels")
                        .and_then(Value::as_object)
                        .map(|labels| {
                            labels
                                .values()
                                .filter_map(|l| l.as_str())
                                .filter(|l| !l.is_empty())
                                .map(|l| l.replace('_', " "))
                                .collect()
                        })
                        .unwrap_or_default();
                    labels.dedup();
                    Some(Voice {
                        id,
                        name,
                        category: v
                            .get("category")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: labels.join(", "),
                        preview_url: v
                            .get("preview_url")
                            .and_then(Value::as_str)
                            .filter(|u| !u.is_empty())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Per-character times into words: a word starts at its first character and
/// ends at its last, and whitespace separates words.
pub fn words_from_alignment(alignment: &Value) -> Vec<TimedWord> {
    let chars: Vec<&str> = alignment
        .get("characters")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|c| c.as_str().unwrap_or("")).collect())
        .unwrap_or_default();
    let times = |name: &str| -> Vec<f64> {
        alignment
            .get(name)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|t| t.as_f64().unwrap_or(0.0)).collect())
            .unwrap_or_default()
    };
    let starts = times("character_start_times_seconds");
    let ends = times("character_end_times_seconds");
    let micros = |s: f64| (s.max(0.0) * 1_000_000.0).round() as i64;

    let mut words = Vec::new();
    let mut current: Option<TimedWord> = None;
    for (i, ch) in chars.iter().enumerate() {
        let (Some(&start), Some(&end)) = (starts.get(i), ends.get(i)) else {
            break;
        };
        if ch.trim().is_empty() {
            if let Some(word) = current.take() {
                words.push(word);
            }
            continue;
        }
        match &mut current {
            Some(word) => {
                word.text.push_str(ch);
                word.end = micros(end).max(word.start);
            }
            None => {
                current = Some(TimedWord {
                    text: ch.to_string(),
                    start: micros(start),
                    end: micros(end).max(micros(start)),
                })
            }
        }
    }
    words.extend(current);
    words
}

#[cfg(test)]
mod tests {
    use super::super::super::http::test_server;
    use super::super::super::registry::ProviderKind;
    use super::*;

    fn provider(url: &str) -> ElevenLabs {
        let mut account = Account::of_kind(ProviderKind::Elevenlabs);
        account.base_url = url.to_string();
        ElevenLabs::new(account, "xi-secret-key".into())
    }

    fn subscription(tier: &str) -> (u16, &'static str, Vec<u8>) {
        (
            200,
            "application/json",
            format!(r#"{{"tier":"{tier}","character_count":2500,"character_limit":10000}}"#)
                .into_bytes(),
        )
    }

    #[test]
    fn the_connection_test_reads_the_plan() {
        let server = test_server::serve(vec![subscription("free")]);
        let report = provider(&server.url).test();
        assert!(report.ok);
        assert_eq!(report.tier.as_deref(), Some("free"));
        assert!(
            report.message.contains("75 % of credits left"),
            "{}",
            report.message
        );
        assert!(report.message.contains("not for commercial use"));
        let sent = server.requests.lock().unwrap()[0].clone();
        assert_eq!(sent.request_line, "GET /v1/user/subscription HTTP/1.1");
        assert_eq!(sent.header("xi-api-key"), Some("xi-secret-key"));
        assert!(sent.header("user-agent").unwrap().starts_with("chukcut/"));
    }

    #[test]
    fn voices_come_with_labels_and_previews() {
        let server = test_server::serve(vec![
            (
                200,
                "application/json",
                br#"{"voices":[{"voice_id":"v1","name":"Rachel","category":"premade","labels":{"accent":"american","gender":"female"},"preview_url":"https://x/p.mp3"}],"has_more":true,"next_page_token":"t 2"}"#.to_vec(),
            ),
            (
                200,
                "application/json",
                br#"{"voices":[{"voice_id":"v2","name":"Adam","labels":{}}],"has_more":false}"#
                    .to_vec(),
            ),
        ]);
        let voices = provider(&server.url).voices().unwrap();
        assert_eq!(voices.len(), 2);
        assert_eq!(voices[0].description, "american, female");
        assert_eq!(voices[0].preview_url.as_deref(), Some("https://x/p.mp3"));
        assert_eq!(voices[1].preview_url, None);
        let requests = server.requests.lock().unwrap();
        assert!(requests[1].request_line.contains("next_page_token=t%202"));
    }

    #[test]
    fn speech_with_timestamps_gives_words() {
        let audio = base64::engine::general_purpose::STANDARD.encode(b"ID3fake-mp3");
        let body = format!(
            r#"{{"audio_base64":"{audio}","alignment":{{"characters":["H","i"," ","y","o","u"],"character_start_times_seconds":[0.0,0.1,0.2,0.3,0.4,0.5],"character_end_times_seconds":[0.1,0.2,0.3,0.4,0.5,0.6]}}}}"#
        );
        let server = test_server::serve(vec![
            subscription("creator"),
            (200, "application/json", body.into_bytes()),
        ]);
        let mut request = TtsRequest::new("Hi you", "voice-1");
        request.speed = 1.1;
        let out = provider(&server.url).speak(&request).unwrap();
        assert_eq!(out.bytes, b"ID3fake-mp3");
        assert_eq!(out.tier.as_deref(), Some("creator"));
        assert_eq!(out.model, "eleven_multilingual_v2");
        assert_eq!(
            out.words,
            vec![
                TimedWord {
                    text: "Hi".into(),
                    start: 0,
                    end: 200_000
                },
                TimedWord {
                    text: "you".into(),
                    start: 300_000,
                    end: 600_000
                },
            ]
        );
        let sent = server.requests.lock().unwrap()[1].clone();
        assert!(sent
            .request_line
            .starts_with("POST /v1/text-to-speech/voice-1/with-timestamps?output_format="));
        let json: Value = serde_json::from_slice(&sent.body).unwrap();
        assert_eq!(json["text"], "Hi you");
        assert_eq!(json["model_id"], "eleven_multilingual_v2");
        assert!((json["voice_settings"]["speed"].as_f64().unwrap() - 1.1).abs() < 1e-6);
    }

    #[test]
    fn sound_effects_and_music_send_their_settings() {
        let server = test_server::serve(vec![
            subscription("free"),
            (200, "audio/mpeg", b"sfx-bytes".to_vec()),
            (200, "audio/mpeg", b"music-bytes".to_vec()),
        ]);
        let eleven = provider(&server.url);
        let sfx = eleven
            .sound_effect(&AudioGenRequest {
                prompt: "door slam".into(),
                duration_seconds: Some(45.0),
                looping: true,
                instrumental: false,
                prompt_influence: Some(0.4),
            })
            .unwrap();
        assert_eq!(sfx.bytes, b"sfx-bytes");
        assert_eq!(sfx.tier.as_deref(), Some("free"));
        let music = eleven
            .music(&AudioGenRequest {
                prompt: "lofi beat".into(),
                duration_seconds: Some(20.0),
                looping: false,
                instrumental: true,
                prompt_influence: None,
            })
            .unwrap();
        assert_eq!(music.bytes, b"music-bytes");

        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "the plan is asked once");
        assert!(requests[1]
            .request_line
            .starts_with("POST /v1/sound-generation?"));
        let sfx: Value = serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(sfx["duration_seconds"], 30.0, "clamped to the API's range");
        assert_eq!(sfx["loop"], true);
        assert!(requests[2].request_line.starts_with("POST /v1/music?"));
        let music: Value = serde_json::from_slice(&requests[2].body).unwrap();
        assert_eq!(music["music_length_ms"], 20_000);
        assert_eq!(music["force_instrumental"], true);
    }

    #[test]
    fn a_refused_key_is_never_echoed() {
        let server = test_server::serve(vec![(
            401,
            "application/json",
            br#"{"detail":{"status":"invalid_api_key","message":"Invalid API key xi-secret-key"}}"#
                .to_vec(),
        )]);
        let report = provider(&server.url).test();
        assert!(!report.ok);
        assert!(
            !report.message.contains("xi-secret-key"),
            "{}",
            report.message
        );
    }
}
