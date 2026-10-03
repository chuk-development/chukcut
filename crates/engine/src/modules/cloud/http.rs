//! The one HTTP client every provider uses.
//!
//! Blocking `ureq` with rustls, called from `shell::spawn_blocking` threads.
//! Every request carries `User-Agent: chukcut/<version>` and nothing else that
//! identifies the person using it — no e-mail, no account name, no hostname.
//! Request headers are never logged, so a key cannot reach a log file through
//! here; provider error *bodies* are, after [`redact`] has had a look.

use std::time::Duration;

/// The neutral agent string. The version, never anything about the user.
pub const USER_AGENT: &str = concat!("chukcut/", env!("CARGO_PKG_VERSION"));

/// A client with sane timeouts: quick to connect, patient to read, because a
/// transcription of ten minutes of audio can take a minute to answer.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(600))
        .timeout_write(Duration::from_secs(600))
        .build()
}

/// `send_json` for `ureq::Request` without ureq's `json` feature (and the
/// second copy of serde glue it brings).
pub trait SendJson {
    // `ureq::Error` carries the response; it is returned once per request.
    #[allow(clippy::result_large_err)]
    fn send_json(self, body: serde_json::Value) -> Result<ureq::Response, ureq::Error>;
}

impl SendJson for ureq::Request {
    fn send_json(self, body: serde_json::Value) -> Result<ureq::Response, ureq::Error> {
        self.set("Content-Type", "application/json")
            .send_string(&body.to_string())
    }
}

/// `base` and `path` joined with exactly one slash.
pub fn join(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// A `multipart/form-data` body, built by hand: two field kinds is all any
/// provider here needs, and a crate for it would be more code than this.
pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Default for Multipart {
    fn default() -> Self {
        Self::new()
    }
}

impl Multipart {
    pub fn new() -> Self {
        Self {
            boundary: format!("chukcut-{}", uuid::Uuid::new_v4().simple()),
            body: Vec::new(),
        }
    }

    pub fn text(mut self, name: &str, value: &str) -> Self {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self
    }

    pub fn file(mut self, name: &str, file_name: &str, content_type: &str, data: &[u8]) -> Self {
        self.body.extend_from_slice(
            format!(
                "--{}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {content_type}\r\n\r\n",
                self.boundary
            )
            .as_bytes(),
        );
        self.body.extend_from_slice(data);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// The `Content-Type` header value and the finished body.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
    }
}

/// A request error as one sentence a user can act on.
///
/// The provider's own message is used where it sent one in the usual
/// `{"error": {"message": …}}` shape, because "model `whisper-2` does not
/// exist" says more than any status code.
pub fn describe(error: ureq::Error, key: &str) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .or_else(|| v.pointer("/detail/message"))
                        .or_else(|| v.pointer("/error"))
                        .or_else(|| v.pointer("/message"))
                        .or_else(|| v.pointer("/detail"))
                        .and_then(|m| m.as_str().map(str::to_string))
                })
                .unwrap_or_else(|| body.chars().take(200).collect());
            let message = redact(&message, key);
            let lead = match code {
                401 | 403 => "the provider refused the API key",
                404 => "the provider does not have that endpoint or model",
                413 => "the upload is too large for this provider",
                429 => "the provider is rate limiting or the account is out of credit",
                500..=599 => "the provider had an internal error",
                _ => "the provider refused the request",
            };
            if message.trim().is_empty() {
                format!("{lead} (HTTP {code})")
            } else {
                format!("{lead} (HTTP {code}): {}", message.trim())
            }
        }
        ureq::Error::Transport(transport) => {
            format!(
                "could not reach the provider: {}",
                redact(&transport.to_string(), key)
            )
        }
    }
}

/// The largest answer read into memory: a ten-minute music track at 192
/// kbit/s is 15 MB, so this leaves room without letting a broken server fill
/// the RAM.
pub const MAX_BODY: u64 = 256 * 1024 * 1024;

/// A response body as bytes, up to [`MAX_BODY`]. `ureq`'s `into_string`
/// stops at 10 MB, which a minute of WAV already passes.
pub fn read_bytes(response: ureq::Response) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("the answer broke off: {e}"))?;
    Ok(bytes)
}

/// A JSON response body.
pub fn read_json(response: ureq::Response) -> Result<serde_json::Value, String> {
    let bytes = read_bytes(response)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("the provider's answer is not JSON ({e})"))
}

/// Fetch `url` into `dest` without holding it in memory: written to a
/// `.part` beside it and renamed when complete, so a broken download never
/// looks like a finished file. `progress` gets the bytes so far. No key is
/// sent: every URL fetched this way is a public or signed CDN link.
pub fn download(
    url: &str,
    dest: &std::path::Path,
    progress: &dyn Fn(u64),
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<u64, String> {
    use std::io::{Read as _, Write as _};
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let response = agent().get(url).call().map_err(|e| describe(e, ""))?;
    let mut reader = response.into_reader().take(8 * MAX_BODY);
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("bin")
    ));
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut buffer = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err("cancelled".to_string());
        }
        let n = reader
            .read(&mut buffer)
            .map_err(|e| format!("the download broke off: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
        total += n as u64;
        progress(total);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&part, dest).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    Ok(total)
}

/// `text` with `%`-escapes where a URL query value needs them.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Remove `key` from `text`, in case a provider echoes it back.
pub fn redact(text: &str, key: &str) -> String {
    if key.len() >= 4 {
        text.replace(key, "<key>")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
pub(crate) mod test_server {
    //! A one-thread HTTP server for tests: it answers each connection with the
    //! next canned response and records what it was sent.

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub struct Recorded {
        pub request_line: String,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Recorded {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    pub struct Server {
        pub url: String,
        pub requests: Arc<Mutex<Vec<Recorded>>>,
    }

    /// Serve `responses` in order: `(status, content type, body)`.
    pub fn serve(responses: Vec<(u16, &'static str, Vec<u8>)>) -> Server {
        serve_with(|_| responses)
    }

    /// Like [`serve`], for answers that must name the server's own URL.
    pub fn serve_with(responses: impl FnOnce(&str) -> Vec<(u16, &'static str, Vec<u8>)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let responses = responses(&url);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for (status, content_type, body) in responses {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = line.split_once(':') {
                        headers.push((k.trim().to_string(), v.trim().to_string()));
                    }
                }
                let length = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let mut request_body = vec![0u8; length];
                reader.read_exact(&mut request_body).unwrap();
                log.lock().unwrap().push(Recorded {
                    request_line: request_line.trim_end().to_string(),
                    headers,
                    body: request_body,
                });
                let mut stream = stream;
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        Server { url, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(encode("rain & thunder"), "rain%20%26%20thunder");
        assert_eq!(encode("ü"), "%C3%BC");
    }

    #[test]
    fn urls_join_with_one_slash() {
        assert_eq!(join("https://a/v1/", "/models"), "https://a/v1/models");
        assert_eq!(join("https://a/v1", "models"), "https://a/v1/models");
    }

    #[test]
    fn the_user_agent_names_only_the_program() {
        assert!(USER_AGENT.starts_with("chukcut/"));
        assert!(!USER_AGENT.contains('@'));
    }

    #[test]
    fn a_multipart_body_has_every_part_and_a_terminator() {
        let (content_type, body) = Multipart::new()
            .text("model", "whisper-1")
            .file("file", "a.wav", "audio/wav", b"RIFF")
            .finish();
        let boundary = content_type.split("boundary=").nth(1).unwrap();
        let body = String::from_utf8(body).unwrap();
        assert!(body.contains("name=\"model\"\r\n\r\nwhisper-1\r\n"));
        assert!(body.contains("filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFF\r\n"));
        assert!(body.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[test]
    fn errors_use_the_providers_message_and_never_the_key() {
        let server = test_server::serve(vec![(
            401,
            "application/json",
            br#"{"error":{"message":"Incorrect API key provided: sk-abcdef"}}"#.to_vec(),
        )]);
        let error = agent()
            .get(&join(&server.url, "models"))
            .call()
            .unwrap_err();
        let message = describe(error, "sk-abcdef");
        assert!(message.contains("refused the API key"), "{message}");
        assert!(message.contains("Incorrect API key"), "{message}");
        assert!(!message.contains("sk-abcdef"), "{message}");
    }
}
