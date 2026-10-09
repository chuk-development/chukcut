//! `chukcut-whisper-cuda`: whisper.cpp on the GPU, in a process of its own.
//!
//! ```text
//! chukcut-whisper-cuda           say hello, read one request on stdin, answer, exit
//! chukcut-whisper-cuda --probe   say hello (which device) and exit
//! ```
//!
//! The editor starts it (`chukcut-engine`, `modules/speech/helper.rs`); the
//! messages are in [`chukcut_whisper::protocol`]. It links cudart and cuBLAS,
//! so on a machine without them the dynamic loader refuses to start it and
//! the editor transcribes on the CPU instead. Nothing is written to stdout but
//! protocol lines; whisper.cpp's own logging goes nowhere.

use std::io::{self, BufReader};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use chukcut_whisper::protocol::{self, Reply, PROTOCOL_VERSION};

fn main() {
    // Before anything touches ggml: the device query below initialises the
    // CUDA backend, which logs through the same hooks.
    whisper_rs::install_logging_hooks();

    let stdout = Mutex::new(io::stdout());
    let send = |reply: &Reply| {
        let mut out = stdout.lock().unwrap_or_else(|e| e.into_inner());
        // A closed stdout means the editor is gone; there is no one to tell.
        let _ = protocol::write_reply(&mut *out, reply);
    };

    let device = chukcut_whisper::run::device();
    send(&Reply::Hello {
        protocol: PROTOCOL_VERSION,
        device: device.clone(),
    });
    if std::env::args().any(|a| a == "--probe") {
        return;
    }

    let (request, samples) = match protocol::read_request(&mut BufReader::new(io::stdin().lock())) {
        Ok(read) => read,
        Err(e) => {
            send(&Reply::Error {
                message: format!("the request could not be read: {e}"),
            });
            std::process::exit(2);
        }
    };

    // Cancelling is the editor killing this process, so nothing sets it.
    let cancel = AtomicBool::new(false);
    let last = Mutex::new(-1i32);
    let result = chukcut_whisper::run::transcribe(
        &request.model,
        &samples,
        request.language.as_deref(),
        device.gpu,
        &|fraction| {
            // whisper.cpp reports whole percents; send each one once.
            let percent = (fraction * 100.0).round() as i32;
            let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
            if percent != *last {
                *last = percent;
                send(&Reply::Progress { fraction });
            }
        },
        &cancel,
    );
    match result {
        Ok(transcription) => send(&Reply::Done { transcription }),
        Err(message) => {
            send(&Reply::Error { message });
            std::process::exit(1);
        }
    }
}
