//! Keeping a rasterised title between frames.
//!
//! A 1080p text layer costs single-digit milliseconds to draw and 8 MB to hold.
//! Drawing it again for every frame of a five-second title is 150 wasted
//! rasterisations, and it produces the same bytes each time, because a title
//! does not change between frames — animation moves the *quad*, not the
//! pixels. So the whole thing is keyed on a hash of everything that could
//! change it and held until the budget pushes it out.
//!
//! The key deliberately includes the raster options as well as the text: the
//! preview asks for 540x960 and the export for 1080x1920, and handing the
//! export the preview's raster would be the same soft-title bug the media
//! provider's size check exists to prevent.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::raster::RasteredText;
use super::request::{RasterOptions, RasterTarget, TextRequest};

/// Everything the cache holds, for diagnostics and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub bytes: usize,
    pub hits: u64,
    pub misses: u64,
}

struct Entry {
    value: Arc<RasteredText>,
    bytes: usize,
    last_used: u64,
}

pub(crate) struct RasterCache {
    entries: HashMap<u64, Entry>,
    /// Bytes of pixel data allowed before the least recently used entry goes.
    budget: usize,
    bytes: usize,
    clock: u64,
    hits: u64,
    misses: u64,
}

impl RasterCache {
    /// 96 MB, about a dozen 1080p layers. A timeline with more simultaneous
    /// titles than that is not a thing anybody makes, and the failure mode when
    /// it happens is a slower render rather than a wrong one.
    pub const DEFAULT_BUDGET: usize = 96 * 1024 * 1024;

    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget,
            bytes: 0,
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn get(&mut self, key: u64) -> Option<Arc<RasteredText>> {
        self.clock += 1;
        let clock = self.clock;
        match self.entries.get_mut(&key) {
            Some(entry) => {
                entry.last_used = clock;
                self.hits += 1;
                Some(Arc::clone(&entry.value))
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    pub fn insert(&mut self, key: u64, value: Arc<RasteredText>) {
        self.clock += 1;
        let bytes = value.byte_size();
        if let Some(previous) = self.entries.insert(
            key,
            Entry {
                value,
                bytes,
                last_used: self.clock,
            },
        ) {
            self.bytes -= previous.bytes;
        }
        self.bytes += bytes;
        self.evict();
    }

    fn evict(&mut self) {
        while self.bytes > self.budget && self.entries.len() > 1 {
            let Some((&oldest, _)) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(k, v)| (k, v))
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes -= entry.bytes;
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            bytes: self.bytes,
            hits: self.hits,
            misses: self.misses,
        }
    }
}

/// Hash of everything that changes the pixels.
///
/// Floats are hashed by their bit pattern, which makes `-0.0` and `0.0`
/// different keys and `NaN` a stable one. Both are the right trade here: a
/// false miss costs a rasterisation, a false hit shows the wrong title.
pub(crate) fn cache_key(request: &TextRequest, options: &RasterOptions) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    request.content.hash(&mut hasher);
    request.font_family.hash(&mut hasher);
    request.bold.hash(&mut hasher);
    request.italic.hash(&mut hasher);
    (request.align as u8).hash(&mut hasher);
    for value in [
        request.font_size,
        request.stroke_width,
        request.letter_spacing,
        request.background_radius,
    ] {
        value.to_bits().hash(&mut hasher);
    }
    for channel in request.color.iter().chain(request.stroke_color.iter()) {
        channel.to_bits().hash(&mut hasher);
    }
    hash_option_f32(&mut hasher, request.line_height);
    hash_option_f32(&mut hasher, request.background_padding);
    match request.shadow {
        None => 0u8.hash(&mut hasher),
        Some(shadow) => {
            1u8.hash(&mut hasher);
            for value in shadow
                .color
                .iter()
                .chain(shadow.offset.iter())
                .chain(std::iter::once(&shadow.blur))
            {
                value.to_bits().hash(&mut hasher);
            }
        }
    }
    match request.background {
        None => 0u8.hash(&mut hasher),
        Some(color) => {
            1u8.hash(&mut hasher);
            for channel in color {
                channel.to_bits().hash(&mut hasher);
            }
        }
    }
    match &request.highlight {
        None => 0u8.hash(&mut hasher),
        Some(highlight) => {
            1u8.hash(&mut hasher);
            highlight.range.start.hash(&mut hasher);
            highlight.range.end.hash(&mut hasher);
            for channel in highlight.color {
                channel.to_bits().hash(&mut hasher);
            }
        }
    }

    match options.target {
        RasterTarget::Canvas { width, height } => {
            0u8.hash(&mut hasher);
            width.hash(&mut hasher);
            height.hash(&mut hasher);
        }
        RasterTarget::Tight { max_width } => {
            1u8.hash(&mut hasher);
            hash_option_f32(&mut hasher, max_width);
        }
    }
    options.scale.to_bits().hash(&mut hasher);
    options.margin.to_bits().hash(&mut hasher);
    (options.vertical_align as u8).hash(&mut hasher);

    hasher.finish()
}

fn hash_option_f32(hasher: &mut impl Hasher, value: Option<f32>) {
    match value {
        None => 0u8.hash(hasher),
        Some(value) => {
            1u8.hash(hasher);
            value.to_bits().hash(hasher);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_follows_every_field_that_changes_the_picture() {
        let base = TextRequest {
            content: "Hello".into(),
            ..TextRequest::default()
        };
        let options = RasterOptions::tight();
        let key = cache_key(&base, &options);

        let mut changed = base.clone();
        changed.content = "Hellp".into();
        assert_ne!(cache_key(&changed, &options), key);

        let mut changed = base.clone();
        changed.font_size += 0.5;
        assert_ne!(cache_key(&changed, &options), key);

        let mut changed = base.clone();
        changed.color[0] = 0.5;
        assert_ne!(cache_key(&changed, &options), key);

        assert_ne!(cache_key(&base, &options.with_scale(2.0)), key);
        assert_eq!(cache_key(&base.clone(), &options), key);
    }
}
