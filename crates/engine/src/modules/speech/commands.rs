//! Commands for transcription.
//!
//! [`speech_transcribe`] is blocking and long — seconds for a cloud call, a
//! minute or more for a local model on a long timeline. Run it on a
//! background thread; it reports through `progress` and stops when `cancel`
//! is set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;

use super::audio;
use super::models::LocalModel;
use super::settings::{Backend, SpeechSettings};
use crate::modules::captions::Transcript;
use crate::modules::cloud::{self, CloudStore, Transcribe, TranscribeRequest};
use crate::state::AppState;

/// Where a transcription is, for a progress bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpeechProgress {
    /// A short sentence: "Extracting audio", "Uploading part 2 of 3".
    pub label: String,
    /// 0..1, when the stage has a measurable length.
    pub fraction: Option<f32>,
}

impl SpeechProgress {
    fn new(label: impl Into<String>, fraction: Option<f32>) -> Self {
        Self {
            label: label.into(),
            fraction,
        }
    }
}

/// Transcribe the open project's timeline with the transcriber `settings`
/// names.
pub fn speech_transcribe(
    state: &Arc<AppState>,
    settings: &SpeechSettings,
    progress: &(dyn Fn(SpeechProgress) + Sync),
    cancel: &AtomicBool,
) -> Result<Transcript, String> {
    // A snapshot: the lock is not held across minutes of work.
    let project = state.project.read().clone().ok_or("no project is open")?;

    progress(SpeechProgress::new("Extracting audio", None));
    let samples = audio::timeline_audio(&project, cancel)?;
    if samples.iter().all(|s| s.abs() < 1e-4) {
        return Err("the timeline is silent".to_string());
    }

    match settings.backend {
        Backend::Cloud => {
            let account = settings
                .account
                .as_deref()
                .ok_or("choose an account to transcribe with, or add one")?;
            let transcriber = cloud::transcriber(&CloudStore::user(), account)?;
            transcribe_chunked(
                &samples,
                transcriber.as_ref(),
                settings.language.as_deref(),
                settings.model.as_deref(),
                audio::MAX_CHUNK,
                progress,
                cancel,
            )
        }
        Backend::Local => {
            let model = settings.local_model;
            let path = model.ensure(
                &|done, total| {
                    let fraction = (total > 0).then(|| done as f32 / total as f32);
                    progress(SpeechProgress::new(
                        format!(
                            "Downloading the {} model ({} of {} MB)",
                            short_name(model),
                            done / 1_000_000,
                            total / 1_000_000
                        ),
                        fraction,
                    ));
                },
                cancel,
            )?;
            progress(SpeechProgress::new(
                "Transcribing on this computer",
                Some(0.0),
            ));
            super::local::transcribe(
                &path,
                &samples,
                settings.language.as_deref(),
                &|fraction| {
                    progress(SpeechProgress::new(
                        "Transcribing on this computer",
                        Some(fraction),
                    ))
                },
                cancel,
            )
        }
    }
}

fn short_name(model: LocalModel) -> &'static str {
    model.label().split(" (").next().unwrap_or("")
}

/// Send `samples` in chunks of at most `max_chunk` samples, one request each,
/// and stitch the answers into one transcript in timeline time.
pub fn transcribe_chunked(
    samples: &[f32],
    transcriber: &dyn Transcribe,
    language: Option<&str>,
    model: Option<&str>,
    max_chunk: usize,
    progress: &(dyn Fn(SpeechProgress) + Sync),
    cancel: &AtomicBool,
) -> Result<Transcript, String> {
    let ranges = audio::chunks(samples, max_chunk, audio::CUT_SEARCH.min(max_chunk / 4));
    let count = ranges.len();
    let mut transcript = Transcript::default();
    for (index, range) in ranges.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let label = if count == 1 {
            "Transcribing".to_string()
        } else {
            format!("Transcribing part {} of {count}", index + 1)
        };
        progress(SpeechProgress::new(
            label,
            Some(index as f32 / count as f32),
        ));

        let offset = audio::micros(range.start);
        let piece = &samples[range];
        let request = TranscribeRequest {
            audio: audio::wav_bytes(piece),
            file_name: "audio.wav".to_string(),
            content_type: "audio/wav".to_string(),
            language: language.map(str::to_string),
            model: model.map(str::to_string),
            duration: audio::micros(piece.len()),
        };
        let part = transcriber.transcribe(&request).map_err(|e| {
            if count > 1 {
                format!("part {} of {count}: {e}", index + 1)
            } else {
                e
            }
        })?;
        transcript.extend(part.shifted(offset));
    }
    progress(SpeechProgress::new("Transcribed", Some(1.0)));
    Ok(transcript)
}

/// A local model as the panel lists it.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub model: LocalModel,
    pub label: &'static str,
    pub downloaded: bool,
}

pub fn speech_models() -> Vec<ModelInfo> {
    LocalModel::ALL
        .into_iter()
        .map(|model| ModelInfo {
            model,
            label: model.label(),
            downloaded: model.is_downloaded(),
        })
        .collect()
}

/// Whether this build can transcribe offline.
pub fn speech_local_available() -> bool {
    super::local::AVAILABLE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cloud::http::test_server;
    use crate::modules::cloud::providers::openai_compat::OpenAiCompatible;
    use crate::modules::cloud::{Account, ProviderKind};

    /// Long audio is sent in pieces and the pieces' times are moved back onto
    /// the timeline: the second answer's "0.5 s" is 0.5 s into the second
    /// chunk, not into the clip.
    #[test]
    fn chunks_are_stitched_back_in_timeline_time() {
        let server = test_server::serve(vec![
            (
                200,
                "application/json",
                br#"{"words":[{"word":"first","start":0.5,"end":0.9}]}"#.to_vec(),
            ),
            (
                200,
                "application/json",
                br#"{"words":[{"word":"second","start":0.5,"end":0.9}]}"#.to_vec(),
            ),
            (
                200,
                "application/json",
                br#"{"segments":[{"start":0.0,"end":0.4,"text":"third part"}]}"#.to_vec(),
            ),
        ]);
        let transcriber = OpenAiCompatible {
            account: Account::new(
                ProviderKind::OpenaiCompatible,
                "mock",
                &format!("{}/v1", server.url),
                "whisper-1",
            ),
            key: Some("k".into()),
        };
        // 5 s of noise, chunks of at most 2 s: three requests.
        let samples: Vec<f32> = (0..audio::RATE as usize * 5)
            .map(|i| ((i * 7919) % 200) as f32 / 400.0 - 0.25)
            .collect();
        let seen = std::sync::Mutex::new(Vec::new());
        let transcript = transcribe_chunked(
            &samples,
            &transcriber,
            None,
            None,
            audio::RATE as usize * 2,
            &|p| seen.lock().unwrap().push(p),
            &AtomicBool::new(false),
        )
        .unwrap();

        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let ranges = audio::chunks(&samples, audio::RATE as usize * 2, audio::RATE as usize / 2);
        let offsets: Vec<i64> = ranges.iter().map(|r| audio::micros(r.start)).collect();
        // Each upload is the WAV of exactly its chunk.
        for (request, range) in requests.iter().zip(&ranges) {
            let wav = audio::wav_bytes(&samples[range.clone()]);
            let body = &request.body;
            assert!(
                body.windows(wav.len()).any(|w| w == wav.as_slice()),
                "the chunk's audio is not in the upload"
            );
        }
        assert_eq!(transcript.words.len(), 2);
        assert_eq!(transcript.words[0].start, 500_000 + offsets[0]);
        assert_eq!(transcript.words[1].start, 500_000 + offsets[1]);
        assert_eq!(transcript.segments[0].start, offsets[2]);
        assert!(offsets[1] > 0 && offsets[2] > offsets[1]);

        let seen = seen.into_inner().unwrap();
        assert_eq!(seen[0].label, "Transcribing part 1 of 3");
        assert_eq!(seen.last().unwrap().fraction, Some(1.0));
    }

    #[test]
    fn a_failing_chunk_names_itself_and_cancel_stops_before_sending() {
        let server = test_server::serve(vec![
            (200, "application/json", br#"{"text":"ok"}"#.to_vec()),
            (
                500,
                "application/json",
                br#"{"error":{"message":"boom"}}"#.to_vec(),
            ),
        ]);
        let transcriber = OpenAiCompatible {
            account: Account::new(ProviderKind::OpenaiCompatible, "mock", &server.url, "m"),
            key: None,
        };
        let samples = vec![0.1f32; audio::RATE as usize * 4];
        let error = transcribe_chunked(
            &samples,
            &transcriber,
            None,
            None,
            audio::RATE as usize * 2,
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        // Flat audio has no pause, so the cuts land early and there are three parts.
        assert!(error.starts_with("part 2 of "), "{error}");
        assert!(error.contains("boom"));

        let cancelled = transcribe_chunked(
            &samples,
            &transcriber,
            None,
            None,
            audio::RATE as usize * 2,
            &|_| {},
            &AtomicBool::new(true),
        );
        assert_eq!(cancelled.unwrap_err(), "cancelled");
    }
}
