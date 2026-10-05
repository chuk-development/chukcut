//! Leaving the process.
//!
//! A normal return from `main` calls libc's `exit`, which runs the `atexit`
//! handlers and the destructors of every loaded library (`_dl_fini`) while
//! the process's other threads keep running. The GPU driver is one of those
//! libraries. A thread that still frees a texture, polls the device or tears
//! down a decoder at that moment calls into a driver that is being unloaded
//! under it: `chukcut-cli export` died that way once with SIGSEGV in
//! `libnvidia-glcore` on its export thread, after the file was written
//! (docs/STATUS.md, "The crash after the export").
//!
//! Every shell therefore leaves through [`exit`]: it stops what it can stop
//! in order — the exports, then the ML worker process — flushes its own
//! output, and ends the process with `_exit`, which runs no library
//! destructors at all. The kernel releases the device, the memory and the
//! files as it does for any process. Nothing the engine keeps needs a
//! destructor at exit: the log file and the export queue file are written
//! unbuffered, the project is saved by its own command, and the shared wgpu
//! device lives in a static that Rust never drops.
//!
//! ## Panics
//!
//! A panic is a bug, and the policy for one (decision 0034) is: **write it
//! down, contain it where it happened, tell the user, keep the process.**
//! [`install_panic_hook`] puts every panic — message, thread, place and a
//! backtrace — in the log file, which a launcher-started app otherwise never
//! has: stderr goes nowhere. [`contain`] and [`contained`] are how a job
//! thread turns a panic into the failure it reports; the places that must
//! never die with a bug (the player, an export, the export queue, every bake
//! and analysis job, the MCP server) run their work through them.

use std::any::Any;
use std::io::Write;
use std::sync::Once;

/// Stop the engine's background work that must not be cut off mid-way: the
/// exports (each waits until its decoders, textures and encoder are gone)
/// and the ML worker process (asked to stop, killed after two seconds).
pub fn quiesce() {
    crate::modules::export::commands::export_shutdown();
    crate::modules::ml::worker::shutdown();
}

/// [`quiesce`], flush stdout and stderr, and end the process with `code`
/// without running any library destructor.
pub fn exit(code: u8) -> ! {
    quiesce();
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    end(code)
}

#[cfg(unix)]
fn end(code: u8) -> ! {
    // SAFETY: `_exit` has no preconditions. It ends every thread of the
    // process at once, which is the point: nothing runs after it.
    unsafe { libc::_exit(i32::from(code)) }
}

/// Windows builds are experimental (docs/decisions/0033-release-builds.md).
/// `std::process::exit` runs the DLL detach handlers that `_exit` skips on
/// Unix; if that brings the driver crash back there, `TerminateProcess` on
/// the current process is the equivalent.
#[cfg(not(unix))]
fn end(code: u8) -> ! {
    std::process::exit(i32::from(code))
}

// ---------------------------------------------------------------------------
// Panics
// ---------------------------------------------------------------------------

/// Write every panic to the log file — message, thread, place, backtrace —
/// then run the hook that was there before (the default prints to stderr).
/// Once per process; later calls do nothing.
pub fn install_panic_hook() {
    install_panic_hook_with(|report| {
        crate::modules::workspace::logging::write_raw(report);
    });
}

/// [`install_panic_hook`] with the report going to `sink`. For a process
/// whose log is not the engine's file, and for the test.
pub fn install_panic_hook_with(sink: impl Fn(&str) + Send + Sync + 'static) {
    static ONCE: Once = Once::new();
    let mut sink = Some(sink);
    ONCE.call_once(|| {
        let sink = sink.take().expect("once");
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("unnamed");
            let place = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "an unknown place".into());
            let message = panic_message(info.payload());
            // Captured whatever RUST_BACKTRACE says: the log is read after
            // the fact, when nobody can run it again with the variable set.
            let backtrace = std::backtrace::Backtrace::force_capture();
            sink(&format!(
                "PANIC in thread '{name}' at {place}: {message}\nbacktrace:\n{backtrace}\n"
            ));
            previous(info);
        }));
    });
}

/// The text of a panic payload: what `panic!` was given, when it was a
/// string.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic without a message".into()
    }
}

/// Run `work`, turning a panic into an error that says where: "<what> stopped
/// on an internal error: <message>". The panic itself is already in the log
/// (the hook); the error is what the caller shows the user.
///
/// `AssertUnwindSafe` because every caller discards the state the work was
/// building when it fails — a job's partial output is thrown away, as on any
/// other failure.
pub fn contain<T>(what: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|payload| {
        let message = panic_message(payload.as_ref());
        tracing::error!(%what, %message, "contained a panic");
        format!("{what} stopped on an internal error: {message}. The log has the details.")
    })
}

/// [`contain`] for work that already answers `Result<T, String>`.
pub fn contained<T>(what: &str, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    contain(what, work).and_then(|result| result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contained_panic_becomes_an_error_that_names_the_job() {
        let result: Result<u32, String> = contain("The test job", || panic!("boom {}", 7));
        let error = result.unwrap_err();
        assert!(error.starts_with("The test job stopped"), "{error}");
        assert!(error.contains("boom 7"), "{error}");
        assert_eq!(contained("x", || Ok::<_, String>(3)), Ok(3));
        assert_eq!(
            contained::<u8>("x", || Err("plain".into())),
            Err("plain".into())
        );
    }

    #[test]
    fn the_hook_reports_message_thread_place_and_backtrace() {
        static SEEN: parking_lot::Mutex<String> = parking_lot::Mutex::new(String::new());
        install_panic_hook_with(|report| SEEN.lock().push_str(report));
        let _ = std::thread::Builder::new()
            .name("hook-test".into())
            .spawn(|| panic!("reported panic"))
            .unwrap()
            .join();
        let seen = SEEN.lock().clone();
        // Another test may have installed the hook first; then only that
        // it ran is checkable here.
        if seen.is_empty() {
            return;
        }
        assert!(seen.contains("reported panic"), "{seen}");
        assert!(seen.contains("'hook-test'"), "{seen}");
        assert!(seen.contains("lifecycle.rs"), "{seen}");
        assert!(seen.contains("backtrace:"), "{seen}");
    }
}
