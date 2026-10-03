//! The preview, as GPUI sees it.
//!
//! All the work is the engine's `preview::player::FramePlayer`: decode-ahead,
//! render-ahead during playback, a GPU swizzle to BGRA and an asynchronous
//! readback. This wrapper only turns its frames into `RenderImage`s, which is
//! the one step that needs `gpui`.
//!
//! The frames still leave the GPU and come back through GPUI's atlas, because
//! GPUI has no element that draws a texture of ours.
//! `docs/research/gpui-shared-texture.md` says what removing that takes.

use std::sync::Arc;

use chukcut_engine::modules::preview::player::{FramePlayer, PlayerRequest, PlayerStats};
use chukcut_engine::modules::project::{Micros, Project};
use gpui::RenderImage;

/// One rendered preview frame.
pub struct Frame {
    pub time: Micros,
    pub image: Arc<RenderImage>,
}

pub struct Player {
    inner: FramePlayer,
}

impl Player {
    pub fn new() -> Self {
        Self {
            inner: FramePlayer::new(),
        }
    }

    /// Say what should be on screen: the project after edit `generation`, at
    /// `time`, `size` device pixels big. `playing` is normal-speed playback on
    /// the audio clock, which renders ahead; anything else renders exactly
    /// `time`, newest request first.
    pub fn request(
        &self,
        project: Arc<Project>,
        generation: u64,
        time: Micros,
        size: (u32, u32),
        playing: bool,
    ) {
        self.inner.request(PlayerRequest {
            project,
            generation,
            time,
            size,
            playing,
        });
    }

    /// The frame to show now that the clock reads `clock`, if a new one is due.
    pub fn take(&self, clock: Micros) -> Option<Frame> {
        let frame = self.inner.take(clock)?;
        // Already BGRA, which is what `RenderImage` stores; the buffer is only
        // wrapped, never copied or swizzled here.
        let buffer = image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)?;
        Some(Frame {
            time: frame.time,
            image: Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])),
        })
    }

    /// Preview from proxy media where it exists.
    pub fn use_proxies(&self, on: bool) {
        self.inner.use_proxies(on);
    }

    pub fn failure(&self) -> Option<String> {
        self.inner.failure()
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> PlayerStats {
        self.inner.stats()
    }
}
