//! Feature modules.
//!
//! Every module here has a mirror directory under `src/modules/<name>/` in the
//! frontend. Rust owns the capability; the frontend owns the presentation of
//! it. Nothing in the webview touches the file system, a decoder, or the GPU
//! directly — it all comes through `#[tauri::command]` functions declared in
//! these modules and registered in `lib.rs`.

pub mod audio;
pub mod captions;
pub mod cloud;
pub mod effects;
pub mod export;
pub mod gpu;
pub mod inspector;
pub mod media;
pub mod motion;
pub mod preview;
pub mod project;
pub mod proxy;
pub mod render;
pub mod speech;
pub mod text;
pub mod timeline;
pub mod transitions;
pub mod workspace;
