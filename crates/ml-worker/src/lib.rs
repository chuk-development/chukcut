//! The chukcut ML worker, as a library: the wire protocol and the model
//! registry the engine shares with the worker process, and the model-specific
//! pre- and post-processing.
//!
//! Without the `runtime` feature this crate is a few hundred lines of plain
//! Rust with serde as its only dependency. That is how the engine uses it:
//! the editor speaks the protocol and downloads models, but never links ONNX
//! Runtime. The `chukcut-ml-worker` binary turns `runtime` on.
//!
//! Design: `docs/decisions/0025-ml-worker-process.md`.

pub mod birefnet;
pub mod esrgan;
pub mod lama;
pub mod protocol;
pub mod registry;
pub mod rife;
pub mod rvm;
pub mod sam;
pub mod vittrack;
pub mod yunet;

#[cfg(feature = "runtime")]
pub mod runtime;
