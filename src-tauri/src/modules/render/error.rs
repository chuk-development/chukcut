//! Failures the compositor can produce.
//!
//! These are engine faults, not user-facing prose. The command layer that
//! eventually exposes rendering over IPC is the place that turns them into the
//! `Result<T, String>` the frontend expects.

/// Everything that can go wrong between "give me a frame" and having one.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// No backend produced a usable adapter. On a headless box this usually
    /// means neither Vulkan nor a software rasterizer (lavapipe, llvmpipe) is
    /// installed.
    #[error("no GPU adapter is available: {0}")]
    NoAdapter(String),

    #[error("could not open a GPU device: {0}")]
    DeviceCreation(String),

    #[error("frame size must be non-zero, got {0}x{1}")]
    ZeroSize(u32, u32),

    /// Callers should clamp with [`super::RenderContext::clamp_size`] rather
    /// than hitting this.
    #[error("frame size {0}x{1} exceeds this device's {2}px texture limit")]
    FrameTooLarge(u32, u32, u32),

    #[error("reading the rendered frame back from the GPU failed: {0}")]
    Readback(String),

    /// The `SourceProvider` could not hand over a texture. Only surfaced when
    /// the compositor is configured to be strict about sources; otherwise the
    /// segment is skipped and a warning is logged.
    #[error("source for material {material_id} at {source_time}us failed: {source}")]
    Source {
        material_id: String,
        source_time: i64,
        #[source]
        source: anyhow::Error,
    },
}

pub type Result<T> = std::result::Result<T, RenderError>;
