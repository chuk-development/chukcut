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

use std::io::Write;

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
    // SAFETY: `_exit` has no preconditions. It ends every thread of the
    // process at once, which is the point: nothing runs after it.
    unsafe { libc::_exit(i32::from(code)) }
}
