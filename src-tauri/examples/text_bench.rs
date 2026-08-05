//! What a title costs to rasterise.
//!
//! ```bash
//! cargo run --release --example text_bench
//! cargo run --release --example text_bench -- --write-golden
//! ```
//!
//! The second form regenerates the committed golden image for
//! `modules::text::tests::golden_image_matches`. Do that only when the change
//! to the picture was intended, and look at the file afterwards.

use std::time::Instant;

use chukcut_lib::modules::text::{RasterOptions, TextRenderer, TextRequest};

const SHORT: &str = "How I edit 10 videos a day";
const PARAGRAPH: &str = "The quick brown fox jumps over the lazy dog. \
Pack my box with five dozen liquor jugs. \
How vexingly quick daft zebras jump! \
Sphinx of black quartz, judge my vow. \
The five boxing wizards jump quickly.";

/// Minimum and median, in that order.
///
/// The minimum is quoted first and is the number to compare across changes.
/// This machine routinely runs at three or four times its core count while
/// several agents build, and a median under that load measures the scheduler:
/// the same case has been seen at 4.7 ms and 15.1 ms minutes apart. The
/// minimum of enough runs is the closest thing to an uncontended measurement
/// available without an idle machine.
fn stats(mut values: Vec<f64>) -> (f64, f64) {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (values[0], values[values.len() / 2])
}

fn bench(label: &str, renderer: &TextRenderer, request: &TextRequest, options: &RasterOptions) {
    // Cold: the cache is bypassed entirely, so this is shaping plus painting.
    let mut cold = Vec::new();
    for _ in 0..25 {
        let start = Instant::now();
        let image = renderer.rasterize_uncached(request, options);
        cold.push(start.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(image.pixels.len());
    }

    // Shaping alone, to say which half the time is in.
    let mut layout_only = Vec::new();
    for _ in 0..25 {
        let start = Instant::now();
        let layout = renderer.layout(request, options);
        layout_only.push(start.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(layout.glyphs.len());
    }

    renderer.clear_cache();
    let mut warm = Vec::new();
    for _ in 0..2000 {
        let start = Instant::now();
        let hit = renderer.rasterize(request, options);
        warm.push(start.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(hit.width);
    }
    // The first call filled the cache and is not a warm measurement.
    warm.remove(0);

    let image = renderer.rasterize(request, options);
    let (cold_min, cold_median) = stats(cold);
    let (layout_min, _) = stats(layout_only);
    let (warm_min, _) = stats(warm);

    println!(
        "{label:<28} {:>4} glyphs {:>2} lines  {:>4}x{:<4}  \
         cold {:>6.2} ms (median {:>6.2})  layout {:>5.2}  warm {:>7.4} ms",
        image.layout.glyphs.len(),
        image.layout.lines.len(),
        image.width,
        image.height,
        cold_min,
        cold_median,
        layout_min,
        warm_min,
    );
}

fn main() {
    // `RUST_LOG=debug` turns on the per-stage breakdown `raster.rs` always
    // emits, which is how you find out whether the shadow or the outline is
    // what a slow title is spending its time on.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let renderer = TextRenderer::new();

    let start = Instant::now();
    let families = renderer.font_families().len();
    println!(
        "font database: {families} families, discovered in {:.1} ms\n",
        start.elapsed().as_secs_f64() * 1000.0
    );

    if std::env::args().any(|a| a == "--write-golden") {
        write_golden(&renderer);
        return;
    }

    let title = TextRequest {
        content: SHORT.into(),
        font_family: "sans-serif".into(),
        font_size: 72.0,
        color: [1.0, 1.0, 1.0, 1.0],
        ..TextRequest::default()
    };
    let mut outlined = title.clone();
    outlined.stroke_width = 4.0;
    outlined.stroke_color = [0.0, 0.0, 0.0, 1.0];
    let mut full = outlined.clone();
    full.shadow = Some(chukcut_lib::modules::project::document::TextShadow {
        color: [0.0, 0.0, 0.0, 0.7],
        offset: [6.0, 6.0],
        blur: 12.0,
    });
    full.background = Some([0.0, 0.0, 0.0, 0.35]);

    let paragraph = TextRequest {
        content: PARAGRAPH.into(),
        font_family: "sans-serif".into(),
        font_size: 40.0,
        color: [1.0, 1.0, 1.0, 1.0],
        ..TextRequest::default()
    };

    for (name, size) in [
        ("1920x1080", (1920u32, 1080u32)),
        ("1080x1920", (1080, 1920)),
    ] {
        let options = RasterOptions::canvas(size.0, size.1);
        println!("-- {name} --");
        bench("short title", &renderer, &title, &options);
        bench("short title + outline", &renderer, &outlined, &options);
        bench("short title, everything on", &renderer, &full, &options);
        bench("paragraph", &renderer, &paragraph, &options);
        println!();
    }

    println!("-- tight rasters (thumbnail sized) --");
    bench("short title", &renderer, &title, &RasterOptions::tight());
    bench(
        "paragraph, wrapped at 900",
        &renderer,
        &paragraph,
        &RasterOptions::tight().with_max_width(900.0),
    );
}

fn write_golden(renderer: &TextRenderer) {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/modules/text/testdata/golden_ag.png"
    );
    let image = renderer.rasterize_uncached(
        &TextRequest {
            content: "Ag".into(),
            font_family: "DejaVu Sans".into(),
            font_size: 32.0,
            color: [1.0, 1.0, 1.0, 1.0],
            ..TextRequest::default()
        },
        &RasterOptions::tight(),
    );
    let buffer =
        image::RgbaImage::from_raw(image.width, image.height, image.pixels.clone()).unwrap();
    buffer.save(path).unwrap();
    println!("wrote {}x{} golden to {path}", image.width, image.height);
}
