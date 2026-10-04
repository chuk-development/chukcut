//! What the compositor keeps between frames for compound clips.
//!
//! A compound clip is its sequence rendered whole into a texture
//! (`Compositor::nested_frame`, decision 0024). Two things in that are worth
//! keeping:
//!
//! - **The nested view**, the document the nested render walks: the pool and
//!   the sequence's lanes. Building it clones the whole material pool, which
//!   in a long project is the larger part of the cost of a small compound
//!   clip. Kept per document fingerprint, so playback builds it once.
//! - **The nested frames.** The same sequence at the same inner instant and
//!   size is the same picture: two compound clips of one sequence on screen
//!   at once, and every re-render of a paused frame after an edit to the
//!   *outer* timeline (moving a title, grading the compound clip itself)
//!   reuse it instead of rendering the inside again.
//!
//! Both are keyed by `sequence::digest::digest` — a hash of the compound
//! clip's lanes and of every pool entry its clips name — so any edit that
//! could change the picture changes the key, and nothing has to be told to
//! invalidate. Frames are kept only for a provider that names itself
//! (`SourceProvider::cache_identity`): what a provider hands out is part of
//! the picture, and one that does not say when that changes cannot be
//! cached against.
//!
//! Bounded: a handful of views, and frames up to [`MAX_FRAME_BYTES`] of
//! texture memory, least recently used out first. Playback renders a new
//! inner instant every frame and never hits, so the bound is also what it
//! costs to keep the cache around while it does not help.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::modules::project::{Micros, Project};

/// Nested views kept: one per sequence a frame shows, for a few documents.
const MAX_VIEWS: usize = 16;

/// Nested frames kept, at most.
const MAX_FRAMES: usize = 24;

/// Texture memory nested frames may hold: about 24 frames at 1080p, 6 at 4K.
pub const MAX_FRAME_BYTES: u64 = 200 << 20;

struct View {
    key: u64,
    project: Arc<Project>,
}

struct Frame {
    key: u64,
    provider: u64,
    time: Micros,
    size: (u32, u32),
    texture: Arc<wgpu::Texture>,
    bytes: u64,
}

/// How often the cache answered, for the bench and the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NestedStats {
    pub frame_hits: u64,
    pub frame_misses: u64,
    pub view_hits: u64,
    pub view_misses: u64,
    /// Frames held now, and their texture bytes.
    pub frames: usize,
    pub bytes: u64,
}

#[derive(Default)]
pub(crate) struct NestedCache {
    views: VecDeque<View>,
    frames: VecDeque<Frame>,
    bytes: u64,
    stats: NestedStats,
}

impl NestedCache {
    /// The nested view for fingerprint `key`, most recently used last.
    pub fn view(&mut self, key: u64) -> Option<Arc<Project>> {
        let Some(at) = self.views.iter().position(|v| v.key == key) else {
            self.stats.view_misses += 1;
            return None;
        };
        self.stats.view_hits += 1;
        let view = self.views.remove(at).expect("found above");
        let project = Arc::clone(&view.project);
        self.views.push_back(view);
        Some(project)
    }

    pub fn put_view(&mut self, key: u64, project: Arc<Project>) {
        self.views.retain(|v| v.key != key);
        if self.views.len() >= MAX_VIEWS {
            self.views.pop_front();
        }
        self.views.push_back(View { key, project });
    }

    pub fn frame(
        &mut self,
        key: u64,
        provider: u64,
        time: Micros,
        size: (u32, u32),
    ) -> Option<Arc<wgpu::Texture>> {
        let at = self.frames.iter().position(|f| {
            f.key == key && f.provider == provider && f.time == time && f.size == size
        });
        let Some(at) = at else {
            self.stats.frame_misses += 1;
            return None;
        };
        self.stats.frame_hits += 1;
        let frame = self.frames.remove(at).expect("found above");
        let texture = Arc::clone(&frame.texture);
        self.frames.push_back(frame);
        Some(texture)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_frame(
        &mut self,
        key: u64,
        provider: u64,
        time: Micros,
        size: (u32, u32),
        texture: Arc<wgpu::Texture>,
        bytes: u64,
    ) {
        if bytes > MAX_FRAME_BYTES {
            return;
        }
        while !self.frames.is_empty()
            && (self.frames.len() >= MAX_FRAMES || self.bytes + bytes > MAX_FRAME_BYTES)
        {
            let old = self.frames.pop_front().expect("not empty");
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.frames.push_back(Frame {
            key,
            provider,
            time,
            size,
            texture,
            bytes,
        });
    }

    pub fn stats(&self) -> NestedStats {
        NestedStats {
            frames: self.frames.len(),
            bytes: self.bytes,
            ..self.stats
        }
    }

    pub fn clear(&mut self) {
        self.views.clear();
        self.frames.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_are_bounded_and_least_recently_used_goes_first() {
        let mut cache = NestedCache::default();
        let project = Arc::new(Project::new(
            "v",
            crate::modules::project::CanvasConfig::default(),
            30.0,
        ));
        for key in 0..MAX_VIEWS as u64 {
            cache.put_view(key, Arc::clone(&project));
        }
        // Touch the oldest so the second oldest is the one to go.
        assert!(cache.view(0).is_some());
        cache.put_view(1000, Arc::clone(&project));
        assert!(cache.view(0).is_some());
        assert!(cache.view(1).is_none());
        assert_eq!(cache.views.len(), MAX_VIEWS);
    }
}
