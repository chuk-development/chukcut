//! Behaviour that could plausibly break.
//!
//! Two rules govern what is asserted here. The first is that anything depending
//! on *which* fonts the machine has is checked against a family the test has
//! confirmed exists, and skipped with a message otherwise — a red test on a
//! machine without DejaVu tells us nothing about our code. The second is that
//! nothing asserts an exact pixel unless the golden image is committed next to
//! it, because a font update is allowed to change a glyph and is not allowed to
//! fail the build.

use super::*;
use crate::modules::project::document::{TextAlign, TextShadow};

/// A family the test needs, or `None` if this machine does not have it.
fn family(renderer: &TextRenderer, wanted: &str) -> Option<String> {
    renderer
        .font_families()
        .into_iter()
        .find(|name| name == wanted)
}

fn request(content: &str, family: &str, size: f32) -> TextRequest {
    TextRequest {
        content: content.to_string(),
        font_family: family.to_string(),
        font_size: size,
        ..TextRequest::default()
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

#[test]
fn a_monospaced_string_is_as_wide_as_its_character_count() {
    let renderer = TextRenderer::new();
    let Some(mono) = family(&renderer, "DejaVu Sans Mono") else {
        eprintln!("skipped: DejaVu Sans Mono is not installed");
        return;
    };

    let options = RasterOptions::tight();
    let one = renderer.layout(&request("M", &mono, 64.0), &options);
    let four = renderer.layout(&request("MMMM", &mono, 64.0), &options);
    let narrow = renderer.layout(&request("iiii", &mono, 64.0), &options);

    assert!(one.width > 0.0);
    assert!(
        (four.width - one.width * 4.0).abs() < 1.0,
        "four Ms should be four times one M: {} vs {}",
        four.width,
        one.width * 4.0
    );
    assert!(
        (four.width - narrow.width).abs() < 1.0,
        "monospace means iiii and MMMM are the same width: {} vs {}",
        narrow.width,
        four.width
    );
}

#[test]
fn width_scales_with_font_size() {
    let renderer = TextRenderer::new();
    let options = RasterOptions::tight();
    let small = renderer.layout(&request("Hamburgefonstiv", "sans-serif", 32.0), &options);
    let large = renderer.layout(&request("Hamburgefonstiv", "sans-serif", 64.0), &options);
    assert!(small.width > 0.0);
    let ratio = large.width / small.width;
    assert!(
        (ratio - 2.0).abs() < 0.05,
        "doubling the size should double the width, got {ratio}"
    );
}

#[test]
fn the_scale_factor_changes_pixels_and_not_proportions() {
    let renderer = TextRenderer::new();
    let request = request("Export me", "sans-serif", 40.0);
    let one = renderer.layout(&request, &RasterOptions::tight());
    let two = renderer.layout(&request, &RasterOptions::tight().with_scale(2.0));
    let ratio = two.width / one.width;
    assert!(
        (ratio - 2.0).abs() < 0.05,
        "a 2x raster is twice as wide in device pixels, got {ratio}"
    );
}

// ---------------------------------------------------------------------------
// Shaping
// ---------------------------------------------------------------------------

#[test]
fn arabic_is_shaped_rather_than_spelled_out() {
    let renderer = TextRenderer::new();
    // Lam followed by alef is a *mandatory* ligature in Arabic: any shaper
    // worth having produces one glyph, and a renderer that blits characters
    // produces two and is wrong in a way Arabic readers notice immediately.
    let layout = renderer.layout(&request("لا", "sans-serif", 48.0), &RasterOptions::tight());
    if layout.glyphs.is_empty() {
        eprintln!("skipped: no font on this machine covers Arabic");
        return;
    }
    assert_eq!(
        layout.glyphs.len(),
        1,
        "lam-alef must shape to one glyph, got {:?}",
        layout.glyphs
    );
    assert!(layout.is_rtl, "an Arabic paragraph is right to left");
}

#[test]
fn hebrew_is_ordered_right_to_left() {
    let renderer = TextRenderer::new();
    // Three separate letters, no ligatures, so glyph order is purely bidi.
    let layout = renderer.layout(&request("אבג", "sans-serif", 48.0), &RasterOptions::tight());
    if layout.glyphs.len() < 3 {
        eprintln!("skipped: no font on this machine covers Hebrew");
        return;
    }

    assert!(layout.is_rtl);
    let glyphs = layout.line_glyphs(0);
    assert!(glyphs.windows(2).all(|w| w[0].x <= w[1].x), "visual order");
    // The first letter of the string is the *rightmost* glyph, so the cluster
    // offsets run backwards along x.
    assert!(
        glyphs[0].cluster.start > glyphs[glyphs.len() - 1].cluster.start,
        "leftmost glyph should come from the end of the string: {:?}",
        glyphs
            .iter()
            .map(|g| (g.x, g.cluster.clone()))
            .collect::<Vec<_>>()
    );
    assert!(glyphs.iter().all(|g| g.rtl));
}

#[test]
fn latin_is_ordered_left_to_right() {
    let renderer = TextRenderer::new();
    let layout = renderer.layout(&request("abc", "sans-serif", 48.0), &RasterOptions::tight());
    assert_eq!(layout.glyphs.len(), 3);
    let glyphs = layout.line_glyphs(0);
    assert!(!layout.is_rtl);
    assert!(glyphs[0].cluster.start < glyphs[2].cluster.start);
    assert!(glyphs.iter().all(|g| !g.rtl));
}

#[test]
fn cjk_lays_out_and_does_not_fall_back_to_boxes() {
    let renderer = TextRenderer::new();
    let layout = renderer.layout(
        &request("日本語のテキスト", "sans-serif", 48.0),
        &RasterOptions::tight(),
    );
    if layout.glyphs.is_empty() {
        eprintln!("skipped: no font on this machine covers CJK");
        return;
    }
    assert_eq!(layout.glyphs.len(), 8);
    // A CJK glyph is about one em wide, so eight of them at 48px is about 384.
    assert!(
        layout.width > 300.0 && layout.width < 450.0,
        "unexpected CJK advance: {}",
        layout.width
    );
    assert!(
        layout.glyphs.iter().all(|g| g.id != 0),
        "glyph 0 is .notdef — the font fell back to tofu"
    );
}

// ---------------------------------------------------------------------------
// Breaking and alignment
// ---------------------------------------------------------------------------

#[test]
fn text_wraps_inside_the_requested_width() {
    let renderer = TextRenderer::new();
    let options = RasterOptions::tight().with_max_width(300.0);
    let layout = renderer.layout(
        &request(
            "The quick brown fox jumps over the lazy dog and keeps going",
            "sans-serif",
            32.0,
        ),
        &options,
    );
    assert!(layout.lines.len() > 1, "should have wrapped");
    for line in &layout.lines {
        assert!(
            line.width <= 300.5,
            "line {} is {} wide, over the 300 limit",
            line.index,
            line.width
        );
    }
    assert!(layout.height > 32.0 * (layout.lines.len() as f32 - 1.0));
}

#[test]
fn an_explicit_newline_breaks_even_without_a_width() {
    let renderer = TextRenderer::new();
    let layout = renderer.layout(
        &request("first\nsecond\nthird", "sans-serif", 32.0),
        &RasterOptions::tight(),
    );
    assert_eq!(layout.lines.len(), 3);
    assert!(layout.lines[1].y > layout.lines[0].y);
}

#[test]
fn alignment_moves_the_short_line_and_not_the_long_one() {
    let renderer = TextRenderer::new();
    let content = "short\na considerably longer line";

    let measure = |align: TextAlign| {
        let request = TextRequest {
            align,
            ..request(content, "sans-serif", 32.0)
        };
        let layout = renderer.layout(&request, &RasterOptions::tight().with_max_width(400.0));
        assert_eq!(layout.lines.len(), 2, "the fixture must be two lines");
        (
            layout.lines[0].x,
            layout.lines[1].x,
            layout.lines[0].width,
            layout.lines[1].width,
        )
    };

    let (left_short, left_long, short_width, long_width) = measure(TextAlign::Left);
    assert!(short_width < long_width, "the fixture must be uneven");
    assert!(left_short.abs() < 0.5 && left_long.abs() < 0.5);

    let (centre_short, centre_long, _, _) = measure(TextAlign::Center);
    let (right_short, right_long, _, _) = measure(TextAlign::Right);

    // Both lines shift, because the alignment box is the wrap width rather than
    // the longest line — but the short one has further to travel, and its right
    // edge should end up level with the long one's.
    assert!(
        centre_short > left_short + 1.0,
        "centring indents the short line"
    );
    assert!(
        centre_short - centre_long > (long_width - short_width) / 2.0 - 1.0,
        "the short line moves by half the difference in length"
    );
    assert!(
        right_short > centre_short + 1.0,
        "right alignment indents further than centring: {right_short} vs {centre_short}"
    );
    assert!(
        ((right_short + short_width) - (right_long + long_width)).abs() < 1.0,
        "right-aligned lines end level"
    );
}

// ---------------------------------------------------------------------------
// Degenerate input
// ---------------------------------------------------------------------------

#[test]
fn a_missing_font_falls_back_instead_of_panicking() {
    let renderer = TextRenderer::new();
    let layout = renderer.layout(
        &request("Fallback works", "Definitely Not Installed 9000", 48.0),
        &RasterOptions::tight(),
    );
    assert_eq!(layout.glyphs.len(), 14, "every character was shaped");
    assert!(layout.width > 0.0);
    assert!(
        layout.glyphs.iter().all(|g| g.id != 0),
        "the fallback font should actually cover Latin"
    );

    let image = renderer.rasterize_uncached(
        &request("Fallback works", "Definitely Not Installed 9000", 48.0),
        &RasterOptions::tight(),
    );
    assert!(
        image.pixels.iter().any(|&byte| byte != 0),
        "it drew something"
    );
}

#[test]
fn an_empty_string_produces_an_empty_but_valid_image() {
    let renderer = TextRenderer::new();
    let layout = renderer.layout(&request("", "sans-serif", 48.0), &RasterOptions::tight());
    assert!(layout.is_empty());

    let tight =
        renderer.rasterize_uncached(&request("", "sans-serif", 48.0), &RasterOptions::tight());
    assert!(
        tight.width >= 1 && tight.height >= 1,
        "never a zero-size texture"
    );
    assert_eq!(
        tight.pixels.len(),
        tight.width as usize * tight.height as usize * 4
    );

    let canvas = renderer.rasterize_uncached(
        &request("", "sans-serif", 48.0),
        &RasterOptions::canvas(320, 200),
    );
    assert_eq!((canvas.width, canvas.height), (320, 200));
    assert!(
        canvas.pixels.iter().all(|&byte| byte == 0),
        "an empty title must be fully transparent, not a black rectangle"
    );
    assert!(canvas.glyph_rects.is_empty());
}

#[test]
fn whitespace_only_content_does_not_panic() {
    let renderer = TextRenderer::new();
    let image = renderer.rasterize_uncached(
        &request("   \n \t ", "sans-serif", 48.0),
        &RasterOptions::canvas(200, 100),
    );
    assert_eq!((image.width, image.height), (200, 100));
    assert_eq!(image.glyph_rects.len(), image.layout.glyphs.len());
}

#[test]
fn a_string_of_only_emoji_renders_or_at_least_survives() {
    let renderer = TextRenderer::new();
    let request = request("😀🎬🔥", "sans-serif", 64.0);
    let layout = renderer.layout(&request, &RasterOptions::tight());
    if layout.glyphs.is_empty() {
        eprintln!("skipped: no emoji font on this machine");
        return;
    }
    assert_eq!(layout.glyphs.len(), 3, "one glyph per emoji");

    let image = renderer.rasterize_uncached(&request, &RasterOptions::tight());
    assert_eq!(image.glyph_rects.len(), 3);
    let opaque = image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] > 200)
        .count();
    assert!(
        opaque > 100,
        "a colour bitmap strike should have filled some pixels, got {opaque}"
    );
    // Colour emoji are colour: if the whole thing came out as one flat value
    // we drew the outline in the fill colour instead of the bitmap.
    let distinct: std::collections::HashSet<[u8; 3]> = image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] > 200)
        .map(|px| [px[0], px[1], px[2]])
        .collect();
    assert!(
        distinct.len() > 4,
        "expected many colours, got {}",
        distinct.len()
    );
}

#[test]
fn a_huge_font_size_on_a_small_canvas_is_clipped_not_crashed() {
    let renderer = TextRenderer::new();
    let image = renderer.rasterize_uncached(
        &request("BIG", "sans-serif", 4000.0),
        &RasterOptions::canvas(64, 64),
    );
    assert_eq!((image.width, image.height), (64, 64));
    assert_eq!(image.pixels.len(), 64 * 64 * 4);
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

#[test]
fn the_layer_is_canvas_sized_and_the_text_sits_inside_it() {
    let renderer = TextRenderer::new();
    let image = renderer.rasterize_uncached(
        &request("centre", "sans-serif", 48.0),
        &RasterOptions::canvas(640, 360),
    );
    assert_eq!((image.width, image.height), (640, 360));

    let ink: Vec<[f32; 4]> = image
        .glyph_rects
        .iter()
        .copied()
        .filter(|r| r[2] > r[0])
        .collect();
    assert!(!ink.is_empty());
    let top = ink.iter().fold(f32::MAX, |a, r| a.min(r[1]));
    let bottom = ink.iter().fold(f32::MIN, |a, r| a.max(r[3]));
    assert!(
        top > 100.0 && bottom < 260.0,
        "vertically centred: {top}..{bottom}"
    );
}

#[test]
fn a_background_box_fills_behind_the_text() {
    let renderer = TextRenderer::new();
    let mut request = request("box", "sans-serif", 48.0);
    request.background = Some([1.0, 0.0, 0.0, 1.0]);
    request.background_padding = Some(12.0);

    let image = renderer.rasterize_uncached(&request, &RasterOptions::tight());
    // The corner of a tight raster is inside the padded box and outside the
    // glyphs, so it is background and nothing else.
    let corner = image.pixel(image.width / 2, 2);
    assert!(
        corner[3] > 200,
        "the box should be opaque there: {corner:?}"
    );
    assert!(corner[0] > 200 && corner[1] < 60, "and red: {corner:?}");
}

#[test]
fn a_rounded_box_leaves_its_corners_clear() {
    let renderer = TextRenderer::new();
    let mut square = request("box", "sans-serif", 48.0);
    square.background = Some([1.0, 0.0, 0.0, 1.0]);
    square.background_padding = Some(16.0);
    let mut round = square.clone();
    round.background_radius = 24.0;

    let square = renderer.rasterize_uncached(&square, &RasterOptions::tight());
    let round = renderer.rasterize_uncached(&round, &RasterOptions::tight());
    // Two pixels in from the corner of a tight raster: inside a square box,
    // outside the curve of a rounded one.
    assert!(square.pixel(2, 2)[3] > 200, "{:?}", square.pixel(2, 2));
    assert!(round.pixel(2, 2)[3] < 40, "{:?}", round.pixel(2, 2));
}

#[test]
fn more_padding_makes_a_bigger_box() {
    let renderer = TextRenderer::new();
    let mut small = request("box", "sans-serif", 48.0);
    small.background = Some([1.0, 0.0, 0.0, 1.0]);
    small.background_padding = Some(4.0);
    let mut large = small.clone();
    large.background_padding = Some(30.0);
    let small = renderer.rasterize_uncached(&small, &RasterOptions::tight());
    let large = renderer.rasterize_uncached(&large, &RasterOptions::tight());
    assert!(
        large.width >= small.width + 50,
        "{} vs {}",
        large.width,
        small.width
    );
}

#[test]
fn letter_spacing_widens_the_line() {
    let renderer = TextRenderer::new();
    let tight = request("Spacing", "sans-serif", 48.0);
    let mut loose = tight.clone();
    loose.letter_spacing = 10.0;
    let options = RasterOptions::tight();
    let a = renderer.layout(&tight, &options).width;
    let b = renderer.layout(&loose, &options).width;
    // Seven letters, six gaps at least.
    assert!(b > a + 55.0, "{a} -> {b}");
}

#[test]
fn line_height_sets_the_distance_between_lines() {
    let renderer = TextRenderer::new();
    let mut single = request("one\ntwo", "sans-serif", 40.0);
    single.line_height = Some(1.0);
    let mut double = single.clone();
    double.line_height = Some(2.0);
    let options = RasterOptions::tight();
    let a = renderer.layout(&single, &options);
    let b = renderer.layout(&double, &options);
    let gap = |l: &TextLayout| l.lines[1].baseline - l.lines[0].baseline;
    assert!(
        (gap(&a) - 40.0).abs() < 2.0,
        "1.0 is one font size: {}",
        gap(&a)
    );
    assert!((gap(&b) - 80.0).abs() < 2.0, "2.0 is two: {}", gap(&b));
}

/// The longest run of opaque pixels in row `y`.
fn filled_run(image: &RasteredText, y: u32) -> u32 {
    let (mut best, mut run) = (0, 0);
    for x in 0..image.width {
        if image.pixel(x, y)[3] > 200 {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

#[test]
fn an_underline_runs_under_the_whole_line_and_moves_with_its_letters() {
    let renderer = TextRenderer::new();
    // No descenders, so anything solid below the baseline is the underline.
    let plain = request("onewar", "sans-serif", 64.0);
    let mut lined = plain.clone();
    lined.underline = true;

    let bare = renderer.rasterize_uncached(&plain, &RasterOptions::tight());
    let image = renderer.rasterize_uncached(&lined, &RasterOptions::tight());
    let line = &image.layout.lines[0];
    let baseline = (image.origin.1 + line.baseline).ceil() as u32 + 1;

    let longest = |image: &RasteredText| {
        (baseline..image.height)
            .map(|y| filled_run(image, y))
            .max()
            .unwrap_or(0)
    };
    assert!(longest(&bare) < 4, "nothing below the baseline without it");
    let run = longest(&image);
    assert!(
        run as f32 > line.width * 0.95,
        "a solid line as wide as the text: {run} of {}",
        line.width
    );

    // Every letter's rectangle reaches down over the line, so an animator
    // that moves a letter moves the piece of underline under it.
    let below = baseline as f32 + 2.0;
    for rect in &image.glyph_rects {
        assert!(rect[3] >= below, "{rect:?} stops above {below}");
    }
}

#[test]
fn a_material_carries_every_style_field_into_the_request() {
    let material = crate::modules::project::document::TextMaterial {
        content: "x".into(),
        underline: true,
        letter_spacing: 3.0,
        line_height: Some(1.3),
        background_padding: Some(9.0),
        background_radius: 7.0,
        ..Default::default()
    };
    let request = TextRequest::from(&material);
    assert!(request.underline);
    assert_eq!(request.letter_spacing, 3.0);
    assert_eq!(request.line_height, Some(1.3));
    assert_eq!(request.background_padding, Some(9.0));
    assert_eq!(request.background_radius, 7.0);
}

#[test]
fn a_title_saved_before_the_style_fields_existed_reads_with_their_defaults() {
    let old = r#"{"id":"t","content":"Hi","font_family":"sans-serif","font_size":40.0,
        "color":[1,1,1,1],"bold":false,"italic":false,"align":"center",
        "stroke_width":0.0,"stroke_color":[0,0,0,1],"shadow":null,"background":null}"#;
    let material: crate::modules::project::document::TextMaterial =
        serde_json::from_str(old).expect("an old title reads");
    assert!(!material.underline);
    assert_eq!(material.letter_spacing, 0.0);
    assert_eq!(material.line_height, None);
    assert_eq!(material.background_padding, None);
    assert_eq!(material.background_radius, 0.0);

    // And at their defaults they stay out of the file, so it saves as it did.
    let saved = serde_json::to_string(&material).expect("saves");
    for key in [
        "underline",
        "letter_spacing",
        "line_height",
        "background_padding",
        "background_radius",
    ] {
        assert!(!saved.contains(key), "{key} was written: {saved}");
    }
}

#[test]
fn a_stroke_widens_the_ink_and_paints_its_own_colour() {
    let renderer = TextRenderer::new();
    let plain = request("O", "sans-serif", 96.0);
    let mut outlined = plain.clone();
    outlined.stroke_width = 6.0;
    outlined.stroke_color = [1.0, 0.0, 0.0, 1.0];

    let bare = renderer.rasterize_uncached(&plain, &RasterOptions::tight());
    let stroked = renderer.rasterize_uncached(&outlined, &RasterOptions::tight());

    assert!(
        stroked.width > bare.width,
        "an outline makes the tight image wider: {} vs {}",
        stroked.width,
        bare.width
    );
    let red = stroked
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] > 200 && px[0] > 180 && px[1] < 70)
        .count();
    assert!(red > 200, "expected a red rim, found {red} pixels");
}

#[test]
fn a_shadow_darkens_pixels_the_glyph_does_not_cover() {
    let renderer = TextRenderer::new();
    let mut request = request("S", "sans-serif", 96.0);
    request.shadow = Some(TextShadow {
        color: [0.0, 0.0, 0.0, 1.0],
        offset: [10.0, 10.0],
        blur: 8.0,
    });

    let image = renderer.rasterize_uncached(&request, &RasterOptions::tight());
    let semi = image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] > 10 && px[3] < 200)
        .count();
    assert!(
        semi > 200,
        "a blur has soft edges; found {semi} partial pixels"
    );
}

// ---------------------------------------------------------------------------
// Caching
// ---------------------------------------------------------------------------

#[test]
fn the_same_title_is_rasterised_once() {
    let renderer = TextRenderer::new();
    let request = request("cache me", "sans-serif", 48.0);
    let options = RasterOptions::canvas(320, 200);

    let first = renderer.rasterize(&request, &options);
    let second = renderer.rasterize(&request, &options);
    assert!(
        Arc::ptr_eq(&first, &second),
        "the second call must return the cached image, not an equal one"
    );

    let stats = renderer.cache_stats();
    assert_eq!(stats.entries, 1);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 1);

    // A different size is a different picture.
    let bigger = renderer.rasterize(&request, &RasterOptions::canvas(640, 400));
    assert!(!Arc::ptr_eq(&first, &bigger));
    assert_eq!(renderer.cache_stats().entries, 2);

    renderer.clear_cache();
    assert_eq!(renderer.cache_stats().entries, 0);
}

#[test]
fn the_cache_stays_inside_its_budget() {
    // Room for one 200x200 layer and no more.
    let renderer = TextRenderer::with_budget(200 * 200 * 4);
    let options = RasterOptions::canvas(200, 200);
    for i in 0..6 {
        renderer.rasterize(
            &request(&format!("title {i}"), "sans-serif", 24.0),
            &options,
        );
    }
    let stats = renderer.cache_stats();
    assert!(
        stats.entries <= 2,
        "evicted down to {} entries",
        stats.entries
    );
    assert!(stats.bytes <= 200 * 200 * 4 * 2);
}

// ---------------------------------------------------------------------------
// Per-glyph output, which is what per-character animation will consume
// ---------------------------------------------------------------------------

#[test]
fn every_glyph_has_a_rectangle_inside_the_image() {
    let renderer = TextRenderer::new();
    let image = renderer.rasterize_uncached(
        &request("Animate me", "sans-serif", 60.0),
        &RasterOptions::canvas(800, 400),
    );
    assert_eq!(image.glyph_rects.len(), image.layout.glyphs.len());

    let mut previous_right = f32::MIN;
    for (glyph, rect) in image.layout.glyphs.iter().zip(&image.glyph_rects) {
        assert!(rect[0] >= -1.0 && rect[2] <= 801.0, "{rect:?}");
        assert!(rect[1] >= -1.0 && rect[3] <= 401.0, "{rect:?}");
        if rect[2] > rect[0] {
            assert!(
                rect[0] >= previous_right - 60.0,
                "glyphs should advance left to right"
            );
            previous_right = rect[2];
        }
        // The pen is inside its own glyph's box, give or take a bearing.
        let pen_x = image.origin.0 + glyph.x;
        assert!(pen_x >= rect[0] - 40.0 && pen_x <= rect[2] + 40.0);
    }
}

#[test]
fn a_ligature_reports_the_characters_it_came_from() {
    let renderer = TextRenderer::new();
    let layout = renderer.layout(&request("لا", "sans-serif", 48.0), &RasterOptions::tight());
    if layout.glyphs.len() != 1 {
        eprintln!("skipped: no Arabic font, covered by the shaping test");
        return;
    }
    let cluster = &layout.glyphs[0].cluster;
    assert_eq!(
        cluster.end - cluster.start,
        4,
        "one glyph covering two two-byte characters"
    );
}

// ---------------------------------------------------------------------------
// Golden image
// ---------------------------------------------------------------------------

/// The one pixel-exact test, against a committed PNG.
///
/// It is deliberately tiny — a 32px "Ag" — so the file is a couple of hundred
/// bytes and a diff is readable. The tolerance is there because a point release
/// of the font may move an edge by a code value, and the failure we want to
/// catch is "the renderer stopped drawing outlines", not that.
#[test]
fn golden_image_matches() {
    const GOLDEN: &[u8] = include_bytes!("testdata/golden_ag.png");

    let renderer = TextRenderer::new();
    let Some(family) = family(&renderer, "DejaVu Sans") else {
        eprintln!("skipped: DejaVu Sans is not installed");
        return;
    };

    let image = renderer.rasterize_uncached(
        &TextRequest {
            content: "Ag".into(),
            font_family: family,
            font_size: 32.0,
            color: [1.0, 1.0, 1.0, 1.0],
            ..TextRequest::default()
        },
        &RasterOptions::tight(),
    );

    let expected = image::load_from_memory_with_format(GOLDEN, image::ImageFormat::Png)
        .expect("golden image decodes")
        .to_rgba8();
    assert_eq!(
        (image.width, image.height),
        (expected.width(), expected.height()),
        "size changed; regenerate with `cargo run --example text_bench -- --write-golden`"
    );

    let mut worst = 0i32;
    let mut differing = 0usize;
    for (actual, expected) in image
        .pixels
        .as_chunks::<4>()
        .0
        .iter()
        .zip(expected.as_chunks::<4>().0)
    {
        // Only alpha is compared. The RGB of a transparent pixel is whatever
        // the edge bleed put there, which is deliberately unspecified.
        let delta = (actual[3] as i32 - expected[3] as i32).abs();
        worst = worst.max(delta);
        if delta > 0 {
            differing += 1;
        }
    }
    assert!(
        worst <= 8,
        "worst coverage difference {worst} over {differing} pixels"
    );
}
