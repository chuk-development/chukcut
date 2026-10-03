//! Text and titles: a `TextMaterial` becomes an RGBA image.
//!
//! Titles are the most-used feature of a short-form editor. This module is the
//! part of that feature that has to be right before any of the visible part can
//! exist: given the document's description of a text layer, produce the pixels,
//! and produce them fast enough to do it every frame if we have to — which we
//! do not, because we cache.
//!
//! ```text
//!   TextMaterial ──► TextRequest ──► parley ──► TextLayout ──► RasteredText
//!                                   (shaping,    (owned,        (RGBA8 +
//!                                    breaking,     Send)          layout)
//!                                    bidi)
//! ```
//!
//! ## What it owns
//!
//! - **Font resolution.** `fontique` enumerates the system's fonts; a family
//!   the machine does not have falls back instead of failing. See [`font`].
//! - **Shaping and line breaking.** `parley`, and `harfrust` underneath it.
//!   Ligatures, kerning, mark attachment, Arabic joining and bidi reordering are
//!   all theirs, which is the entire reason for the dependency: hand-rolling any
//!   of them produces an editor that works until somebody types in their own
//!   language. It costs 0.1 ms for a title; see
//!   `docs/research/text-rendering.md`.
//! - **Everything above shaping.** Outline fills, strokes, drop shadows,
//!   background boxes and colour-bitmap emoji are written here, because no Rust
//!   crate does them. See [`raster`].
//! - **A cache keyed on content.** A title does not change between frames, so
//!   it is rasterised once and held; see `cache.rs`.
//! - **Placing a title on the timeline.** How long a new title is, which lane
//!   it lands on, what happens when that instant is taken — see [`edit`], which
//!   is pure, and [`commands`], which is the IPC surface over it.
//!
//! ## What it deliberately does not own
//!
//! - **The GPU.** This module produces bytes in system memory and never touches
//!   wgpu. `media` uploads them exactly as it uploads a decoded video frame;
//!   the compositor cannot tell the difference and does not need to.
//! - **Placement on the canvas.** A text layer is rasterised at canvas size and
//!   the segment's own `Transform` moves it, so a title is dragged, scaled and
//!   rotated by the same code that drags a clip. See [`RasterTarget::Canvas`].
//! - **Animation.** Per-character animation is the next piece of work and it is
//!   why [`RasteredText`] carries its [`TextLayout`] and a rectangle per glyph.
//!   Nothing here interpolates anything.
//!
//! ## Known gaps
//!
//! - **COLRv1 colour glyphs are not painted.** Bitmap strikes (`CBDT`/`sbix`,
//!   which is what Ubuntu's Noto Color Emoji ships) are. A COLRv1-only font
//!   draws its fallback outline, or nothing if it has none.
//! - **Vertical CJK (`writing-mode: vertical-rl`) is not implemented**, because
//!   no Rust text stack has the layout half of it. `docs/research/rust-crate-survey.md`
//!   §4 has the shape of the work if it is ever needed.
//! - **The document has no line height or letter spacing**, so
//!   [`TextRequest`] carries them with defaults and `TextMaterial` cannot yet
//!   set them.

pub mod animate;
mod cache;
pub mod commands;
pub mod edit;
mod font;
pub mod layout;
pub mod raster;
pub mod request;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;

pub use cache::CacheStats;
pub use edit::{default_material, insert_command, TextPlacement};
pub use layout::{GlyphRunStyle, LineBox, PositionedGlyph, TextLayout};
pub use raster::RasteredText;
pub use request::{RasterOptions, RasterTarget, TextHighlight, TextRequest, VerticalAlign};

use crate::modules::project::document::TextMaterial;

/// parley's two contexts, which are `Send` but not `Sync` in use.
struct Engine {
    fonts: parley::FontContext,
    layouts: parley::LayoutContext<()>,
}

/// The module's entry point.
///
/// Building one scans the system's fonts, which takes tens of milliseconds and
/// should happen once per process — use [`TextRenderer::shared`] unless a test
/// needs isolation. It is `Send + Sync`, so the preview's frame server and the
/// exporter can share it the way they share the compositor.
pub struct TextRenderer {
    engine: Mutex<Engine>,
    cache: Mutex<cache::RasterCache>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl TextRenderer {
    pub fn new() -> Self {
        Self::with_budget(cache::RasterCache::DEFAULT_BUDGET)
    }

    pub fn with_budget(bytes: usize) -> Self {
        Self {
            engine: Mutex::new(Engine {
                fonts: parley::FontContext::new(),
                layouts: parley::LayoutContext::new(),
            }),
            cache: Mutex::new(cache::RasterCache::new(bytes)),
        }
    }

    /// The process-wide renderer, built on first use.
    pub fn shared() -> &'static Arc<TextRenderer> {
        static SHARED: OnceLock<Arc<TextRenderer>> = OnceLock::new();
        SHARED.get_or_init(|| Arc::new(TextRenderer::new()))
    }

    /// Every font family the system offers, sorted, for a font picker.
    pub fn font_families(&self) -> Vec<String> {
        let mut engine = self.engine.lock();
        let mut names: Vec<String> = engine
            .fonts
            .collection
            .family_names()
            .map(str::to_string)
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Shape and break the text without drawing it.
    ///
    /// For measurement — how wide is this title, where does it wrap, how tall
    /// is the block — which the UI needs before it needs pixels.
    pub fn layout(&self, request: &TextRequest, options: &RasterOptions) -> TextLayout {
        let engine = &mut *self.engine.lock();
        layout::build(&mut engine.fonts, &mut engine.layouts, request, options)
    }

    /// Rasterise, using the cache.
    pub fn rasterize(&self, request: &TextRequest, options: &RasterOptions) -> Arc<RasteredText> {
        let key = cache::cache_key(request, options);
        if let Some(hit) = self.cache.lock().get(key) {
            return hit;
        }

        // The lock is not held across rasterisation: two threads racing on the
        // same title do the work twice and the second insert wins, which is
        // cheaper than making every other title wait behind this one.
        let layout = self.layout(request, options);
        let rastered = Arc::new(raster::rasterize(layout, request, options));
        self.cache.lock().insert(key, Arc::clone(&rastered));
        rastered
    }

    /// Rasterise ignoring the cache. For benchmarks, and for the tests that
    /// need to measure a cold draw.
    pub fn rasterize_uncached(
        &self,
        request: &TextRequest,
        options: &RasterOptions,
    ) -> RasteredText {
        let layout = self.layout(request, options);
        raster::rasterize(layout, request, options)
    }

    /// What `media` calls: a document material, drawn as one canvas-sized layer.
    ///
    /// `image` is the size of the frame being rendered and `scale` is how many
    /// of its pixels there are per document pixel — `image.0 / canvas.width`.
    /// The two are separate because the preview renders a 1080x1920 project
    /// into a 540x960 frame, and a 72-pixel title has to come out 36 pixels
    /// tall there rather than filling half the screen.
    pub fn rasterize_material(
        &self,
        material: &TextMaterial,
        image: (u32, u32),
        scale: f32,
    ) -> Arc<RasteredText> {
        self.rasterize(
            &TextRequest::from(material),
            &RasterOptions::canvas(image.0, image.1).with_scale(scale),
        )
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache.lock().stats()
    }

    pub fn clear_cache(&self) {
        self.cache.lock().clear();
    }
}
