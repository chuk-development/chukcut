//! The library's network: one fetch with a disk copy behind it.
//!
//! Every catalogue goes through [`cached`]: a copy younger than its time to
//! live answers without a request; an older one is refreshed when the
//! network answers and used as it is when it does not. So the panel keeps
//! working offline once it has been opened online, and a failure the user
//! sees is a sentence that says what could not be reached and that what was
//! already downloaded still works — never a panic, never an empty grid
//! without a reason.
//!
//! Requests carry `User-Agent: chukcut/<version>` and nothing else about the
//! person ([`crate::modules::cloud::http::USER_AGENT`]).

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::modules::cloud::http;

/// A catalogue answer and whether it came from an old copy because the
/// network did not answer.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub stale: bool,
}

/// A client for small requests: quick to give up, so a dead network turns
/// into a message in seconds rather than minutes.
pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(http::USER_AGENT)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .build()
}

/// What the user reads when `what` could not be fetched.
pub fn unreachable(what: &str, error: &str) -> String {
    format!("Could not download {what} ({error}). Check the connection; everything already downloaded still works offline.")
}

/// `url`'s body, up to `cloud::http::MAX_BODY`.
pub fn get(url: &str) -> Result<Vec<u8>, String> {
    let response = agent().get(url).call().map_err(|e| http::describe(e, ""))?;
    http::read_bytes(response)
}

/// `url` into `dest`, through a `.part` file, so a broken download never
/// looks finished.
pub fn download(url: &str, dest: &Path) -> Result<u64, String> {
    http::download(url, dest, &|_| {}, &AtomicBool::new(false))
}

fn age(path: &Path) -> Option<Duration> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
}

/// `url` through the copy at `path`; see the module docs. `what` names the
/// thing for the error message ("the font list").
pub fn cached(url: &str, path: &Path, ttl: Duration, what: &str) -> Result<Fetched, String> {
    let fresh = age(path).is_some_and(|age| age < ttl);
    if fresh {
        if let Ok(bytes) = std::fs::read(path) {
            return Ok(Fetched {
                bytes,
                stale: false,
            });
        }
    }
    match get(url) {
        Ok(bytes) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let part = path.with_extension("part");
            if std::fs::write(&part, &bytes).is_ok() {
                let _ = std::fs::rename(&part, path);
            }
            Ok(Fetched {
                bytes,
                stale: false,
            })
        }
        Err(error) => match std::fs::read(path) {
            Ok(bytes) => {
                tracing::warn!(%error, %url, "using the stored copy");
                Ok(Fetched { bytes, stale: true })
            }
            Err(_) => Err(unreachable(what, &error)),
        },
    }
}

/// A file fetched once into `path` and kept: a thumbnail, a font subset.
pub fn once(url: &str, path: &Path, what: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    download(url, path)
        .map(|_| ())
        .map_err(|error| unreachable(what, &error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cloud::http::test_server;

    #[test]
    fn a_catalogue_survives_the_network_going_away() {
        let dir = super::super::scratch("net");
        let path = dir.join("list.json");
        let server = test_server::serve(vec![(200, "application/json", b"[1]".to_vec())]);
        let url = format!("{}/list.json", server.url);
        let first = cached(&url, &path, Duration::from_secs(3600), "the list").unwrap();
        assert_eq!(first.bytes, b"[1]");
        assert!(!first.stale);
        // Fresh: answered from disk, the server has nothing more to say.
        let again = cached(&url, &path, Duration::from_secs(3600), "the list").unwrap();
        assert_eq!(again.bytes, b"[1]");
        // Expired and the server gone: the old copy, marked stale.
        let stale = cached(&url, &path, Duration::ZERO, "the list").unwrap();
        assert!(stale.stale);
        assert_eq!(stale.bytes, b"[1]");
        // Nothing on disk and no server: a message, not a panic.
        let error = cached(&url, &dir.join("other.json"), Duration::ZERO, "the list").unwrap_err();
        assert!(error.starts_with("Could not download the list"), "{error}");
    }

    #[test]
    fn requests_name_only_the_program() {
        let server = test_server::serve(vec![(200, "text/plain", b"ok".to_vec())]);
        get(&format!("{}/x", server.url)).unwrap();
        let recorded = server.requests.lock().unwrap();
        let agent = recorded[0].header("user-agent").unwrap();
        assert!(agent.starts_with("chukcut/"), "{agent}");
        assert!(recorded[0].header("from").is_none());
    }
}
