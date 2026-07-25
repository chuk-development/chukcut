//! Recycling of GPU textures.
//!
//! A compositor that allocates a fresh texture per clip per frame spends its
//! time in the driver's allocator instead of in the rasterizer, and the stutter
//! that produces looks exactly like a slow renderer. Frames are the same size
//! for minutes at a time, so almost every allocation is a repeat of one we just
//! freed.
//!
//! The pool is keyed by everything that makes a texture non-interchangeable:
//! size, format *and* usage. Usage is part of the key because a texture created
//! without `COPY_SRC` cannot be read back, and handing one out for a readback
//! target would fail validation at draw time rather than here.

use parking_lot::Mutex;
use std::collections::HashMap;

/// The dimensions of interchangeability. Two textures with the same key can be
/// swapped for one another without the caller noticing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub usage: wgpu::TextureUsages,
}

impl TextureKey {
    pub fn new(
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Self {
        Self {
            width,
            height,
            format,
            usage,
        }
    }

    /// Bytes this texture occupies, ignoring driver padding and mips. Good
    /// enough to keep a budget honest; not an allocator.
    pub fn size_bytes(&self) -> u64 {
        let (bw, bh) = self.format.block_dimensions();
        let block = self.format.block_copy_size(None).unwrap_or(4) as u64;
        let cols = self.width.div_ceil(bw) as u64;
        let rows = self.height.div_ceil(bh) as u64;
        cols * rows * block
    }
}

/// A texture on loan from a [`TexturePool`].
///
/// Dropping one is legal — it just frees the texture instead of returning it,
/// which is the right behaviour on an error path. Give it back with
/// [`TexturePool::release`] on the happy path.
pub struct PooledTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    key: TextureKey,
}

impl PooledTexture {
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn key(&self) -> TextureKey {
        self.key
    }

    pub fn width(&self) -> u32 {
        self.key.width
    }

    pub fn height(&self) -> u32 {
        self.key.height
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.key.format
    }
}

impl std::fmt::Debug for PooledTexture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledTexture").field("key", &self.key).finish()
    }
}

/// What the pool is doing, for logging and for the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolStats {
    /// Textures handed out from the free list rather than allocated.
    pub hits: u64,
    /// Textures the driver had to create.
    pub misses: u64,
    /// Textures dropped on release because the budget was full.
    pub evictions: u64,
    /// Bytes currently sitting idle in the free list.
    pub retained_bytes: u64,
    /// Textures currently sitting idle in the free list.
    pub retained_count: usize,
}

/// 256 MiB of idle textures. A 4K RGBA frame is 33 MB, so this holds a handful
/// of render targets plus the source textures of a busy composite without
/// growing without bound on a project that keeps changing resolution.
pub const DEFAULT_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

/// Recycles textures by [`TextureKey`], up to a byte budget.
pub struct TexturePool {
    inner: Mutex<Inner>,
    budget_bytes: u64,
}

struct Inner {
    free: HashMap<TextureKey, Vec<(wgpu::Texture, wgpu::TextureView)>>,
    retained_bytes: u64,
    retained_count: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl Default for TexturePool {
    fn default() -> Self {
        Self::new(DEFAULT_BUDGET_BYTES)
    }
}

impl TexturePool {
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            inner: Mutex::new(Inner {
                free: HashMap::new(),
                retained_bytes: 0,
                retained_count: 0,
                hits: 0,
                misses: 0,
                evictions: 0,
            }),
            budget_bytes,
        }
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget_bytes
    }

    /// Take a texture matching `key`, recycling one if we have it.
    ///
    /// The contents are undefined — a recycled texture still holds the previous
    /// frame. Every consumer either clears it (render target) or overwrites it
    /// wholesale (upload), so we do not pay to zero it.
    pub fn acquire(&self, device: &wgpu::Device, key: TextureKey) -> PooledTexture {
        {
            let mut inner = self.inner.lock();
            if let Some(bucket) = inner.free.get_mut(&key) {
                if let Some((texture, view)) = bucket.pop() {
                    inner.hits += 1;
                    inner.retained_count -= 1;
                    inner.retained_bytes = inner.retained_bytes.saturating_sub(key.size_bytes());
                    return PooledTexture { texture, view, key };
                }
            }
            inner.misses += 1;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("chukcut pooled texture"),
            size: wgpu::Extent3d {
                width: key.width,
                height: key.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: key.format,
            usage: key.usage,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        PooledTexture { texture, view, key }
    }

    /// Hand a texture back. Over budget, it is dropped instead of retained.
    pub fn release(&self, texture: PooledTexture) {
        let key = texture.key;
        let size = key.size_bytes();
        let mut inner = self.inner.lock();

        if inner.retained_bytes + size > self.budget_bytes {
            inner.evictions += 1;
            return;
        }

        inner.retained_bytes += size;
        inner.retained_count += 1;
        inner
            .free
            .entry(key)
            .or_default()
            .push((texture.texture, texture.view));
    }

    /// Drop every idle texture. Called when the canvas resolution changes, or
    /// when the app wants its VRAM back.
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.free.clear();
        inner.retained_bytes = 0;
        inner.retained_count = 0;
    }

    pub fn retained_bytes(&self) -> u64 {
        self.inner.lock().retained_bytes
    }

    pub fn stats(&self) -> PoolStats {
        let inner = self.inner.lock();
        PoolStats {
            hits: inner.hits,
            misses: inner.misses,
            evictions: inner.evictions,
            retained_bytes: inner.retained_bytes,
            retained_count: inner.retained_count,
        }
    }
}

impl std::fmt::Debug for TexturePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TexturePool")
            .field("budget_bytes", &self.budget_bytes)
            .field("stats", &self.stats())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::render::RenderContext;

    const RGBA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
    const USAGE: wgpu::TextureUsages =
        wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::COPY_SRC);

    #[test]
    fn key_size_matches_rgba8_math() {
        let key = TextureKey::new(1920, 1080, RGBA, USAGE);
        assert_eq!(key.size_bytes(), 1920 * 1080 * 4);
    }

    #[test]
    fn differing_usage_is_a_different_bucket() {
        let a = TextureKey::new(64, 64, RGBA, wgpu::TextureUsages::COPY_SRC);
        let b = TextureKey::new(64, 64, RGBA, wgpu::TextureUsages::COPY_DST);
        assert_ne!(a, b);
    }

    #[test]
    fn released_textures_are_recycled() {
        let Some(ctx) = RenderContext::try_new() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let pool = TexturePool::default();
        let key = TextureKey::new(320, 180, RGBA, USAGE);

        let first = pool.acquire(ctx.device(), key);
        pool.release(first);
        assert_eq!(pool.retained_bytes(), key.size_bytes());

        let second = pool.acquire(ctx.device(), key);
        let stats = pool.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(pool.retained_bytes(), 0);
        pool.release(second);
    }

    #[test]
    fn budget_caps_what_is_retained() {
        let Some(ctx) = RenderContext::try_new() else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let key = TextureKey::new(256, 256, RGBA, USAGE);
        // Room for exactly one.
        let pool = TexturePool::new(key.size_bytes());

        let a = pool.acquire(ctx.device(), key);
        let b = pool.acquire(ctx.device(), key);
        pool.release(a);
        pool.release(b);

        assert_eq!(pool.stats().retained_count, 1);
        assert_eq!(pool.stats().evictions, 1);

        pool.clear();
        assert_eq!(pool.retained_bytes(), 0);
    }
}
