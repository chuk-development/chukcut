//! Turning a [`TextLayout`] into pixels.
//!
//! Everything above the shaping layer is ours; no Rust crate draws an outlined,
//! shadowed title with a background box. What is borrowed is the hard part —
//! `skrifa` for outlines, `kurbo` for expanding one into a stroke, `zeno` for
//! scan conversion — and what is written here is the compositing on top.
//!
//! ## Order of painting
//!
//! Back to front: background box, drop shadow, outline, fill, colour bitmaps.
//! The shadow is cast by the silhouette of *fill plus outline*, because a
//! shadow that ignored the outline would peek out from under one side of it.
//!
//! ## Why the intermediate buffer is premultiplied and the output is not
//!
//! Four layers blend over each other, and source-over blending is only
//! associative in premultiplied alpha — compositing straight-alpha values
//! directly is how a dark shadow under white text turns the antialiased edges
//! grey. The compositor, on the other hand, takes straight alpha (`SourceFrame`
//! documents its input so, and the quad shader premultiplies on its own), so the
//! last step divides the colour back out.
//!
//! Blending happens on sRGB-encoded bytes rather than in linear light. That is
//! technically the wrong place to antialias, and it is what every text renderer
//! does, because glyph edges antialiased in linear light look too thin at small
//! sizes. It also matches what the user saw in the colour picker.

use kurbo::{BezPath, PathEl, Point, Shape};
use skrifa::instance::{LocationRef, NormalizedCoord, Size};
use skrifa::outline::DrawSettings;
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use zeno::{Command, Mask, Origin, Style, Vector};

use super::layout::{GlyphRunStyle, TextLayout};
use super::request::{RasterOptions, RasterTarget, TextRequest, VerticalAlign};

/// A rasterised text layer, with the layout that produced it.
///
/// The layout is here so per-character animation can exist later without
/// re-shaping: a caller that wants to fly each letter in separately takes
/// `glyph_rects[i]` as a sub-rectangle of `pixels` and gives it its own
/// transform. That is the only reason this is not just an image.
pub struct RasteredText {
    pub width: u32,
    pub height: u32,
    /// RGBA8, straight (non-premultiplied) alpha, sRGB-encoded, tightly packed,
    /// `width * height * 4` bytes. Ready for `media`'s `upload_rgba`.
    pub pixels: Vec<u8>,
    /// Where the layout's origin (top-left of the text block) sits in the image.
    pub origin: (f32, f32),
    pub layout: TextLayout,
    /// Ink rectangle of each glyph in `layout.glyphs`, as `[x0, y0, x1, y1]` in
    /// image pixel space, including the outline but not the shadow. Empty
    /// glyphs (a space) get a zero-area rectangle at the pen position.
    pub glyph_rects: Vec<[f32; 4]>,
}

impl std::fmt::Debug for RasteredText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RasteredText")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("origin", &self.origin)
            .field("glyphs", &self.layout.glyphs.len())
            .finish()
    }
}

impl RasteredText {
    /// Bytes held, for the cache's budget.
    pub fn byte_size(&self) -> usize {
        self.pixels.len() + self.glyph_rects.len() * 16
    }

    /// Straight-alpha RGBA of one pixel. Test convenience.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y as usize) * (self.width as usize) + x as usize) * 4;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }
}

/// One path drawn one way, as an input to a coverage mask.
#[derive(Clone, Copy)]
struct Layer<'a> {
    path: &'a [Command],
    style: Style<'a>,
    /// How far this layer paints outside the path's control-point bounds —
    /// half the stroke width, plus whatever a blur will spread it by. It sizes
    /// the mask; getting it too small clips, too large only wastes.
    pad: f32,
}

impl<'a> Layer<'a> {
    fn fill(path: &'a [Command]) -> Self {
        Self {
            path,
            style: Style::Fill(zeno::Fill::NonZero),
            pad: 0.0,
        }
    }

    /// A stroke `visible` pixels wide *outside* the path.
    ///
    /// A stroke is centred, so the width is doubled and the fill is painted
    /// back over the inner half. Round joins because a glyph outline has sharp
    /// corners and a mitred outline grows spikes at them.
    fn stroke(path: &'a [Command], visible: f32) -> Self {
        // Round joins, measured rather than assumed: `Join::Miter` with a limit
        // of 2 — which is what a glyph's sharp corners would seem to want — was
        // **four times slower** on the same 26-glyph title (54–62 ms against
        // 15 ms), presumably because every corner of every contour produces a
        // spike that then has to be resolved into a bevel.
        let mut stroke = zeno::Stroke::new(visible * 2.0);
        stroke.join(zeno::Join::Round);
        stroke.cap(zeno::Cap::Round);
        Self {
            path,
            style: Style::Stroke(stroke),
            pad: visible + 1.0,
        }
    }
}

/// An 8-bit coverage mask covering part of the image.
struct Coverage {
    x0: i32,
    y0: i32,
    width: u32,
    height: u32,
    alpha: Vec<u8>,
}

impl Coverage {
    /// Scan-convert one or more layers into a single mask, taking the maximum
    /// coverage where they overlap.
    ///
    /// The union is by `max` rather than by drawing them all as one non-zero
    /// fill, because a stroke laid over the glyph it came from has boundaries
    /// running the opposite way and the winding numbers cancel — which shows up
    /// as holes in the shadow of outlined text.
    ///
    /// Strokes are `zeno`'s, not `kurbo::stroke`'s. The crate survey suggested
    /// kurbo and it is the wrong tool here by a wide margin: expanding a glyph
    /// outline into a stroke *path* and then rasterising that path cost about
    /// 0.9 ms per glyph, because round joins on every corner of every contour
    /// produce an outline with several times the segments. zeno strokes during
    /// scan conversion and never builds the path. Measured in
    /// `docs/research/text-rendering.md`.
    fn render(layers: &[Layer<'_>], clip: (u32, u32)) -> Option<Self> {
        let mut bounds: Option<[f32; 4]> = None;
        for layer in layers {
            let Some(b) = path_bounds(layer.path) else {
                continue;
            };
            let b = [
                b[0] - layer.pad,
                b[1] - layer.pad,
                b[2] + layer.pad,
                b[3] + layer.pad,
            ];
            bounds = Some(match bounds {
                None => b,
                Some(a) => [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ],
            });
        }
        let b = bounds?;

        let x0 = (b[0].floor() as i32).clamp(0, clip.0 as i32);
        let y0 = (b[1].floor() as i32).clamp(0, clip.1 as i32);
        let x1 = (b[2].ceil() as i32).clamp(0, clip.0 as i32);
        let y1 = (b[3].ceil() as i32).clamp(0, clip.1 as i32);
        let (width, height) = ((x1 - x0) as u32, (y1 - y0) as u32);
        if width == 0 || height == 0 {
            return None;
        }

        let mut alpha = vec![0u8; (width as usize) * (height as usize)];
        let mut scratch = if layers.len() > 1 {
            vec![0u8; alpha.len()]
        } else {
            Vec::new()
        };
        for (i, layer) in layers.iter().enumerate() {
            if layer.path.is_empty() {
                continue;
            }
            let target = if i == 0 { &mut alpha } else { &mut scratch };
            target.fill(0);
            Mask::new(layer.path)
                .style(layer.style)
                .origin(Origin::TopLeft)
                .offset(Vector::new(-x0 as f32, -y0 as f32))
                .size(width, height)
                .render_into(target, None);
            if i > 0 {
                for (dst, src) in alpha.iter_mut().zip(scratch.iter()) {
                    *dst = (*dst).max(*src);
                }
            }
        }

        Some(Self {
            x0,
            y0,
            width,
            height,
            alpha,
        })
    }

    /// The same coverage, moved by `offset` and with `pad` pixels of room around
    /// it, clipped to the image.
    ///
    /// This is what makes a drop shadow nearly free. The shadow is the text's
    /// own silhouette somewhere else, and scan-converting it a second time was
    /// the single most expensive thing this module did — a stroked 26-glyph
    /// title costs about 15 ms per pass at 1080p, and the shadow was paying for
    /// two of them again. Moving the rectangle costs a memcpy.
    ///
    /// The offset is rounded to whole pixels. Under a blur that is invisible,
    /// and a hard shadow half a pixel off is not something anyone has ever
    /// noticed in a title.
    fn shifted(&self, offset: (f32, f32), pad: f32, clip: (u32, u32)) -> Self {
        let pad = pad.ceil() as i32;
        let dx = offset.0.round() as i32;
        let dy = offset.1.round() as i32;
        let x0 = (self.x0 + dx - pad).clamp(0, clip.0 as i32);
        let y0 = (self.y0 + dy - pad).clamp(0, clip.1 as i32);
        let x1 = (self.x0 + dx + self.width as i32 + pad).clamp(0, clip.0 as i32);
        let y1 = (self.y0 + dy + self.height as i32 + pad).clamp(0, clip.1 as i32);
        let (width, height) = ((x1 - x0) as u32, (y1 - y0) as u32);

        let mut alpha = vec![0u8; (width as usize) * (height as usize)];
        for row in 0..self.height as i32 {
            let target_y = self.y0 + dy + row - y0;
            if target_y < 0 || target_y >= height as i32 {
                continue;
            }
            // The source row may hang off either end of the destination, so the
            // overlap is computed rather than assumed.
            let source_x0 = (-(self.x0 + dx - x0)).max(0);
            let count =
                (self.width as i32 - source_x0).min(width as i32 - (self.x0 + dx - x0).max(0));
            if count <= 0 {
                continue;
            }
            let target_x0 = (self.x0 + dx - x0).max(0);
            let source = (row * self.width as i32 + source_x0) as usize;
            let target = (target_y * width as i32 + target_x0) as usize;
            alpha[target..target + count as usize]
                .copy_from_slice(&self.alpha[source..source + count as usize]);
        }

        Self {
            x0,
            y0,
            width,
            height,
            alpha,
        }
    }

    /// A separable triple box blur, which is a Gaussian to within a percent and
    /// costs the same whatever the radius.
    fn blur(&mut self, sigma: f32) {
        if sigma <= 0.05 {
            return;
        }
        // Three box passes whose widths sum to the right variance. The
        // arithmetic is Kovesi, "Fast Almost-Gaussian Filtering": with n boxes,
        // the first m of them are `wl` wide and the rest `wl + 2`.
        const N: f32 = 3.0;
        let variance = 12.0 * sigma * sigma;
        let mut wl = (variance / N + 1.0).sqrt().floor() as i32;
        if wl % 2 == 0 {
            wl -= 1;
        }
        let wl = wl.max(1);
        let wu = wl + 2;
        let m = ((variance - N * (wl * wl) as f32 - 4.0 * N * wl as f32 - 3.0 * N)
            / (-4.0 * wl as f32 - 4.0))
            .round() as i32;
        let radii = [
            if m > 0 { wl } else { wu },
            if m > 1 { wl } else { wu },
            if m > 2 { wl } else { wu },
        ];

        let (w, h) = (self.width as usize, self.height as usize);
        let mut tmp = vec![0u8; self.alpha.len()];
        for radius in radii {
            let r = ((radius - 1) / 2).max(0) as usize;
            if r == 0 {
                continue;
            }
            box_blur_axis(&self.alpha, &mut tmp, w, h, r, true);
            box_blur_axis(&tmp, &mut self.alpha, w, h, r, false);
        }
    }
}

fn box_blur_axis(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    let (step_outer, step_inner) = if horizontal { (w, 1) } else { (1, w) };
    let window = (2 * r + 1) as u32;

    for o in 0..outer {
        let base = o * step_outer;
        // A running sum with the edges clamped, so the mask does not darken at
        // its own border.
        let mut sum: u32 = 0;
        let first = src[base] as u32;
        let last = src[base + (inner - 1) * step_inner] as u32;
        sum += first * (r as u32 + 1);
        for i in 1..=r.min(inner - 1) {
            sum += src[base + i * step_inner] as u32;
        }
        if r >= inner {
            sum += last * (r - inner + 1) as u32;
        }
        for i in 0..inner {
            dst[base + i * step_inner] = (sum / window) as u8;
            let add = if i + r + 1 < inner {
                src[base + (i + r + 1) * step_inner] as u32
            } else {
                last
            };
            let sub = if i >= r {
                src[base + (i - r) * step_inner] as u32
            } else {
                first
            };
            sum = sum + add - sub;
        }
    }
}

/// A premultiplied RGBA8 accumulation buffer.
struct Canvas {
    width: u32,
    height: u32,
    px: Vec<u8>,
    /// Rectangle anything has been painted into, as `[x0, y0, x1, y1]`.
    ///
    /// A canvas-sized layer is mostly empty — a title covers perhaps a twentieth
    /// of a 1080p frame — and the final unpremultiply-and-bleed pass is a scan
    /// over every pixel. Tracking what was touched makes that pass cost what the
    /// text costs instead of what the frame costs.
    dirty: [i32; 4],
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            px: vec![0u8; (width as usize) * (height as usize) * 4],
            dirty: [i32::MAX, i32::MAX, i32::MIN, i32::MIN],
        }
    }

    fn mark(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.dirty[0] = self.dirty[0].min(x0.clamp(0, self.width as i32));
        self.dirty[1] = self.dirty[1].min(y0.clamp(0, self.height as i32));
        self.dirty[2] = self.dirty[2].max(x1.clamp(0, self.width as i32));
        self.dirty[3] = self.dirty[3].max(y1.clamp(0, self.height as i32));
    }

    /// Source-over one flat colour through a coverage mask.
    ///
    /// Integer arithmetic throughout, and the row's x range is clipped once
    /// instead of per pixel. The float version of this was the most expensive
    /// thing left in the module after the scan conversion — a full-width
    /// background box, which is one rectangle, cost 5 ms at 1080p purely in
    /// `round()` and `clamp()` per channel per pixel.
    fn blend_mask(&mut self, mask: &Coverage, color: [f32; 4]) {
        let alpha = channel(color[3]);
        if alpha == 0 {
            return;
        }
        let rgb = [channel(color[0]), channel(color[1]), channel(color[2])];
        self.mark(
            mask.x0,
            mask.y0,
            mask.x0 + mask.width as i32,
            mask.y0 + mask.height as i32,
        );

        let first = (-mask.x0).max(0);
        let last = ((self.width as i32) - mask.x0).min(mask.width as i32);
        if last <= first {
            return;
        }

        for row in 0..mask.height as i32 {
            let y = mask.y0 + row;
            if y < 0 || y >= self.height as i32 {
                continue;
            }
            let mask_row = row as usize * mask.width as usize;
            for col in first..last {
                let coverage = mask.alpha[mask_row + col as usize] as u32;
                if coverage == 0 {
                    continue;
                }
                let src_a = div255(alpha * coverage);
                if src_a == 0 {
                    continue;
                }
                let i = (y as usize * self.width as usize + (mask.x0 + col) as usize) * 4;
                let inv = 255 - src_a;
                for c in 0..3 {
                    self.px[i + c] =
                        (div255(rgb[c] * src_a) + div255(self.px[i + c] as u32 * inv)) as u8;
                }
                self.px[i + 3] = (src_a + div255(self.px[i + 3] as u32 * inv)) as u8;
            }
        }
    }

    /// Source-over one already-premultiplied RGBA sample, 0..255 per channel.
    fn over(&mut self, x: usize, y: usize, src: [u32; 4]) {
        let i = (y * self.width as usize + x) * 4;
        let inv = 255 - src[3].min(255);
        for c in 0..4 {
            self.px[i + c] = (src[c] + div255(self.px[i + c] as u32 * inv)).min(255) as u8;
        }
    }

    /// Premultiplied accumulation → straight alpha, with the colour of the
    /// nearest opaque neighbour bled into fully transparent pixels.
    ///
    /// The bleed matters because the compositor samples this texture bilinearly
    /// and a transparent pixel whose RGB is black drags a dark fringe into the
    /// glyph edge next to it whenever the layer is scaled. Straight-alpha
    /// textures are only safe if the colour under alpha 0 is plausible.
    fn finish(mut self) -> Vec<u8> {
        let w = self.width as usize;
        if self.dirty[2] <= self.dirty[0] || self.dirty[3] <= self.dirty[1] {
            return self.px;
        }
        // One pixel of slack so the bleed has somewhere to write.
        let x0 = (self.dirty[0] - 1).max(0) as usize;
        let y0 = (self.dirty[1] - 1).max(0) as usize;
        let x1 = (self.dirty[2] + 1).min(self.width as i32) as usize;
        let y1 = (self.dirty[3] + 1).min(self.height as i32) as usize;

        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) * 4;
                let a = self.px[i + 3];
                if a == 0 || a == 255 {
                    continue;
                }
                let scale = 255.0 / a as f32;
                for c in 0..3 {
                    self.px[i + c] =
                        (self.px[i + c] as f32 * scale).round().clamp(0.0, 255.0) as u8;
                }
            }
        }

        // Only the touched rows are copied. At 1080p the whole buffer is 8 MB
        // and a title occupies a few hundred rows of it; cloning all of it to
        // read from while writing was measurably more expensive than the bleed
        // itself.
        let row = w * 4;
        let source = self.px[y0 * row..y1 * row].to_vec();
        let at = |x: usize, y: usize| (y - y0) * row + x * 4;

        for y in y0..y1 {
            for x in x0..x1 {
                let i = at(x, y);
                if source[i + 3] != 0 {
                    continue;
                }
                let mut best = None;
                'search: for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                        if nx < x0 as i32 || ny < y0 as i32 || nx >= x1 as i32 || ny >= y1 as i32 {
                            continue;
                        }
                        let j = at(nx as usize, ny as usize);
                        if source[j + 3] != 0 {
                            best = Some(j);
                            break 'search;
                        }
                    }
                }
                if let Some(j) = best {
                    let target = (y * w + x) * 4;
                    self.px[target] = source[j];
                    self.px[target + 1] = source[j + 1];
                    self.px[target + 2] = source[j + 2];
                }
            }
        }

        self.px
    }
}

/// The font's own underline, made bold enough to read in a video.
///
/// Fonts draw a line for body text, about a twentieth of the size; on a
/// title over footage that is a hairline. It is kept at least a fifteenth of
/// the size, and a font without the metric gets that at a tenth below the
/// baseline.
fn underline_metrics(font: &FontRef<'_>, style: &GlyphRunStyle) -> (f32, f32) {
    let size = style.font_size.max(1.0);
    let coords: Vec<NormalizedCoord> = style
        .coords
        .iter()
        .map(|bits| NormalizedCoord::from_bits(*bits))
        .collect();
    let metrics = font.metrics(Size::new(size), LocationRef::new(&coords));
    let (offset, thickness) = metrics
        .underline
        .map(|d| (-d.offset, d.thickness))
        .unwrap_or((size * 0.1, size / 15.0));
    let thickness = thickness.max(size / 15.0).max(1.0);
    (offset.max(0.0), thickness)
}

/// A colour bitmap glyph, already decoded and positioned.
struct BitmapDraw {
    /// Straight-alpha RGBA8 source.
    rgba: Vec<u8>,
    src_width: u32,
    src_height: u32,
    /// Destination rectangle in image space.
    rect: [f32; 4],
}

/// Rasterise `layout` for `request`.
///
/// Infallible by construction: a font that will not parse, a glyph with no
/// outline and a zero-size image all degrade to drawing less, never to an
/// error. A title that fails to render is a black frame in an export, which is
/// worse than a title that renders in the wrong font.
pub(crate) fn rasterize(
    layout: TextLayout,
    request: &TextRequest,
    options: &RasterOptions,
) -> RasteredText {
    let scale = options.scale.max(0.01);
    let bleed = request.bleed() * scale;

    let (width, height, origin) = match options.target {
        RasterTarget::Canvas { width, height } => {
            let width = width.max(1);
            let height = height.max(1);
            let margin = options.margin * scale;
            let free = height as f32 - layout.height;
            let y = match options.vertical_align {
                VerticalAlign::Top => margin,
                VerticalAlign::Middle => free * 0.5,
                VerticalAlign::Bottom => free - margin,
            };
            (width, height, (margin, y))
        }
        RasterTarget::Tight { .. } => {
            let width = (layout.width + 2.0 * bleed).ceil().max(1.0);
            let height = (layout.height + 2.0 * bleed).ceil().max(1.0);
            (
                width.min(16384.0) as u32,
                height.min(16384.0) as u32,
                (bleed, bleed),
            )
        }
    };

    let clip = (width, height);
    let mut canvas = Canvas::new(width, height);
    let mut timing = Timing::default();

    // --- background box -------------------------------------------------
    if let Some(color) = request.background {
        let pad = request.background_padding_px() * scale;
        let radius = (request.background_radius * scale).max(0.0) as f64;
        let mut path = BezPath::new();
        for line in &layout.lines {
            if line.width <= 0.0 {
                continue;
            }
            let rect = kurbo::Rect::new(
                (origin.0 + line.x - pad) as f64,
                (origin.1 + line.y - pad) as f64,
                (origin.0 + line.x + line.width + pad) as f64,
                (origin.1 + line.y + line.height + pad) as f64,
            );
            let radius = radius.min(rect.width() * 0.5).min(rect.height() * 0.5);
            path.extend(kurbo::RoundedRect::from_rect(rect, radius).path_elements(0.1));
        }
        let commands = bez_to_commands(&path);
        if let Some(mask) = Coverage::render(&[Layer::fill(&commands)], clip) {
            canvas.blend_mask(&mask, color);
        }
        timing.background = timing.mark();
    }

    // --- glyph geometry -------------------------------------------------
    let fonts = RunFonts::new(&layout);
    let stroke_width = (request.stroke_width * scale).max(0.0);
    let mut fill_commands: Vec<Command> = Vec::new();
    // Glyphs whose run wants a weight the family does not have. Kept apart from
    // `fill_commands` because faking the weight means stroking them, and the
    // rest of the text must not be stroked.
    let mut bold_commands: Vec<Command> = Vec::new();
    let mut bold_width = 0.0f32;
    let mut bitmaps: Vec<BitmapDraw> = Vec::new();
    let mut glyph_rects: Vec<[f32; 4]> = Vec::with_capacity(layout.glyphs.len());
    // The karaoke word: its glyphs are filled a second time in their own
    // colour, over the ordinary fill. Over rather than instead of, so the
    // outline, shadow and silhouette below stay one shape whatever is lit.
    let mut lit_commands: Vec<Command> = Vec::new();
    let mut lit_bold: Vec<Command> = Vec::new();

    for glyph in &layout.glyphs {
        let pen = (origin.0 + glyph.x, origin.1 + glyph.y);
        let Some(font) = fonts.get(glyph.run) else {
            glyph_rects.push([pen.0, pen.1, pen.0, pen.1]);
            continue;
        };

        if let Some(draw) = font.bitmap(glyph.id, pen) {
            glyph_rects.push(draw.rect);
            bitmaps.push(draw);
            continue;
        }

        let Some(path) = font.outline(glyph.id, pen) else {
            glyph_rects.push([pen.0, pen.1, pen.0, pen.1]);
            continue;
        };
        let mut ink = bez_bounds(&path).unwrap_or([pen.0, pen.1, pen.0, pen.1]);
        if stroke_width > 0.0 {
            ink = [
                ink[0] - stroke_width,
                ink[1] - stroke_width,
                ink[2] + stroke_width,
                ink[3] + stroke_width,
            ];
        }
        glyph_rects.push(ink);

        let commands = bez_to_commands(&path);
        let lit = request.highlight.as_ref().is_some_and(|h| {
            glyph.cluster.start < h.range.end && glyph.cluster.end > h.range.start
        });
        if font.embolden {
            bold_width = bold_width.max(font.font_size * 0.02);
            bold_commands.extend(commands.iter().copied());
            if lit {
                lit_bold.extend(commands.iter().copied());
            }
        }
        if lit {
            lit_commands.extend(commands.iter().copied());
        }
        fill_commands.extend(commands);
    }

    // --- underline ------------------------------------------------------
    //
    // Part of the fill geometry, so it is outlined and casts a shadow like
    // the letters it sits under. Each glyph's rectangle grows down over the
    // stretch of line under its advance, so a per-letter animation carries
    // its piece of the line with it instead of dropping it.
    if request.underline {
        let mut path = BezPath::new();
        for line in &layout.lines {
            let glyphs = &layout.glyphs[line.glyphs.clone()];
            let Some((offset, thickness)) = glyphs
                .iter()
                .filter_map(|g| fonts.get(g.run))
                .map(RunFont::underline)
                .max_by(|a, b| a.1.total_cmp(&b.1))
            else {
                continue;
            };
            if line.width <= 0.0 {
                continue;
            }
            let top = origin.1 + line.baseline + offset;
            let bottom = top + thickness;
            let left = origin.0 + line.x;
            let right = left + line.width;
            path.extend(
                kurbo::Rect::new(left as f64, top as f64, right as f64, bottom as f64)
                    .path_elements(0.1),
            );
            let grow = stroke_width;
            for (index, glyph) in layout.glyphs[line.glyphs.clone()].iter().enumerate() {
                let rect = &mut glyph_rects[line.glyphs.start + index];
                let x0 = (origin.0 + glyph.x).max(left);
                let x1 = (origin.0 + glyph.x + glyph.advance).min(right);
                if x1 <= x0 {
                    continue;
                }
                rect[0] = rect[0].min(x0 - grow);
                rect[1] = rect[1].min(top - grow);
                rect[2] = rect[2].max(x1 + grow);
                rect[3] = rect[3].max(bottom + grow);
            }
        }
        fill_commands.extend(bez_to_commands(&path));
    }
    timing.outlines = timing.mark();

    // --- masks ----------------------------------------------------------
    //
    // Two scan conversions at most, whatever effects are on: the glyphs alone,
    // and the glyphs grown by the outline. The drop shadow is the second of
    // those moved sideways, not a third pass — see `Coverage::shifted`.
    let fill_layer = Layer::fill(&fill_commands);
    let bold_layer = Layer::stroke(&bold_commands, bold_width);
    let outline_layer = Layer::stroke(&fill_commands, stroke_width);

    let mut fill_layers: Vec<Layer<'_>> = vec![fill_layer];
    if bold_width > 0.0 {
        fill_layers.push(bold_layer);
    }
    let fill_mask = Coverage::render(&fill_layers, clip);

    // The silhouette of everything painted: what casts the shadow, and — when
    // the fill is opaque — also what the outline colour is painted through,
    // since the fill covers its inner half anyway.
    let silhouette = if stroke_width > 0.0 {
        let mut layers = fill_layers.clone();
        layers.push(outline_layer);
        Coverage::render(&layers, clip)
    } else {
        None
    };

    // --- shadow ---------------------------------------------------------
    if let Some(shadow) = request.shadow {
        // CSS-style: the declared blur is roughly the visible spread, twice the
        // Gaussian sigma. Users type numbers they have seen in other editors.
        let sigma = (shadow.blur.max(0.0) * scale) * 0.5;
        let offset = (shadow.offset[0] * scale, shadow.offset[1] * scale);
        if let Some(source) = silhouette.as_ref().or(fill_mask.as_ref()) {
            let mut mask = source.shifted(offset, sigma * 3.0, clip);
            mask.blur(sigma);
            canvas.blend_mask(&mask, shadow.color);
        }
        timing.shadow = timing.mark();
    }

    // --- outline, then fill ---------------------------------------------
    if stroke_width > 0.0 {
        // A translucent fill would let the outline colour show through the
        // whole glyph if the silhouette were used, which is not what
        // `paint-order: stroke fill` means. It costs a third scan conversion,
        // and only when the fill is actually see-through.
        if request.color[3] >= 1.0 {
            if let Some(mask) = silhouette.as_ref() {
                canvas.blend_mask(mask, request.stroke_color);
            }
        } else if let Some(mask) = Coverage::render(&[outline_layer], clip) {
            canvas.blend_mask(&mask, request.stroke_color);
        }
        timing.outline = timing.mark();
    }
    if let Some(mask) = fill_mask.as_ref() {
        canvas.blend_mask(mask, request.color);
        timing.fill = timing.mark();
    }
    if let Some(highlight) = request.highlight.as_ref() {
        if !lit_commands.is_empty() {
            let mut layers = vec![Layer::fill(&lit_commands)];
            if bold_width > 0.0 && !lit_bold.is_empty() {
                layers.push(Layer::stroke(&lit_bold, bold_width));
            }
            if let Some(mask) = Coverage::render(&layers, clip) {
                canvas.blend_mask(&mask, highlight.color);
            }
        }
    }

    // --- colour bitmaps (emoji) -----------------------------------------
    for draw in &bitmaps {
        blit_scaled(&mut canvas, draw);
    }

    let pixels = canvas.finish();
    timing.finish = timing.mark();
    timing.report(&layout, width, height);

    RasteredText {
        width,
        height,
        pixels,
        origin,
        layout,
        glyph_rects,
    }
}

/// Where the time in one rasterisation went.
///
/// Always on, at `debug`, for the same reason `media::provider` logs its decode
/// and upload split: when a preview stutters, the only way to tell whether the
/// title or the footage caused it is to have the numbers already.
struct Timing {
    started: std::time::Instant,
    background: f64,
    outlines: f64,
    shadow: f64,
    outline: f64,
    fill: f64,
    finish: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            background: 0.0,
            outlines: 0.0,
            shadow: 0.0,
            outline: 0.0,
            fill: 0.0,
            finish: 0.0,
        }
    }
}

impl Timing {
    /// Milliseconds since the last call, and restart.
    fn mark(&mut self) -> f64 {
        let elapsed = self.started.elapsed().as_secs_f64() * 1000.0;
        self.started = std::time::Instant::now();
        elapsed
    }

    fn report(&self, layout: &TextLayout, width: u32, height: u32) {
        tracing::debug!(
            size = format_args!("{width}x{height}"),
            glyphs = layout.glyphs.len(),
            background_ms = self.background,
            outlines_ms = self.outlines,
            shadow_ms = self.shadow,
            outline_ms = self.outline,
            fill_ms = self.fill,
            finish_ms = self.finish,
            "rasterized a text layer"
        );
    }
}

/// Fonts opened once per run rather than once per glyph.
struct RunFonts<'a> {
    runs: Vec<Option<RunFont<'a>>>,
}

struct RunFont<'a> {
    /// Parsed once per run rather than once per glyph. `outline_glyphs()` and
    /// `BitmapStrikes::new` re-read the font's table directory every call, and
    /// a title is dozens of glyphs from one font.
    outlines: skrifa::outline::OutlineGlyphCollection<'a>,
    strikes: skrifa::bitmap::BitmapStrikes<'a>,
    coords: Vec<NormalizedCoord>,
    font_size: f32,
    units_per_em: f32,
    embolden: bool,
    /// `tan` of the faux-italic angle, applied as a shear in y-up glyph space.
    shear: f32,
    /// Where the underline goes: the distance from the baseline down to its
    /// top edge, and its thickness, in device pixels.
    underline: (f32, f32),
}

impl<'a> RunFonts<'a> {
    fn new(layout: &'a TextLayout) -> Self {
        Self {
            runs: layout.runs.iter().map(RunFont::open).collect(),
        }
    }

    fn get(&self, index: usize) -> Option<&RunFont<'a>> {
        self.runs.get(index).and_then(|f| f.as_ref())
    }
}

impl<'a> RunFont<'a> {
    fn open(style: &'a GlyphRunStyle) -> Option<Self> {
        let data: &'a [u8] = style.font.data.as_ref();
        let font = FontRef::from_index(data, style.font.index).ok()?;
        let units_per_em = font
            .head()
            .map(|h| h.units_per_em() as f32)
            .unwrap_or(1000.0);
        Some(Self {
            outlines: font.outline_glyphs(),
            strikes: font.bitmap_strikes(),
            coords: style
                .coords
                .iter()
                .map(|bits| NormalizedCoord::from_bits(*bits))
                .collect(),
            font_size: style.font_size,
            units_per_em,
            embolden: style.embolden,
            shear: style.skew.to_radians().tan(),
            underline: underline_metrics(&font, style),
        })
    }

    fn underline(&self) -> (f32, f32) {
        self.underline
    }

    /// The glyph's outline, already in image space (y down, pen at `pen`).
    fn outline(&self, glyph_id: u32, pen: (f32, f32)) -> Option<BezPath> {
        let glyph = self.outlines.get(GlyphId::new(glyph_id))?;
        let mut pen_sink = OutlineSink {
            path: BezPath::new(),
            pen,
            shear: self.shear,
            open: false,
        };
        glyph
            .draw(
                DrawSettings::unhinted(Size::new(self.font_size), LocationRef::new(&self.coords)),
                &mut pen_sink,
            )
            .ok()?;
        if pen_sink.open {
            pen_sink.path.close_path();
        }
        if pen_sink.path.elements().is_empty() {
            return None;
        }
        Some(pen_sink.path)
    }

    /// A colour bitmap for this glyph, if the font has one at this size.
    fn bitmap(&self, glyph_id: u32, pen: (f32, f32)) -> Option<BitmapDraw> {
        if self.strikes.is_empty() {
            return None;
        }
        let glyph = self
            .strikes
            .glyph_for_size(Size::new(self.font_size), GlyphId::new(glyph_id))?;
        if glyph.width == 0 || glyph.height == 0 {
            return None;
        }

        let rgba = decode_bitmap(&glyph)?;
        // The strike was designed for `ppem` pixels per em; we want `font_size`.
        let pixel_scale = self.font_size / glyph.ppem_y.max(1.0);
        let unit_scale = self.font_size / self.units_per_em;
        let x = pen.0 + glyph.bearing_x * unit_scale + glyph.inner_bearing_x * pixel_scale;
        let above = glyph.bearing_y * unit_scale + glyph.inner_bearing_y * pixel_scale;
        let height = glyph.height as f32 * pixel_scale;
        let top = match glyph.placement_origin {
            skrifa::bitmap::Origin::TopLeft => pen.1 - above,
            skrifa::bitmap::Origin::BottomLeft => pen.1 - above - height,
        };
        Some(BitmapDraw {
            rgba,
            src_width: glyph.width,
            src_height: glyph.height,
            rect: [x, top, x + glyph.width as f32 * pixel_scale, top + height],
        })
    }
}

/// Decode a bitmap glyph to straight-alpha RGBA8.
fn decode_bitmap(glyph: &skrifa::bitmap::BitmapGlyph<'_>) -> Option<Vec<u8>> {
    let pixels = (glyph.width as usize) * (glyph.height as usize);
    match &glyph.data {
        skrifa::bitmap::BitmapData::Png(bytes) => {
            let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
                .ok()?
                .to_rgba8();
            if decoded.width() != glyph.width || decoded.height() != glyph.height {
                return None;
            }
            Some(decoded.into_raw())
        }
        skrifa::bitmap::BitmapData::Bgra(bytes) => {
            if bytes.len() < pixels * 4 {
                return None;
            }
            // Premultiplied BGRA in the font, straight RGBA out.
            let mut out = vec![0u8; pixels * 4];
            for i in 0..pixels {
                let (b, g, r, a) = (
                    bytes[i * 4],
                    bytes[i * 4 + 1],
                    bytes[i * 4 + 2],
                    bytes[i * 4 + 3],
                );
                let unpremultiply = |v: u8| {
                    if a == 0 {
                        0
                    } else {
                        ((v as f32 * 255.0 / a as f32).round()).clamp(0.0, 255.0) as u8
                    }
                };
                out[i * 4] = unpremultiply(r);
                out[i * 4 + 1] = unpremultiply(g);
                out[i * 4 + 2] = unpremultiply(b);
                out[i * 4 + 3] = a;
            }
            Some(out)
        }
        // A single-channel strike is a monochrome bitmap font; drawing it white
        // would be wrong and drawing it in the fill colour needs the fill
        // colour, which this function does not have. Outlines cover every case
        // we have actually met, so this is left undone rather than guessed at.
        skrifa::bitmap::BitmapData::Mask(_) => None,
    }
}

/// Draw a decoded bitmap into the canvas, scaled, with bilinear sampling.
fn blit_scaled(canvas: &mut Canvas, draw: &BitmapDraw) {
    let [x0, y0, x1, y1] = draw.rect;
    let (dst_w, dst_h) = (x1 - x0, y1 - y0);
    if dst_w <= 0.0 || dst_h <= 0.0 {
        return;
    }
    let px0 = x0.floor().max(0.0) as i32;
    let py0 = y0.floor().max(0.0) as i32;
    let px1 = (x1.ceil() as i32).min(canvas.width as i32);
    let py1 = (y1.ceil() as i32).min(canvas.height as i32);
    canvas.mark(px0, py0, px1, py1);

    for py in py0..py1 {
        for px in px0..px1 {
            let u = (px as f32 + 0.5 - x0) / dst_w;
            let v = (py as f32 + 0.5 - y0) / dst_h;
            if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                continue;
            }
            let sx = (u * draw.src_width as f32 - 0.5).max(0.0);
            let sy = (v * draw.src_height as f32 - 0.5).max(0.0);
            let (ix, iy) = (sx.floor() as usize, sy.floor() as usize);
            let (fx, fy) = (sx - ix as f32, sy - iy as f32);
            let sample = |x: usize, y: usize| -> [f32; 4] {
                let x = x.min(draw.src_width as usize - 1);
                let y = y.min(draw.src_height as usize - 1);
                let i = (y * draw.src_width as usize + x) * 4;
                [
                    draw.rgba[i] as f32,
                    draw.rgba[i + 1] as f32,
                    draw.rgba[i + 2] as f32,
                    draw.rgba[i + 3] as f32,
                ]
            };
            let (c00, c10, c01, c11) = (
                sample(ix, iy),
                sample(ix + 1, iy),
                sample(ix, iy + 1),
                sample(ix + 1, iy + 1),
            );
            let mut texel = [0.0f32; 4];
            for c in 0..4 {
                let top = c00[c] * (1.0 - fx) + c10[c] * fx;
                let bottom = c01[c] * (1.0 - fx) + c11[c] * fx;
                texel[c] = top * (1.0 - fy) + bottom * fy;
            }
            let src_a = texel[3] / 255.0;
            if src_a <= 0.0 {
                continue;
            }
            canvas.over(
                px as usize,
                py as usize,
                [
                    (texel[0] * src_a).round() as u32,
                    (texel[1] * src_a).round() as u32,
                    (texel[2] * src_a).round() as u32,
                    texel[3].round() as u32,
                ],
            );
        }
    }
}

/// Collects skrifa's outline into a `kurbo` path, flipped into image space.
///
/// Font outlines are y-up with the origin on the baseline; images are y-down
/// with the origin at the top-left corner. Doing the flip here rather than with
/// a transform afterwards means every path in this module is already in the
/// coordinate system the rasteriser and the layout both use.
struct OutlineSink {
    path: BezPath,
    pen: (f32, f32),
    shear: f32,
    open: bool,
}

impl OutlineSink {
    fn map(&self, x: f32, y: f32) -> Point {
        Point::new(
            (self.pen.0 + x + self.shear * y) as f64,
            (self.pen.1 - y) as f64,
        )
    }
}

impl skrifa::outline::OutlinePen for OutlineSink {
    fn move_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.path.close_path();
        }
        self.path.move_to(self.map(x, y));
        self.open = true;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        if self.open {
            self.path.line_to(self.map(x, y));
        }
    }

    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        if self.open {
            self.path.quad_to(self.map(cx, cy), self.map(x, y));
        }
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        if self.open {
            self.path
                .curve_to(self.map(cx0, cy0), self.map(cx1, cy1), self.map(x, y));
        }
    }

    fn close(&mut self) {
        if self.open {
            self.path.close_path();
            self.open = false;
        }
    }
}

fn bez_to_commands(path: &BezPath) -> Vec<Command> {
    let mut out = Vec::with_capacity(path.elements().len());
    for element in path.elements() {
        match element {
            PathEl::MoveTo(p) => out.push(Command::MoveTo(point(*p))),
            PathEl::LineTo(p) => out.push(Command::LineTo(point(*p))),
            PathEl::QuadTo(c, p) => out.push(Command::QuadTo(point(*c), point(*p))),
            PathEl::CurveTo(c0, c1, p) => {
                out.push(Command::CurveTo(point(*c0), point(*c1), point(*p)));
            }
            PathEl::ClosePath => out.push(Command::Close),
        }
    }
    out
}

fn point(p: Point) -> zeno::Point {
    zeno::Point::new(p.x as f32, p.y as f32)
}

/// A 0..1 colour component as a byte.
fn channel(value: f32) -> u32 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u32
}

/// `(v + 127) / 255`, exactly, without a division.
fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}

/// Control-point bounds. Conservative for curves, which is what a raster
/// clip rectangle wants.
fn path_bounds(commands: &[Command]) -> Option<[f32; 4]> {
    let mut bounds: Option<[f32; 4]> = None;
    let mut add = |p: zeno::Point| {
        bounds = Some(match bounds {
            None => [p.x, p.y, p.x, p.y],
            Some(b) => [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)],
        });
    };
    for command in commands {
        match *command {
            Command::MoveTo(p) | Command::LineTo(p) => add(p),
            Command::QuadTo(c, p) => {
                add(c);
                add(p);
            }
            Command::CurveTo(c0, c1, p) => {
                add(c0);
                add(c1);
                add(p);
            }
            Command::Close => {}
        }
    }
    bounds
}

fn bez_bounds(path: &BezPath) -> Option<[f32; 4]> {
    if path.elements().is_empty() {
        return None;
    }
    let rect = path.bounding_box();
    Some([
        rect.x0 as f32,
        rect.y0 as f32,
        rect.x1 as f32,
        rect.y1 as f32,
    ])
}
