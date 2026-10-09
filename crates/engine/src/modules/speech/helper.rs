//! Starting `chukcut-whisper-cuda` and talking to it.
//!
//! The helper is whisper.cpp with its CUDA backend, in a process of its own
//! (decision 0036): it links cudart and cuBLAS, so it cannot start on a
//! machine without them, and the editor must. One helper per transcription;
//! the messages are in [`chukcut_whisper::protocol`].
//!
//! Every way this can go wrong — no binary, no NVIDIA driver, missing CUDA
//! libraries, no usable GPU, a crash, an out-of-memory on the card — ends in
//! [`Attempt::Unavailable`] with a reason for the log, and the caller
//! transcribes on the CPU in its own process.

use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use chukcut_whisper::protocol::{self, Reply, Request, PROTOCOL_VERSION};
use chukcut_whisper::{Device, Transcription};
use parking_lot::Mutex;

/// The helper's file name, next to the editor's own.
pub const BINARY: &str = "chukcut-whisper-cuda";

/// How long the helper may take to say hello. Starting CUDA is a fraction of
/// a second; a driver that is still loading its firmware can take seconds.
const HELLO_TIMEOUT: Duration = Duration::from_secs(20);

/// The tail of the helper's stderr kept for the log: the dynamic loader's
/// "error while loading shared libraries" or a CUDA abort message.
const STDERR_TAIL: usize = 4096;

/// What came of trying the helper.
#[derive(Debug)]
pub enum Attempt {
    Done(Transcription, Device),
    Cancelled,
    /// The helper could not do it; the reason is for the log, and the caller
    /// transcribes on the CPU.
    Unavailable(String),
}

/// Where the helper is: `CHUKCUT_WHISPER_HELPER` if set (an empty value or
/// `off` switches it off), else where the ML worker would be — beside the
/// running executable, one up, in a package's `libexec/chukcut/` or
/// `lib/chukcut/` — else on `PATH`.
pub fn binary() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("CHUKCUT_WHISPER_HELPER") {
        let value = PathBuf::from(value);
        if value.as_os_str().is_empty() || value.as_os_str() == "off" {
            return None;
        }
        return value.is_file().then_some(value);
    }
    let exe = std::env::current_exe().ok()?;
    if let Some(found) = beside(exe.parent()?).into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|d| d.join(BINARY))
            .find(|p| p.is_file())
    })
}

/// The places beside an executable's directory the helper may sit, in the
/// order they are tried (the same layout as the ML worker's).
pub fn beside(exe_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![exe_dir.join(BINARY)];
    if let Some(up) = exe_dir.parent() {
        candidates.push(up.join(BINARY));
        candidates.push(up.join("libexec").join("chukcut").join(BINARY));
        candidates.push(up.join("lib").join("chukcut").join(BINARY));
    }
    candidates
}

/// Whether trying the helper makes sense here: it exists and an NVIDIA
/// driver is loaded. Without the driver the helper would start, find no GPU
/// and say so, which costs a process for nothing.
pub fn worth_trying() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let binary = binary()?;
    chukcut_ml_worker::registry::nvidia_driver().map(|_| binary)
}

/// The `lib/` directories of chukcut's own NVIDIA library packs (the CUDA 12
/// and 13 bundles the ML settings install), for a helper whose CUDA
/// libraries the system does not have. The dynamic loader picks the soname
/// the helper was linked against from whichever bundle has it.
fn bundle_library_dirs() -> Vec<PathBuf> {
    use chukcut_ml_worker::registry::{runtime_dir, runtime_present, PackKind, RUNTIME_PACKS};
    let root = crate::modules::ml::root();
    RUNTIME_PACKS
        .iter()
        .filter(|p| p.kind == PackKind::Libraries && runtime_present(&root, p))
        .map(|p| runtime_dir(&root, p).join("lib"))
        .collect()
}

/// A started helper that has said hello.
struct Running {
    child: Child,
    replies: mpsc::Receiver<Option<Reply>>,
    stderr: Arc<Mutex<String>>,
    device: Device,
}

impl Running {
    /// Kill and reap the helper.
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        crate::modules::diag::child::unregister(self.child.id());
    }

    /// What the helper said on stderr, trimmed for a log line.
    fn stderr(&self) -> String {
        self.stderr.lock().trim().to_string()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the helper and wait for its hello. With `library_dirs`, they are
/// put in front of `LD_LIBRARY_PATH`.
fn start(binary: &Path, probe: bool, library_dirs: &[PathBuf]) -> Result<Running, String> {
    let mut command = Command::new(binary);
    if probe {
        command.arg("--probe");
    }
    if !library_dirs.is_empty() {
        let mut dirs = library_dirs.to_vec();
        if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
            dirs.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(dirs) {
            command.env("LD_LIBRARY_PATH", joined);
        }
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{} could not be started: {e}", binary.display()))?;

    let stdout = child.stdout.take().expect("piped");
    let (send, replies) = mpsc::channel();
    std::thread::Builder::new()
        .name("whisper-helper-out".into())
        .spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                match protocol::read_reply(&mut stdout) {
                    Ok(Some(reply)) => {
                        if send.send(Some(reply)).is_err() {
                            return;
                        }
                    }
                    // A garbled line is as good as a crash: stop listening.
                    Ok(None) | Err(_) => {
                        let _ = send.send(None);
                        return;
                    }
                }
            }
        })
        .map_err(|e| format!("a thread could not be started: {e}"))?;

    let stderr = Arc::new(Mutex::new(String::new()));
    {
        let mut pipe = child.stderr.take().expect("piped");
        let stderr = Arc::clone(&stderr);
        std::thread::Builder::new()
            .name("whisper-helper-err".into())
            .spawn(move || {
                let mut buffer = [0u8; 1024];
                while let Ok(n) = pipe.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    let mut text = stderr.lock();
                    text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                    if text.len() > STDERR_TAIL {
                        let mut cut = text.len() - STDERR_TAIL;
                        while !text.is_char_boundary(cut) {
                            cut += 1;
                        }
                        text.drain(..cut);
                    }
                }
            })
            .map_err(|e| format!("a thread could not be started: {e}"))?;
    }

    // The resource sampler reports its memory and CPU next to the app's.
    // Its stderr is kept here rather than forwarded to the log: the tail is
    // the reason a fallback gives, and it goes into the log with that.
    crate::modules::diag::child::register("whisper", child.id());
    let mut running = Running {
        child,
        replies,
        stderr,
        device: Device::cpu(),
    };
    match running.replies.recv_timeout(HELLO_TIMEOUT) {
        Ok(Some(Reply::Hello { protocol, device })) => {
            if protocol != PROTOCOL_VERSION {
                return Err(format!(
                    "it speaks protocol {protocol}, this editor {PROTOCOL_VERSION}"
                ));
            }
            running.device = device;
            Ok(running)
        }
        Ok(Some(other)) => Err(format!("it began with {other:?} instead of hello")),
        Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Give the stderr thread a moment to read the loader's message.
            let _ = running.child.wait();
            std::thread::sleep(Duration::from_millis(20));
            Err(format!("it exited at start: {}", running.stderr()))
        }
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "it did not answer within {} s",
            HELLO_TIMEOUT.as_secs()
        )),
    }
}

/// Start the helper; when the dynamic loader cannot find its CUDA libraries,
/// once more with chukcut's own NVIDIA bundles on the library path.
fn start_with_fallback(binary: &Path, probe: bool) -> Result<Running, String> {
    match start(binary, probe, &[]) {
        Ok(running) => Ok(running),
        Err(why) if why.contains("error while loading shared libraries") => {
            let dirs = bundle_library_dirs();
            if dirs.is_empty() {
                return Err(why);
            }
            start(binary, probe, &dirs)
                .map_err(|again| format!("{why}; with chukcut's CUDA libraries: {again}"))
        }
        Err(why) => Err(why),
    }
}

/// Ask the helper which device it would use, without loading a model.
/// `None` when it is not here or cannot start.
pub fn probe() -> Result<Device, String> {
    let binary = worth_trying().ok_or("no CUDA helper or no NVIDIA driver")?;
    let running = start_with_fallback(&binary, true)?;
    Ok(running.device.clone())
}

/// Transcribe on the helper's GPU. `progress` gets the device and 0..1.
pub fn transcribe(
    binary: &Path,
    request: Request,
    samples: &[f32],
    progress: &(dyn Fn(&Device, f32) + Sync),
    cancel: &AtomicBool,
) -> Attempt {
    let mut running = match start_with_fallback(binary, false) {
        Ok(running) => running,
        Err(why) => return Attempt::Unavailable(why),
    };
    if !running.device.gpu {
        // It started but CUDA found no device it can use; the editor's own
        // CPU path is the same work without a process in between.
        return Attempt::Unavailable("the CUDA backend found no usable GPU".into());
    }
    let device = running.device.clone();
    progress(&device, 0.0);

    // The samples go in from a thread of their own: a long timeline is tens
    // of megabytes, more than the pipe holds, and the helper only reads them
    // after the request line, while this thread must keep watching `cancel`.
    let mut stdin = running.child.stdin.take().expect("piped");
    let outcome = std::thread::scope(|scope| {
        scope.spawn(move || {
            // A helper that died mid-read is reported by the reply loop.
            let _ = protocol::write_request(&mut stdin, &request, samples);
        });
        loop {
            if cancel.load(Ordering::Relaxed) {
                // Killing closes the pipe, which ends the writer above.
                running.stop();
                return Attempt::Cancelled;
            }
            match running.replies.recv_timeout(Duration::from_millis(100)) {
                Ok(Some(Reply::Progress { fraction })) => progress(&device, fraction),
                Ok(Some(Reply::Done { transcription })) => {
                    return Attempt::Done(transcription, device.clone())
                }
                Ok(Some(Reply::Error { message })) => {
                    running.stop();
                    return Attempt::Unavailable(format!("it failed: {message}"));
                }
                Ok(Some(Reply::Hello { .. })) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Reap first, so the stderr thread has seen everything.
                    let status = running.child.wait();
                    std::thread::sleep(Duration::from_millis(20));
                    return Attempt::Unavailable(format!(
                        "it stopped ({}): {}",
                        status.map(|s| s.to_string()).unwrap_or_default(),
                        running.stderr()
                    ));
                }
            }
        }
    });
    if let Attempt::Done(..) = outcome {
        let _ = running.child.wait();
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helper_is_looked_for_where_the_ml_worker_is() {
        let dirs = beside(Path::new("/usr/bin"));
        assert_eq!(dirs[0], Path::new("/usr/bin/chukcut-whisper-cuda"));
        assert!(dirs.contains(&PathBuf::from("/usr/libexec/chukcut/chukcut-whisper-cuda")));
    }

    /// A helper that is not a helper — here `sh` printing garbage, then one
    /// that exits at once — is reported, never waited on forever, and never
    /// turns into a transcription.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_broken_helper_is_unavailable_not_a_hang() {
        let dir =
            std::env::temp_dir().join(format!("chukcut-whisper-helper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, body: &str| {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        };
        let garbage = write("garbage", "#!/bin/sh\necho 'not json'\n");
        let dies = write(
            "dies",
            "#!/bin/sh\necho 'error while loading shared libraries: libcublas.so.12' >&2\nexit 127\n",
        );
        let wrong_version = write(
            "old",
            "#!/bin/sh\necho '{\"type\":\"hello\",\"protocol\":0,\"device\":{\"gpu\":true,\"backend\":\"CUDA\",\"name\":\"x\"}}'\n",
        );
        let request = || Request {
            model: PathBuf::from("/nonexistent"),
            language: None,
            samples: 4,
        };
        let cancel = AtomicBool::new(false);
        for (binary, expect) in [
            (&garbage, "exited at start"),
            (&dies, "libcublas.so.12"),
            (&wrong_version, "protocol 0"),
        ] {
            match transcribe(binary, request(), &[0.0; 4], &|_, _| {}, &cancel) {
                Attempt::Unavailable(why) => assert!(why.contains(expect), "{why}"),
                other => panic!("{binary:?}: {other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A helper that says hello on a GPU and then answers is used, and its
    /// progress reaches the caller with the device.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_working_helper_answers_with_its_device() {
        let dir = std::env::temp_dir().join(format!("chukcut-whisper-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("helper");
        // Reads the request line and the 16 payload bytes, then answers.
        std::fs::write(
            &path,
            "#!/bin/sh\n\
             echo '{\"type\":\"hello\",\"protocol\":1,\"device\":{\"gpu\":true,\"backend\":\"CUDA\",\"name\":\"Test GPU\"}}'\n\
             head -n 1 >/dev/null; head -c 16 >/dev/null\n\
             echo '{\"type\":\"progress\",\"fraction\":0.5}'\n\
             echo '{\"type\":\"done\",\"transcription\":{\"language\":\"en\",\"words\":[{\"text\":\"hi\",\"start\":0,\"end\":10000}]}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let seen = std::sync::Mutex::new(Vec::new());
        let request = Request {
            model: PathBuf::from("/m"),
            language: None,
            samples: 4,
        };
        let attempt = transcribe(
            &path,
            request,
            &[0.0; 4],
            &|device, fraction| seen.lock().unwrap().push((device.name.clone(), fraction)),
            &AtomicBool::new(false),
        );
        let _ = std::fs::remove_dir_all(&dir);
        match attempt {
            Attempt::Done(transcription, device) => {
                assert_eq!(transcription.words[0].text, "hi");
                assert!(device.gpu);
                assert_eq!(device.name, "Test GPU");
            }
            other => panic!("{other:?}"),
        }
        assert!(seen
            .lock()
            .unwrap()
            .contains(&("Test GPU".to_string(), 0.5)));
    }
}
