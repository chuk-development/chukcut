//! Feature modules.
//!
//! Every module here has a mirror directory under `src/modules/<name>/` in the
//! frontend. Rust owns the capability; the frontend owns the presentation of
//! it. Nothing in the webview touches the file system, a decoder, or the GPU
//! directly — it all comes through `#[tauri::command]` functions declared in
//! these modules and registered in `lib.rs`.

pub mod analysis;
pub mod animated;
pub mod audio;
pub mod audiofx;
pub mod body;
pub mod captions;
pub mod cloud;
pub mod compositing;
pub mod effects;
pub mod enhance;
pub mod export;
pub mod fx;
pub mod gpu;
pub mod grading;
pub mod inspector;
pub mod jobs;
pub mod keymap;
pub mod landmarks;
pub mod library;
pub mod loudness;
pub mod matting;
pub mod media;
pub mod ml;
pub mod motion;
pub mod prepare;
pub mod preview;
pub mod project;
pub mod proxy;
pub mod render;
pub mod sequence;
pub mod silence;
pub mod speech;
pub mod speed;
pub mod template;
pub mod text;
pub mod timeline;
pub mod tracking;
pub mod transitions;
pub mod voice;
pub mod workspace;
