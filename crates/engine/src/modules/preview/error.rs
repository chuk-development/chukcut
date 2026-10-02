//! What can go wrong between the compositor and the webview.
//!
//! The `Display` strings are written to be shown to a user unchanged — the IPC
//! layer only calls `to_string()` on these, and the protocol handler puts them
//! in the body of a 4xx response.

use crate::modules::render::RenderError;

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("this machine has no GPU the preview can render on")]
    NoDevice,

    #[error("no preview session is running")]
    NoSession,

    #[error("a preview frame cannot be {width}x{height}")]
    FrameSize { width: u32, height: u32 },

    #[error("frame data is {got} bytes, expected {want}")]
    FrameData { got: usize, want: usize },

    #[error("cannot encode the preview frame: {0}")]
    Encode(String),

    #[error(transparent)]
    Render(#[from] RenderError),
}

pub type Result<T> = std::result::Result<T, PreviewError>;
