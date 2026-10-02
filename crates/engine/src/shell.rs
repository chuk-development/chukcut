//! The two things the command layer needs from whatever shell hosts it.
//!
//! The commands under `modules/*/commands.rs` were written against Tauri's
//! `spawn_blocking` and `ipc::Channel`. The engine no longer depends on Tauri,
//! so these are the same two shapes, implemented on plain threads and
//! callbacks. A native shell builds a [`Channel`] from a closure and awaits
//! the commands on its own executor.

use std::fmt;
use std::sync::Arc;

/// A one-way event stream from a command back to the shell.
///
/// The callback returns `false` once nobody is listening any more — a closed
/// panel, a cancelled view — which is how a long job learns to stop.
pub struct Channel<T> {
    sink: Arc<dyn Fn(T) -> bool + Send + Sync>,
}

impl<T> Clone for Channel<T> {
    fn clone(&self) -> Self {
        Self {
            sink: Arc::clone(&self.sink),
        }
    }
}

impl<T> Channel<T> {
    pub fn new(sink: impl Fn(T) -> bool + Send + Sync + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
        }
    }

    pub fn send(&self, value: T) -> Result<(), ChannelClosed> {
        if (self.sink)(value) {
            Ok(())
        } else {
            Err(ChannelClosed)
        }
    }
}

/// The listener behind a [`Channel`] has gone away.
#[derive(Debug, Clone, Copy)]
pub struct ChannelClosed;

impl fmt::Display for ChannelClosed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the channel's listener is gone")
    }
}

impl std::error::Error for ChannelClosed {}

/// The blocking task panicked instead of returning.
#[derive(Debug, Clone)]
pub struct TaskPanicked;

impl fmt::Display for TaskPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the blocking task panicked")
    }
}

impl std::error::Error for TaskPanicked {}

/// Run `work` on its own thread; the returned future resolves to its result.
///
/// Decoders, probes and file I/O block, and none of them may run on the UI
/// thread or on an async executor's workers. Like Tauri's version, the work
/// starts **now**, not when the future is first polled: a caller that only
/// wants the side effect may drop the future and the work still runs.
pub fn spawn_blocking<T, F>(
    work: F,
) -> impl std::future::Future<Output = Result<T, TaskPanicked>> + Send
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    async move { rx.await.map_err(|_| TaskPanicked) }
}
