//! Shaping and line breaking, flattened into something we own.
//!
//! parley's `Layout` borrows from its `LayoutContext` and is not `Send`, so it
//! cannot be cached, handed to the compositor, or kept next to the pixels it
//! produced. Everything it computed is copied out here into a plain owned
//! structure instead.
//!
//! That copy is not a workaround; it is the point of the module. Per-character
//! animation — the CapCut title feature this whole subsystem exists for — needs
//! a per-glyph position and a per-glyph rectangle *after* the image has been
//! rasterised, so a future caller can transform each glyph's quad
//! independently. [`RasteredText`](super::RasteredText) therefore carries the
//! layout beside the image rather than throwing it away, and this is the type
//! it carries.
//!
//! ## Coordinates
//!
//! Layout space: origin at the top-left of the text block, **y down**, in
//! device pixels (the requested scale is already applied). A glyph's `(x, y)`
//! is its pen position on the baseline, which is where a font's outline origin
//! goes.

use std::ops::Range;

use parley::style::{FontStyle, FontWeight, StyleProperty};
use parley::{
    Alignment, AlignmentOptions, FontData, LineHeight, PositionedLayoutItem, TextWrapMode,
};

use super::request::{RasterOptions, RasterTarget, TextRequest};
use crate::modules::project::document::TextAlign;

/// One glyph, positioned, with enough identity to draw or animate it alone.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedGlyph {
    /// Glyph index in the font of `run`. Not a character.
    pub id: u32,
    /// Index into [`TextLayout::runs`] — which font, size and variation this
    /// glyph is drawn with.
    pub run: usize,
    /// Index into [`TextLayout::lines`].
    pub line: usize,
    /// Pen position on the baseline, layout space.
    pub x: f32,
    pub y: f32,
    pub advance: f32,
    /// Byte range in the source string that this glyph's cluster came from.
    ///
    /// A ligature covers several characters and a mark covers none of its own,
    /// so this is a range and several glyphs can share one. It is what a
    /// per-character animation keys its stagger on when the user's idea of "a
    /// character" has to survive contact with Arabic.
    pub cluster: Range<usize>,
    /// Whether the glyph's run runs right to left.
    pub rtl: bool,
}

/// The font and size a group of glyphs is drawn with.
///
/// Kept out of [`PositionedGlyph`] because variation coordinates are a `Vec`
/// and there are thousands of glyphs and, typically, one run.
#[derive(Debug, Clone)]
pub struct GlyphRunStyle {
    pub font: FontData,
    /// Device pixels.
    pub font_size: f32,
    /// Normalized variation coordinates, as skrifa's `NormalizedCoord` bits.
    pub coords: Vec<i16>,
    /// The font is not actually bold and fontique wants us to fake it.
    pub embolden: bool,
    /// Faux-italic shear, in degrees. 0 when the font has a real italic.
    pub skew: f32,
}

/// One laid-out line.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub index: usize,
    /// Left edge of the line's ink advance, layout space, alignment applied.
    pub x: f32,
    /// Top of the line box, layout space.
    pub y: f32,
    /// Advance excluding trailing whitespace — the width a background box wants.
    pub width: f32,
    pub height: f32,
    /// Baseline offset from the top of the layout.
    pub baseline: f32,
    /// Range into [`TextLayout::glyphs`].
    pub glyphs: Range<usize>,
}

/// A shaped, broken and aligned paragraph, owned and `Send`.
#[derive(Debug, Clone, Default)]
pub struct TextLayout {
    /// Width of the longest line, excluding trailing whitespace.
    pub width: f32,
    pub height: f32,
    /// Whether the paragraph's base direction is right to left.
    pub is_rtl: bool,
    pub lines: Vec<LineBox>,
    pub runs: Vec<GlyphRunStyle>,
    /// Every glyph, in line order and then in **visual** order within the line.
    pub glyphs: Vec<PositionedGlyph>,
}

impl TextLayout {
    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// Glyphs of one line, in visual order.
    pub fn line_glyphs(&self, line: usize) -> &[PositionedGlyph] {
        match self.lines.get(line) {
            Some(line) => &self.glyphs[line.glyphs.clone()],
            None => &[],
        }
    }
}

/// Shape and break `request` and copy the result out of parley.
pub(crate) fn build(
    font_cx: &mut parley::FontContext,
    layout_cx: &mut parley::LayoutContext<()>,
    request: &TextRequest,
    options: &RasterOptions,
) -> TextLayout {
    let scale = options.scale.max(0.01);
    let text = request.content.as_str();

    let mut builder = layout_cx.ranged_builder(font_cx, text, scale, true);
    builder.push_default(StyleProperty::FontFamily(super::font::family_stack(
        &request.font_family,
    )));
    builder.push_default(StyleProperty::FontSize(request.font_size.max(1.0)));
    builder.push_default(StyleProperty::FontWeight(FontWeight::new(
        if request.bold { 700.0 } else { 400.0 },
    )));
    builder.push_default(StyleProperty::FontStyle(if request.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    }));
    if let Some(line_height) = request.line_height {
        builder.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
            line_height.max(0.1),
        )));
    }
    if request.letter_spacing != 0.0 {
        builder.push_default(StyleProperty::LetterSpacing(request.letter_spacing));
    }

    // Emoji get their own font stack over their own byte ranges, so a colour
    // emoji font is consulted for emoji and for nothing else. See
    // `font::emoji_ranges` for why this is a range override rather than a
    // change to the default stack.
    for range in super::font::emoji_ranges(text) {
        builder.push(
            StyleProperty::FontFamily(super::font::emoji_stack(&request.font_family)),
            range,
        );
    }

    // Wrapping is a property of the target, not of the text: a tight raster
    // with no width wraps only where the author typed a newline.
    let max_advance = match options.target {
        RasterTarget::Canvas { width, .. } => {
            Some((width as f32 - 2.0 * options.margin * scale).max(1.0))
        }
        RasterTarget::Tight { max_width } => max_width.map(|w| (w * scale).max(1.0)),
    };
    if max_advance.is_none() {
        builder.push_default(StyleProperty::TextWrapMode(TextWrapMode::NoWrap));
    }

    let mut layout: parley::Layout<()> = builder.build(text);
    layout.break_all_lines(max_advance);
    layout.align(
        match request.align {
            TextAlign::Left => Alignment::Left,
            TextAlign::Center => Alignment::Center,
            TextAlign::Right => Alignment::Right,
        },
        // Centre a line that is wider than the box anyway: clipping a long
        // title symmetrically looks like a long title, clipping it on one side
        // looks like a bug.
        AlignmentOptions {
            align_when_overflowing: true,
        },
    );

    flatten(&layout)
}

fn flatten(layout: &parley::Layout<()>) -> TextLayout {
    let mut out = TextLayout {
        width: layout.width(),
        height: layout.height(),
        is_rtl: layout.is_rtl(),
        ..Default::default()
    };

    for (line_index, line) in layout.lines().enumerate() {
        let metrics = *line.metrics();
        let glyph_start = out.glyphs.len();

        // `GlyphRun::glyph_start` is private, so the flat index of a glyph
        // inside its line item is tracked here the same way parley tracks it:
        // it restarts whenever the item changes, and consecutive glyph runs
        // from one item continue the count. Identity is the item's cluster
        // range rather than the run index, because that is what a line item
        // actually is.
        let mut current_item: Option<(usize, Range<usize>)> = None;
        let mut cursor = 0usize;
        let mut cluster_of_glyph: Vec<Range<usize>> = Vec::new();

        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let run = glyph_run.run();
            let identity = (run.index(), run.cluster_range());
            if current_item.as_ref() != Some(&identity) {
                current_item = Some(identity);
                cursor = 0;
                cluster_of_glyph.clear();
                map_clusters_to_glyphs(run, &mut cluster_of_glyph);
            }

            let run_index = intern_run(&mut out.runs, run);
            let rtl = run.is_rtl();
            let mut count = 0usize;
            for glyph in glyph_run.positioned_glyphs() {
                let cluster = cluster_of_glyph
                    .get(cursor + count)
                    .cloned()
                    .unwrap_or_else(|| run.text_range());
                out.glyphs.push(PositionedGlyph {
                    id: glyph.id,
                    run: run_index,
                    line: line_index,
                    x: glyph.x,
                    y: glyph.y,
                    advance: glyph.advance,
                    cluster,
                    rtl,
                });
                count += 1;
            }
            cursor += count;
        }

        out.lines.push(LineBox {
            index: line_index,
            x: metrics.inline_min_coord + metrics.offset,
            y: metrics.block_min_coord,
            width: (metrics.advance - metrics.trailing_whitespace).max(0.0),
            height: (metrics.block_max_coord - metrics.block_min_coord).max(0.0),
            baseline: metrics.baseline,
            glyphs: glyph_start..out.glyphs.len(),
        });
    }

    out
}

/// Which characters each glyph of `run` came from, in visual order.
///
/// The subtlety is ligatures. parley gives the lam-alef of "لا" one glyph on
/// the *first* cluster and an empty continuation cluster for the second
/// character, so reading `Cluster::text_range` per glyph reports half the text
/// the glyph actually stands for. A per-character animation staggering on that
/// would skip a letter, and a hit test would put the caret in the wrong place.
///
/// So a cluster with no glyphs of its own is folded into the nearest one that
/// has them: the preceding glyph when there is one, and otherwise the next.
/// Which side that lands on depends on the run's direction and on where the
/// shaper chose to hang the ligature — for Arabic lam-alef, harfrust puts the
/// glyph on the *second* cluster — so this deliberately does not assume.
fn map_clusters_to_glyphs(run: &parley::Run<'_, ()>, out: &mut Vec<Range<usize>>) {
    let mut pending: Option<Range<usize>> = None;

    for cluster in run.visual_clusters() {
        let range = cluster.text_range();
        let before = out.len();
        for _ in cluster.glyphs() {
            out.push(range.clone());
        }

        if out.len() == before {
            match out.last_mut() {
                Some(last) => *last = union(last.clone(), range),
                None => {
                    pending = Some(match pending {
                        None => range,
                        Some(previous) => union(previous, range),
                    });
                }
            }
        } else if let Some(continuation) = pending.take() {
            for glyph in &mut out[before..] {
                *glyph = union(glyph.clone(), continuation.clone());
            }
        }
    }
}

fn union(a: Range<usize>, b: Range<usize>) -> Range<usize> {
    a.start.min(b.start)..a.end.max(b.end)
}

/// Add this run's font and size to the style table, or find it if it is there.
///
/// Runs are few — one per script per style — so a linear scan is the right
/// structure and a hash map would be slower.
fn intern_run(runs: &mut Vec<GlyphRunStyle>, run: &parley::Run<'_, ()>) -> usize {
    let font = run.font();
    let synthesis = run.synthesis();
    let candidate = GlyphRunStyle {
        font: font.clone(),
        font_size: run.font_size(),
        coords: run.normalized_coords().to_vec(),
        embolden: synthesis.embolden(),
        skew: synthesis.skew().unwrap_or(0.0),
    };

    let blob_identity = |font: &FontData| {
        let bytes: &[u8] = font.data.as_ref();
        (bytes.as_ptr() as usize, bytes.len(), font.index)
    };
    let candidate_identity = blob_identity(&candidate.font);
    if let Some(index) = runs.iter().position(|existing| {
        blob_identity(&existing.font) == candidate_identity
            && existing.font_size == candidate.font_size
            && existing.coords == candidate.coords
            && existing.embolden == candidate.embolden
            && existing.skew == candidate.skew
    }) {
        return index;
    }

    runs.push(candidate);
    runs.len() - 1
}
