//! What the engine and the CUDA helper say to each other.
//!
//! One transcription per helper process; the helper loads the model per call
//! anyway, so a long-lived process would only hold GPU memory between the
//! rare times a user captions.
//!
//! ```text
//! helper → engine   {"type":"hello", "protocol":1, "device":{…}}      at start
//! engine → helper   {"model":"…", "language":"de", "samples":N}\n    one line
//!                   N little-endian f32 samples, 16 kHz mono
//! helper → engine   {"type":"progress", "fraction":0.42}               any number
//!                   {"type":"done", "transcription":{…}}  or  {"type":"error", …}
//! ```
//!
//! Every message from the helper is one line of JSON on stdout. The helper
//! logs nothing there. Cancelling is killing the process: there is no state
//! worth a clean shutdown.
//!
//! `chukcut-whisper-cuda --probe` prints the hello and exits, which is how
//! the editor learns which device a transcription would use without loading
//! a model.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Device, Transcription};

/// Bumped on any change to the messages. The engine refuses a helper that
/// says another number and transcribes on the CPU.
pub const PROTOCOL_VERSION: u32 = 1;

/// What to transcribe. The samples follow the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub model: PathBuf,
    /// An ISO 639-1 code, or `None` to let whisper detect it.
    pub language: Option<String>,
    /// How many f32 samples follow.
    pub samples: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Hello { protocol: u32, device: Device },
    Progress { fraction: f32 },
    Done { transcription: Transcription },
    Error { message: String },
}

/// Send `request` and its samples.
pub fn write_request(out: &mut impl Write, request: &Request, samples: &[f32]) -> io::Result<()> {
    debug_assert_eq!(request.samples, samples.len());
    serde_json::to_writer(&mut *out, request)?;
    out.write_all(b"\n")?;
    // In pieces, so a ten-minute timeline (38 MB) is not copied once more.
    let mut buffer = Vec::with_capacity(64 * 1024);
    for chunk in samples.chunks(16 * 1024) {
        buffer.clear();
        for sample in chunk {
            buffer.extend_from_slice(&sample.to_le_bytes());
        }
        out.write_all(&buffer)?;
    }
    out.flush()
}

/// Read a request and its samples.
pub fn read_request(input: &mut impl BufRead) -> io::Result<(Request, Vec<f32>)> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "no request arrived",
        ));
    }
    let request: Request = serde_json::from_str(line.trim_end())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut bytes = vec![0u8; request.samples * 4];
    input.read_exact(&mut bytes)?;
    let samples = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    Ok((request, samples))
}

/// Send one reply line and flush it, so the engine sees progress at once.
pub fn write_reply(out: &mut impl Write, reply: &Reply) -> io::Result<()> {
    serde_json::to_writer(&mut *out, reply)?;
    out.write_all(b"\n")?;
    out.flush()
}

/// Read one reply line; `None` at the end of the stream.
pub fn read_reply(input: &mut impl BufRead) -> io::Result<Option<Reply>> {
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    serde_json::from_str(line.trim_end())
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Word;

    #[test]
    fn a_request_and_its_samples_survive_the_pipe() {
        let samples: Vec<f32> = (0..40_000).map(|i| (i as f32 * 0.001).sin()).collect();
        let request = Request {
            model: PathBuf::from("/models/ggml-tiny.bin"),
            language: Some("de".into()),
            samples: samples.len(),
        };
        let mut wire = Vec::new();
        write_request(&mut wire, &request, &samples).unwrap();
        let (back, back_samples) = read_request(&mut io::Cursor::new(wire)).unwrap();
        assert_eq!(back, request);
        assert_eq!(back_samples, samples);
    }

    #[test]
    fn a_short_payload_is_an_error_not_silence() {
        let request = Request {
            model: PathBuf::from("m"),
            language: None,
            samples: 10,
        };
        let mut wire = Vec::new();
        write_request(&mut wire, &request, &[0.0; 10]).unwrap();
        wire.truncate(wire.len() - 8);
        assert!(read_request(&mut io::Cursor::new(wire)).is_err());
    }

    #[test]
    fn replies_are_one_line_each() {
        let replies = vec![
            Reply::Hello {
                protocol: PROTOCOL_VERSION,
                device: Device::cpu(),
            },
            Reply::Progress { fraction: 0.5 },
            Reply::Done {
                transcription: Transcription {
                    language: Some("en".into()),
                    words: vec![Word {
                        text: "ask\nnot".into(),
                        start: 100_000,
                        end: 400_000,
                    }],
                },
            },
            Reply::Error {
                message: "out of memory".into(),
            },
        ];
        let mut wire = Vec::new();
        for reply in &replies {
            write_reply(&mut wire, reply).unwrap();
        }
        assert_eq!(wire.iter().filter(|&&b| b == b'\n').count(), replies.len());
        let mut input = io::Cursor::new(wire);
        for reply in &replies {
            assert_eq!(read_reply(&mut input).unwrap().as_ref(), Some(reply));
        }
        assert_eq!(read_reply(&mut input).unwrap(), None);
    }
}
